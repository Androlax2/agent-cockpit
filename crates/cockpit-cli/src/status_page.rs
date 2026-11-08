//! `torlek status serve [--port N] [--host TAILNET_NAME]` (#850): one read-only status page for
//! the owner's phone. It listens on 127.0.0.1 only; the owner exposes it to their tailnet with `tailscale serve`.
//! It answers GET alone, refuses a foreign `Host` (DNS rebinding), and reads the daemon with
//! the owner's existing observe verbs. It never shows terminal contents, a run request's
//! command or directory, a session's working directory, or the daemon's own error messages,
//! which can name paths. What agents wrote (label, doing, question, reason) shows as written.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use cockpit_client::ClientError;
use cockpit_protocol::{
    Command, GuideStateView, ReadinessStatusView, Reply, RunStateView, SessionReport,
};
use serde_json::json;

use crate::daemon;

const DEFAULT_PORT: u16 = 8850;
const REFRESH_SECONDS: u32 = 15;
/// A client that sends no full request head within this is dropped.
const CLIENT_DEADLINE: Duration = Duration::from_secs(5);
const HEAD_LIMIT: usize = 16 * 1024;
/// A slower daemon is shown as not answering: the page is for noticing a stall.
const OBSERVE_DEADLINE: Duration = Duration::from_secs(10);
/// The daemon probes sign-in and Docker for every readiness request; the page refreshes every
/// 15 s, so one report, or one failure to get it, serves this long.
const READINESS_REUSE: Duration = Duration::from_secs(60);
/// The daemon's Docker probe gives up within 10 s; the server answers no one meanwhile.
const READINESS_DEADLINE: Duration = Duration::from_secs(20);
/// How long a closed answer waits for the rest of a refused request's body.
const DRAIN_DEADLINE: Duration = Duration::from_millis(250);
const DISK_REQUIREMENT: &str = "disk";

pub(crate) fn serve(arguments: &[OsString]) -> Result<ExitCode, String> {
    let options = parse(arguments)?;
    let port = options.port;
    if crate::agent::is_in_a_session() {
        return Ok(daemon::fail(
            "status serve is an owner verb: a session cannot start the status page",
        ));
    }
    let socket = match crate::agent::socket_dir() {
        Ok(dir) => dir.join(cockpit_config::DAEMON_SOCKET),
        Err(code) => return Ok(code),
    };
    // The only address this page ever binds: `tailscale serve` reaches it on the host.
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
        Ok(listener) => listener,
        Err(error) => {
            return Ok(daemon::fail(&format!(
                "cannot listen on 127.0.0.1:{port}: {error}"
            )));
        }
    };
    let port = match listener.local_addr() {
        Ok(address) => address.port(),
        Err(error) => {
            return Ok(daemon::fail(&format!(
                "cannot read the bound port: {error}"
            )));
        }
    };
    outln!(
        "serving http://127.0.0.1:{port}/ and /status.json on loopback only; expose it with `tailscale serve`"
    );
    if options.tailnet_host.is_none() {
        eprintln!(
            "torlek: without --host TAILNET_NAME, requests through `tailscale serve` are refused"
        );
    }
    let mut server = Server {
        socket,
        allowed_hosts: allowed_hosts(port, options.tailnet_host.as_deref()),
        readiness: None,
    };
    for stream in listener.incoming() {
        let answered = stream.and_then(|stream| server.answer(stream));
        if let Err(error) = answered {
            eprintln!("torlek: status page: {error}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

struct Options {
    port: u16,
    /// The machine's tailnet name `tailscale serve` forwards as `Host`.
    tailnet_host: Option<String>,
}

fn parse(arguments: &[OsString]) -> Result<Options, String> {
    let words = daemon::utf8_words(arguments)?;
    let mut options = Options {
        port: DEFAULT_PORT,
        tailnet_host: None,
    };
    let mut seen_port = false;
    let mut words = words.into_iter();
    while let Some(flag) = words.next() {
        let value = words
            .next()
            .filter(|value| !value.starts_with("--"))
            .ok_or_else(|| format!("{flag} needs a value"));
        match flag {
            "--port" if !seen_port => {
                let port = value?;
                options.port = port
                    .parse()
                    .map_err(|_| format!("--port {port}: not a port number"))?;
                seen_port = true;
            }
            "--host" if options.tailnet_host.is_none() => {
                options.tailnet_host = Some(tailnet_host(value?)?);
            }
            "--port" | "--host" => return Err(format!("{flag} given twice")),
            _ => {
                return Err(format!(
                    "status serve takes only --port N and --host NAME, not {flag}"
                ));
            }
        }
    }
    Ok(options)
}

/// One DNS name, matched exactly: a wildcard or suffix would let a rebound name through.
fn tailnet_host(name: &str) -> Result<String, String> {
    let is_dns_name = !name.is_empty()
        && name.split('.').all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        });
    if !is_dns_name {
        return Err(format!(
            "--host {name}: not one DNS name such as box.tailnet.ts.net (no port, no wildcard)"
        ));
    }
    Ok(name.to_ascii_lowercase())
}

/// Loopback with the bound port, and the tailnet name bare or with HTTPS's port, which is how
/// `tailscale serve` forwards it.
fn allowed_hosts(port: u16, tailnet_host: Option<&str>) -> Vec<String> {
    let mut hosts = vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if let Some(name) = tailnet_host {
        hosts.extend([name.to_owned(), format!("{name}:443")]);
    }
    hosts
}

struct Server {
    socket: PathBuf,
    /// `Host` values a request may carry; any other is refused against DNS rebinding.
    allowed_hosts: Vec<String>,
    readiness: Option<(Instant, Result<Readiness, String>)>,
}

impl Server {
    fn answer(&mut self, mut stream: TcpStream) -> std::io::Result<()> {
        stream.set_write_timeout(Some(CLIENT_DEADLINE))?;
        let response = match read_head(&mut stream)? {
            None => Response::refusal(400, "this request head cannot be read"),
            Some(head) => match route(&head, &self.allowed_hosts) {
                Route::Page => {
                    let snapshot = self.snapshot();
                    Response::new(
                        snapshot.status(),
                        "text/html; charset=utf-8",
                        render_page(&snapshot),
                    )
                }
                Route::Json => {
                    let snapshot = self.snapshot();
                    Response::new(
                        snapshot.status(),
                        "application/json",
                        render_json(&snapshot),
                    )
                }
                Route::Refused { status, reason } => Response::refusal(status, reason),
            },
        };
        stream.write_all(&response.to_bytes())?;
        finish(stream)
    }

    fn snapshot(&mut self) -> Snapshot {
        let started = Instant::now();
        let observed = observe(&self.socket);
        let answered_in = started.elapsed();
        let readiness = match &observed {
            Ok(_) => Some(self.readiness()),
            Err(_) => None,
        };
        Snapshot {
            generated_at: chrono::Utc::now().timestamp(),
            observed,
            answered_in,
            readiness,
        }
    }

    fn readiness(&mut self) -> Result<Readiness, String> {
        if let Some((fetched, readiness)) = &self.readiness
            && fetched.elapsed() < READINESS_REUSE
        {
            return readiness.clone();
        }
        let readiness = fetch_readiness(&self.socket);
        self.readiness = Some((Instant::now(), readiness.clone()));
        readiness
    }
}

/// The request head up to its blank line, or `None` when it is not UTF-8, too long, or the
/// client closed first. The whole head must arrive within [`CLIENT_DEADLINE`]: one slow client
/// holds every other.
fn read_head(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let deadline = Instant::now() + CLIENT_DEADLINE;
    let mut head = Vec::new();
    let mut chunk = [0; 2048];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        stream.set_read_timeout(Some(remaining))?;
        if let Some(end) = head.windows(4).position(|window| window == b"\r\n\r\n") {
            head.truncate(end);
            return Ok(String::from_utf8(head).ok());
        }
        if head.len() > HEAD_LIMIT {
            return Ok(None);
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Ok(None);
        }
        head.extend_from_slice(&chunk[..read]);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Route {
    Page,
    Json,
    Refused { status: u16, reason: &'static str },
}

/// The `Host` check comes first, so a rebound name learns nothing, not even which paths exist.
fn route(head: &str, allowed_hosts: &[String]) -> Route {
    let mut lines = head.split("\r\n");
    let request_line: Vec<&str> = lines.next().unwrap_or_default().split(' ').collect();
    let [method, target, version] = request_line[..] else {
        return Route::Refused {
            status: 400,
            reason: "malformed request line",
        };
    };
    if !version.starts_with("HTTP/1.") {
        return Route::Refused {
            status: 400,
            reason: "HTTP/1.x only",
        };
    }
    let hosts: Vec<&str> = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("host")
                .then_some(value.trim())
        })
        .collect();
    let [host] = hosts[..] else {
        return Route::Refused {
            status: 400,
            reason: "exactly one Host header is required",
        };
    };
    if !allowed_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(host))
    {
        return Route::Refused {
            status: 403,
            reason: "this Host is not served",
        };
    }
    if method != "GET" {
        return Route::Refused {
            status: 405,
            reason: "this page is read-only: GET only",
        };
    }
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    match path {
        "/" => Route::Page,
        "/status.json" => Route::Json,
        _ => Route::Refused {
            status: 404,
            reason: "not found",
        },
    }
}

/// Stops writing, then drains what the client still sends (a refused POST's body), so closing
/// does not reset the connection before the client reads the answer. The answer is already
/// written: a failure here can only cost the client that answer, so it is reported, not raised.
fn finish(stream: TcpStream) -> std::io::Result<()> {
    stream.shutdown(Shutdown::Write)?;
    stream.set_read_timeout(Some(DRAIN_DEADLINE))?;
    match std::io::copy(&mut stream.take(64 * 1024), &mut std::io::sink()) {
        Ok(_) => Ok(()),
        Err(error) if is_timeout(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

struct Response {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl Response {
    fn new(status: u16, content_type: &'static str, body: String) -> Response {
        Response {
            status,
            content_type,
            body,
        }
    }

    fn refusal(status: u16, reason: &str) -> Response {
        Response::new(status, "text/plain; charset=utf-8", format!("{reason}\n"))
    }

    fn to_bytes(&self) -> Vec<u8> {
        let reason = match self.status {
            200 => "OK",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            503 => "Service Unavailable",
            _ => "Error",
        };
        let allow = if self.status == 405 {
            "Allow: GET\r\n"
        } else {
            ""
        };
        format!(
            "HTTP/1.1 {} {reason}\r\n\
             Content-Type: {}\r\n\
             Content-Length: {}\r\n\
             {allow}\
             Cache-Control: no-store\r\n\
             Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; \
             base-uri 'none'; form-action 'none'; frame-ancestors 'none'\r\n\
             X-Content-Type-Options: nosniff\r\n\
             Referrer-Policy: no-referrer\r\n\
             Connection: close\r\n\r\n{}",
            self.status,
            self.content_type,
            self.body.len(),
            self.body
        )
        .into_bytes()
    }
}

struct Snapshot {
    /// Seconds since 1970, when this server assembled the page.
    generated_at: i64,
    observed: Result<Observed, String>,
    answered_in: Duration,
    /// `None` when the daemon did not answer the observe reads.
    readiness: Option<Result<Readiness, String>>,
}

impl Snapshot {
    fn status(&self) -> u16 {
        if self.observed.is_ok() { 200 } else { 503 }
    }
}

struct Observed {
    sessions: Vec<SessionRow>,
    approvals: Vec<WaitingApproval>,
}

struct SessionRow {
    id: String,
    project: Option<String>,
    state: String,
    report: Option<SessionReport>,
}

struct WaitingApproval {
    kind: &'static str,
    session: String,
    project: Option<String>,
    /// The run request's reason or the guide's title, never the command.
    reason: String,
    asked_at: i64,
}

#[derive(Clone)]
struct Readiness {
    checked_at: i64,
    requirements: Vec<RequirementRow>,
}

#[derive(Clone)]
struct RequirementRow {
    name: String,
    status: ReadinessStatusView,
    /// Only the disk guard's detail (free space and floor): other details can name paths.
    detail: Option<String>,
}

fn observe(socket: &Path) -> Result<Observed, String> {
    let commands = vec![
        Command::SessionList { ended: false },
        Command::AgentReports,
        Command::CheckoutList { project: None },
        Command::ApprovalQueue,
    ];
    let replies = exchange(socket, commands, OBSERVE_DEADLINE)?;
    let Ok(
        [
            Reply::Sessions { sessions },
            Reply::AgentReports { reports },
            Reply::Checkouts { checkouts },
            Reply::ApprovalQueue { runs, guides },
        ],
    ) = <[Reply; 4]>::try_from(replies)
    else {
        return Err("the daemon answered with unexpected replies".to_owned());
    };
    let mut reports: HashMap<String, SessionReport> = reports
        .into_iter()
        .map(|report| (report.session.clone(), report))
        .collect();
    let projects: HashMap<i64, String> = checkouts
        .into_iter()
        .map(|checkout| (checkout.id, checkout.project))
        .collect();
    let mut rows: Vec<SessionRow> = sessions
        .into_iter()
        .map(|session| SessionRow {
            project: projects.get(&session.checkout).cloned(),
            state: session.state.to_string(),
            report: reports.remove(&session.id),
            id: session.id,
        })
        .collect();
    rows.sort_by_key(|row| row.question().is_none());
    let waiting_runs = runs
        .into_iter()
        .filter(|run| run.state == RunStateView::Asking)
        .map(|run| WaitingApproval {
            kind: "ask run",
            session: run.session,
            project: run.project,
            reason: run.shown_reason.text,
            asked_at: run.asked_at,
        });
    let waiting_guides = guides
        .into_iter()
        .filter(|guide| guide.state == GuideStateView::Asking)
        .map(|guide| WaitingApproval {
            kind: "ask guide",
            session: guide.session,
            project: guide.project,
            reason: guide.title.text,
            asked_at: guide.asked_at,
        });
    Ok(Observed {
        sessions: rows,
        approvals: waiting_runs.chain(waiting_guides).collect(),
    })
}

impl SessionRow {
    fn question(&self) -> Option<&str> {
        self.report.as_ref()?.needs_input.as_deref()
    }
}

fn fetch_readiness(socket: &Path) -> Result<Readiness, String> {
    let replies = exchange(
        socket,
        vec![Command::Readiness {
            project: None,
            is_one_sign_in_enough: false,
        }],
        READINESS_DEADLINE,
    )?;
    let Ok([Reply::Readiness { requirements }]) = <[Reply; 1]>::try_from(replies) else {
        return Err("the daemon answered with an unexpected reply".to_owned());
    };
    Ok(Readiness {
        checked_at: chrono::Utc::now().timestamp(),
        requirements: requirements
            .into_iter()
            .map(|requirement| RequirementRow {
                detail: (requirement.name == DISK_REQUIREMENT).then_some(requirement.detail),
                name: requirement.name,
                status: requirement.status,
            })
            .collect(),
    })
}

/// The replies, or a fixed phrase for the page; the full error goes to this server's stderr.
fn exchange(
    socket: &Path,
    commands: Vec<Command>,
    deadline: Duration,
) -> Result<Vec<Reply>, String> {
    let answer = daemon::exchange_all(socket, commands, deadline)
        .map_err(|_| "this server cannot start its client runtime".to_owned())?;
    answer.map_err(|error| {
        eprintln!("torlek: status page: {error}");
        match error {
            ClientError::Connect { .. } => "the daemon is not running".to_owned(),
            ClientError::DeadlineExceeded => {
                format!("the daemon did not answer within {} s", deadline.as_secs())
            }
            ClientError::Daemon(error) => format!("the daemon refused: {}", error.code.as_str()),
            _ => "the connection to the daemon failed".to_owned(),
        }
    })
}

fn low_disk_state(readiness: &Readiness) -> &'static str {
    match readiness
        .requirements
        .iter()
        .find(|requirement| requirement.name == DISK_REQUIREMENT)
    {
        None => "unknown",
        Some(requirement) if requirement.status == ReadinessStatusView::Available => "ok",
        Some(_) => "low",
    }
}

fn render_json(snapshot: &Snapshot) -> String {
    let daemon = match &snapshot.observed {
        Ok(_) => json!({ "answering": true, "answered_in_ms": snapshot.answered_in.as_millis() }),
        Err(problem) => json!({ "answering": false, "problem": problem }),
    };
    let (sessions, approvals) = match &snapshot.observed {
        Ok(observed) => (
            observed.sessions.iter().map(session_json).collect(),
            observed.approvals.iter().map(approval_json).collect(),
        ),
        Err(_) => (Vec::new(), Vec::new()),
    };
    let readiness = match &snapshot.readiness {
        None => serde_json::Value::Null,
        Some(Err(problem)) => json!({ "problem": problem }),
        Some(Ok(readiness)) => json!({
            "checked_at": readiness.checked_at,
            "low_disk": low_disk_state(readiness),
            "requirements": readiness.requirements.iter().map(|requirement| json!({
                "name": requirement.name,
                "status": requirement.status.as_str(),
                "detail": requirement.detail,
            })).collect::<Vec<_>>(),
        }),
    };
    let document = json!({
        "generated_at": snapshot.generated_at,
        "daemon": daemon,
        "sessions": sessions,
        "approvals": approvals,
        "readiness": readiness,
    });
    format!("{document}\n")
}

fn session_json(session: &SessionRow) -> serde_json::Value {
    let report = session.report.as_ref();
    json!({
        "id": session.id,
        "project": session.project,
        "state": session.state,
        "agent_state": report.and_then(|report| report.state).map(|state| state.as_str()),
        "label": report.and_then(|report| report.label.as_deref()),
        "doing": report.and_then(|report| report.doing.as_deref()),
        "question": session.question(),
        "reported_at": report.map(|report| report.reported_at),
    })
}

fn approval_json(approval: &WaitingApproval) -> serde_json::Value {
    json!({
        "kind": approval.kind,
        "session": approval.session,
        "project": approval.project,
        "reason": approval.reason,
        "asked_at": approval.asked_at,
    })
}

fn render_page(snapshot: &Snapshot) -> String {
    let now = snapshot.generated_at;
    let mut body = format!(
        "<header><h1>Torlek</h1><p class=\"muted\">Generated {}; refreshes every {REFRESH_SECONDS} s.</p></header>\n",
        escaped(&local_time(now))
    );
    match &snapshot.observed {
        Err(problem) => body.push_str(&format!(
            "<section><p class=\"alert\">Daemon: {}</p></section>\n",
            escaped(problem)
        )),
        Ok(observed) => {
            body.push_str(&format!(
                "<section><p class=\"ok\">Daemon answering ({} ms).</p></section>\n",
                snapshot.answered_in.as_millis()
            ));
            body.push_str(&sessions_section(&observed.sessions, now));
            body.push_str(&approvals_section(&observed.approvals, now));
        }
    }
    if let Some(readiness) = &snapshot.readiness {
        body.push_str(&readiness_section(readiness, now));
    }
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta http-equiv=\"refresh\" content=\"{REFRESH_SECONDS}\">\
         <title>Torlek status</title><style>{STYLE}</style></head>\n<body>\n{body}</body></html>\n"
    )
}

const STYLE: &str = "\
:root{color-scheme:light dark;--fg:#1b1b1b;--bg:#fafafa;--muted:#666;--line:#ddd;--alert:#b3261e;--ok:#1e7a34}\
@media (prefers-color-scheme:dark){:root{--fg:#e8e8e8;--bg:#141414;--muted:#9a9a9a;--line:#333;--alert:#ff8a80;--ok:#7bd88f}}\
body{margin:0 auto;max-width:40rem;padding:1rem;font:16px/1.45 system-ui,sans-serif;color:var(--fg);background:var(--bg)}\
h1{font-size:1.4rem;margin:0}h2{font-size:1.1rem;margin:1.5rem 0 .5rem}\
ul{list-style:none;margin:0;padding:0}li{border-top:1px solid var(--line);padding:.6rem 0;overflow-wrap:anywhere}\
.muted{color:var(--muted);font-size:.9rem;margin:.2rem 0}.alert{color:var(--alert);font-weight:600}.ok{color:var(--ok)}\
.question{border-left:4px solid var(--alert);padding-left:.6rem}";

fn sessions_section(sessions: &[SessionRow], now: i64) -> String {
    let questions = sessions
        .iter()
        .filter(|session| session.question().is_some())
        .count();
    let mut section = format!("<section><h2>Sessions ({})</h2>\n", sessions.len());
    if questions > 0 {
        section.push_str(&format!(
            "<p class=\"alert\">{questions} open question{}</p>\n",
            if questions == 1 { "" } else { "s" }
        ));
    }
    section.push_str("<ul>\n");
    for session in sessions {
        let report = session.report.as_ref();
        let agent_state = report
            .and_then(|report| report.state)
            .map_or(String::new(), |state| format!(", {}", state.as_str()));
        let label = report
            .and_then(|report| report.label.as_deref())
            .map_or(String::new(), |label| {
                format!(" &middot; {}", escaped(label))
            });
        let class = if session.question().is_some() {
            " class=\"question\""
        } else {
            ""
        };
        section.push_str(&format!(
            "<li{class}><strong>{}</strong>{label}<p class=\"muted\">{} &middot; {}{agent_state}</p>",
            escaped(session.project.as_deref().unwrap_or("unknown project")),
            escaped(&session.id),
            escaped(&session.state),
        ));
        if let Some(doing) = report.and_then(|report| report.doing.as_deref()) {
            section.push_str(&format!("<p>{}</p>", escaped(doing)));
        }
        if let Some(question) = session.question() {
            section.push_str(&format!(
                "<p class=\"alert\">Question: {}</p>",
                escaped(question)
            ));
        }
        if let Some(report) = report {
            section.push_str(&format!(
                "<p class=\"muted\">reported {}</p>",
                age(now - report.reported_at)
            ));
        }
        section.push_str("</li>\n");
    }
    section.push_str("</ul></section>\n");
    section
}

fn approvals_section(approvals: &[WaitingApproval], now: i64) -> String {
    let mut section = format!(
        "<section><h2>Waiting approvals ({})</h2>\n<ul>\n",
        approvals.len()
    );
    for approval in approvals {
        section.push_str(&format!(
            "<li><strong>{}</strong><p class=\"muted\">{} &middot; {} &middot; session {} &middot; asked {}</p></li>\n",
            escaped(&approval.reason),
            approval.kind,
            escaped(approval.project.as_deref().unwrap_or("unknown project")),
            escaped(&approval.session),
            age(now - approval.asked_at)
        ));
    }
    section.push_str("</ul></section>\n");
    section
}

fn readiness_section(readiness: &Result<Readiness, String>, now: i64) -> String {
    let readiness = match readiness {
        Ok(readiness) => readiness,
        Err(problem) => {
            return format!(
                "<section><h2>Readiness</h2><p class=\"alert\">{}</p></section>\n",
                escaped(problem)
            );
        }
    };
    let blocking = readiness
        .requirements
        .iter()
        .filter(|requirement| requirement.status.blocks())
        .count();
    let summary = match blocking {
        0 => "<p class=\"ok\">Nothing blocks work.</p>".to_owned(),
        count => format!("<p class=\"alert\">{count} requirement(s) block work.</p>"),
    };
    let disk_class = if low_disk_state(readiness) == "low" {
        "alert"
    } else {
        "muted"
    };
    let mut section = format!(
        "<section><h2>Readiness</h2>{summary}<p class=\"{disk_class}\">Low-disk guard: {}</p>\
         <p class=\"muted\">checked {}</p>\n<ul>\n",
        low_disk_state(readiness),
        age(now - readiness.checked_at)
    );
    for requirement in &readiness.requirements {
        let detail = requirement
            .detail
            .as_deref()
            .map_or(String::new(), |detail| format!(": {}", escaped(detail)));
        section.push_str(&format!(
            "<li>{} &middot; {}{detail}</li>\n",
            escaped(&requirement.name),
            requirement.status.as_str()
        ));
    }
    section.push_str("</ul></section>\n");
    section
}

pub(crate) fn age(seconds: i64) -> String {
    match seconds {
        ..10 => "just now".to_owned(),
        10..120 => format!("{seconds} s ago"),
        120..7200 => format!("{} min ago", seconds / 60),
        7200..172_800 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

fn local_time(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0).map_or_else(
        || timestamp.to_string(),
        |at| {
            at.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S %:z")
                .to_string()
        },
    )
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> Vec<String> {
        vec!["127.0.0.1:8850".to_owned(), "localhost:8850".to_owned()]
    }

    #[test]
    fn a_host_header_in_any_case_is_matched_without_regard_to_case() {
        let head = "GET / HTTP/1.1\r\nhost: LOCALHOST:8850";
        assert_eq!(route(head, &allowed()), Route::Page);
    }

    #[test]
    fn two_host_headers_are_refused() {
        let head = "GET / HTTP/1.1\r\nHost: localhost:8850\r\nHost: evil.example";
        assert!(matches!(
            route(head, &allowed()),
            Route::Refused { status: 400, .. }
        ));
    }

    #[test]
    fn a_host_on_another_port_is_refused() {
        let head = "GET / HTTP/1.1\r\nHost: localhost:9999";
        assert!(matches!(
            route(head, &allowed()),
            Route::Refused { status: 403, .. }
        ));
    }

    #[test]
    fn markup_in_agent_text_is_escaped() {
        assert_eq!(
            escaped("<script>'x'&\"y\""),
            "&lt;script&gt;&#39;x&#39;&amp;&quot;y&quot;"
        );
    }
}
