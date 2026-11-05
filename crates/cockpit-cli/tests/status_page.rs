//! `torlek status serve` (#850) driven as a process: a fixture daemon answers its observe reads
//! on the owner's socket, and the tests read the page over loopback HTTP like a phone behind
//! `tailscale serve` would.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::{Arc, Mutex};

use cockpit_protocol::{
    Command, HandshakeReply, Outcome, ProtocolVersion, Reply, Request, Response, ServerFrame,
    Welcome,
};
use serde_json::json;

const QUESTION: &str = "Merge the <b>schema</b> change now?";
const SECRET_COMMAND: &str = "cat ~/.ssh/id_ed25519";
const SECRET_CWD: &str = "/home/owner/private";

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        let home = Home {
            dir: tempfile::tempdir().unwrap(),
        };
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(home.daemon_socket().parent().unwrap())
            .unwrap();
        home
    }

    fn daemon_socket(&self) -> PathBuf {
        self.dir.path().join("run/torlek-dev/daemon.sock")
    }

    fn command(&self, arguments: &[&str]) -> Process {
        let mut command = Process::new(env!("CARGO_BIN_EXE_torlek"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"));
        command
    }

    /// Starts the page on a free port and waits for the line naming it.
    fn serve(&self) -> Server {
        self.serve_with(&[])
    }

    fn serve_with(&self, options: &[&str]) -> Server {
        let mut arguments = vec!["status", "serve", "--port", "0"];
        arguments.extend_from_slice(options);
        let mut child = self
            .command(&arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let address = line
            .split_whitespace()
            .find_map(|word| word.strip_prefix("http://"))
            .and_then(|rest| rest.strip_suffix('/'))
            .unwrap_or_else(|| panic!("no address in {line:?}"))
            .to_owned();
        Server { child, address }
    }
}

struct Server {
    child: Child,
    /// `127.0.0.1:PORT`, as the server printed it.
    address: String,
}

impl Server {
    fn port(&self) -> u16 {
        self.address.rsplit_once(':').unwrap().1.parse().unwrap()
    }

    fn get(&self, path: &str) -> HttpAnswer {
        self.send(&format!(
            "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.address
        ))
    }

    fn send(&self, request: &str) -> HttpAnswer {
        let mut stream = TcpStream::connect(&self.address).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        let (head, body) = answer.split_once("\r\n\r\n").unwrap();
        HttpAnswer {
            status: head.split(' ').nth(1).unwrap().parse().unwrap(),
            head: head.to_owned(),
            body: body.to_owned(),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct HttpAnswer {
    status: u16,
    head: String,
    body: String,
}

/// A daemon on `socket` that answers every connection's handshake, then each observe read
/// from the fixture below; it records every command it receives.
fn fixture_daemon(socket: &Path) -> Arc<Mutex<Vec<Command>>> {
    let listener = UnixListener::bind(socket).unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&received);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = stream.unwrap();
            let recorder = Arc::clone(&recorder);
            std::thread::spawn(move || converse(stream, &recorder));
        }
    });
    received
}

fn converse(stream: std::os::unix::net::UnixStream, recorder: &Mutex<Vec<Command>>) {
    let mut writer = stream.try_clone().unwrap();
    let mut lines = BufReader::new(stream).lines();
    lines.next().expect("a hello").unwrap();
    let welcome = HandshakeReply::Accepted(Welcome {
        protocol_version: ProtocolVersion(1),
        capabilities: Vec::new(),
    });
    send(&mut writer, &serde_json::to_value(welcome).unwrap());
    for line in lines {
        let request: Request = serde_json::from_str(&line.unwrap()).unwrap();
        let outcome = Outcome::Ok(fixture_reply(&request.command));
        recorder.lock().unwrap().push(request.command);
        let frame = ServerFrame::Response(Response {
            id: request.id,
            outcome,
        });
        send(&mut writer, &serde_json::to_value(frame).unwrap());
    }
}

fn send(writer: &mut impl Write, frame: &serde_json::Value) {
    let mut line = serde_json::to_vec(frame).unwrap();
    line.push(b'\n');
    writer.write_all(&line).unwrap();
}

fn shown(text: &str) -> serde_json::Value {
    json!({ "text": text, "has_hidden_characters": false, "non_ascii": [] })
}

/// One session asking a question, one working, an asking run request and guide, and a
/// readiness report whose disk guard holds work.
fn fixture_reply(command: &Command) -> Reply {
    let reply = match command {
        Command::SessionList { ended: false } => json!({
            "kind": "sessions",
            "sessions": [
                session("s-busy", 1),
                session("s-asking", 2),
            ],
        }),
        Command::AgentReports => json!({
            "kind": "agent-reports",
            "reports": [
                { "session": "s-busy", "state": "working", "label": "Tests",
                  "doing": "running the parser tests", "reported_at": 1 },
                { "session": "s-asking", "state": "waiting", "label": "Schema",
                  "needs_input": QUESTION, "question_source": "agent", "reported_at": 1 },
            ],
        }),
        Command::CheckoutList { project: None } => json!({
            "kind": "checkouts",
            "checkouts": [checkout(1, "billing"), checkout(2, "warehouse")],
        }),
        Command::ApprovalQueue => json!({
            "kind": "approval-queue",
            "runs": [{
                "id": 7, "session": "s-busy", "revision": 1, "state": "asking",
                "command": SECRET_COMMAND, "cwd": SECRET_CWD,
                "reason": "check the deploy key", "shown_command": shown(SECRET_COMMAND),
                "shown_cwd": shown(SECRET_CWD), "shown_reason": shown("check the deploy key"),
                "project": "billing", "outside_project": true, "digest": "00",
                "asked_at": 1, "expires_at": 2, "waiting_for_input": false,
            }],
            "guides": [{
                "id": 3, "session": "s-asking", "title": shown("sign in to the registry"),
                "cwd": SECRET_CWD, "shown_cwd": shown(SECRET_CWD), "project": "warehouse",
                "outside_project": false, "state": "asking", "current_step": 0, "steps": [],
                "asked_at": 1, "expires_at": 2,
            }],
        }),
        Command::Readiness { project: None } => json!({
            "kind": "readiness",
            "requirements": [
                { "name": "tmux", "status": "available", "detail": "/usr/bin/tmux 3.5" },
                { "name": "disk", "status": "missing-prerequisite",
                  "detail": "512 MiB free, floor 10240 MiB: new launches and heavy jobs wait" },
            ],
        }),
        other => panic!("the status page sent {other:?}, which is not an observe read"),
    };
    serde_json::from_value(reply).unwrap()
}

fn session(id: &str, checkout: i64) -> serde_json::Value {
    json!({
        "id": id, "agent": "claude", "checkout": checkout, "holds_checkout": true,
        "working_dir": SECRET_CWD, "state": { "kind": "running" },
        "reason": "agent printed: secret terminal line",
        "revision": 1, "created_at": 1, "changed_at": 1,
    })
}

fn checkout(id: i64, project: &str) -> serde_json::Value {
    json!({
        "id": id, "project": project, "kind": { "kind": "main" }, "path": SECRET_CWD,
        "incarnation": 1, "removing": false, "occupant": null, "trust": { "kind": "trusted" },
    })
}

#[test]
fn the_page_lists_each_session_and_flags_its_open_question() {
    let home = Home::new();
    let _daemon = fixture_daemon(&home.daemon_socket());
    let server = home.serve();

    let page = server.get("/");

    assert_eq!(page.status, 200, "{}", page.body);
    assert!(page.body.contains("billing"), "{}", page.body);
    assert!(
        page.body.contains("running the parser tests"),
        "{}",
        page.body
    );
    assert!(
        page.body.contains(
            "<p class=\"alert\">Question: Merge the &lt;b&gt;schema&lt;/b&gt; change now?</p>"
        ),
        "{}",
        page.body
    );
    assert!(
        page.body
            .contains("<meta http-equiv=\"refresh\" content=\"15\">")
    );
}

#[test]
fn the_page_shows_waiting_approvals_by_reason_without_their_command_or_directory() {
    let home = Home::new();
    let _daemon = fixture_daemon(&home.daemon_socket());
    let server = home.serve();

    let page = server.get("/");

    assert!(page.body.contains("check the deploy key"), "{}", page.body);
    assert!(
        page.body.contains("sign in to the registry"),
        "{}",
        page.body
    );
    for secret in [
        SECRET_COMMAND,
        SECRET_CWD,
        "secret terminal line",
        "/usr/bin/tmux",
    ] {
        assert!(
            !page.body.contains(secret),
            "{secret} leaked: {}",
            page.body
        );
    }
}

#[test]
fn the_page_shows_the_low_disk_guard_holding_work() {
    let home = Home::new();
    let _daemon = fixture_daemon(&home.daemon_socket());
    let server = home.serve();

    let page = server.get("/");

    assert!(page.body.contains("Low-disk guard: low"), "{}", page.body);
    assert!(
        page.body.contains("512 MiB free, floor 10240 MiB"),
        "{}",
        page.body
    );
}

#[test]
fn status_json_returns_the_same_sessions_approvals_and_readiness() {
    let home = Home::new();
    let _daemon = fixture_daemon(&home.daemon_socket());
    let server = home.serve();

    let answer = server.get("/status.json");

    assert_eq!(answer.status, 200);
    assert!(answer.head.contains("Content-Type: application/json"));
    let status: serde_json::Value = serde_json::from_str(&answer.body).unwrap();
    assert_eq!(status["daemon"]["answering"], true);
    assert_eq!(
        status["sessions"][0]["id"], "s-asking",
        "questions come first"
    );
    assert_eq!(status["sessions"][0]["project"], "warehouse");
    assert_eq!(status["sessions"][0]["question"], QUESTION);
    assert_eq!(status["approvals"][0]["reason"], "check the deploy key");
    assert_eq!(status["readiness"]["low_disk"], "low");
    assert!(status["generated_at"].as_i64().unwrap() > 0);
    for secret in [SECRET_COMMAND, SECRET_CWD, "secret terminal line"] {
        assert!(
            !answer.body.contains(secret),
            "{secret} leaked: {}",
            answer.body
        );
    }
}

#[test]
fn a_post_is_refused_without_reaching_the_daemon() {
    let home = Home::new();
    let received = fixture_daemon(&home.daemon_socket());
    let server = home.serve();

    let answer = server.send(&format!(
        "POST / HTTP/1.1\r\nHost: {}\r\nContent-Length: 7\r\n\r\napprove",
        server.address
    ));

    assert_eq!(answer.status, 405, "{}", answer.body);
    assert!(answer.head.contains("Allow: GET"));
    assert!(received.lock().unwrap().is_empty());
}

#[test]
fn a_foreign_host_is_refused_without_reaching_the_daemon() {
    let home = Home::new();
    let received = fixture_daemon(&home.daemon_socket());
    let server = home.serve();

    let answer = server.send(&format!(
        "GET /status.json HTTP/1.1\r\nHost: rebound.example:{}\r\n\r\n",
        server.port()
    ));

    assert_eq!(answer.status, 403, "{}", answer.body);
    assert!(received.lock().unwrap().is_empty());
}

#[test]
fn localhost_with_the_port_is_an_accepted_host() {
    let home = Home::new();
    let _daemon = fixture_daemon(&home.daemon_socket());
    let server = home.serve();

    let answer = server.send(&format!(
        "GET / HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
        server.port()
    ));

    assert_eq!(answer.status, 200, "{}", answer.body);
}

#[test]
fn the_tailnet_name_given_with_host_is_accepted_bare_or_with_the_https_port() {
    let home = Home::new();
    let _daemon = fixture_daemon(&home.daemon_socket());
    let server = home.serve_with(&["--host", "box.tailnet.ts.net"]);

    for host in ["Box.Tailnet.ts.net", "box.tailnet.ts.net:443"] {
        let answer = server.send(&format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n"));
        assert_eq!(answer.status, 200, "{host}: {}", answer.body);
    }
}

#[test]
fn a_name_that_only_ends_with_the_tailnet_name_is_refused() {
    let home = Home::new();
    let received = fixture_daemon(&home.daemon_socket());
    let server = home.serve_with(&["--host", "box.tailnet.ts.net"]);

    for host in [
        "rebound.box.tailnet.ts.net",
        "box.tailnet.ts.net:8080",
        "ts.net",
    ] {
        let answer = server.send(&format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n"));
        assert_eq!(answer.status, 403, "{host}: {}", answer.body);
    }
    assert!(received.lock().unwrap().is_empty());
}

#[test]
fn a_wildcard_or_a_port_in_host_is_refused_at_start() {
    let home = Home::new();

    for name in ["*.tailnet.ts.net", "box.tailnet.ts.net:443", ""] {
        let output = home
            .command(&["status", "serve", "--host", name])
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(2), "{name:?}");
        assert!(output.stdout.is_empty(), "{name:?} must not bind");
    }
}

#[test]
fn a_client_that_trickles_its_request_is_dropped_within_five_seconds() {
    let home = Home::new();
    let _daemon = fixture_daemon(&home.daemon_socket());
    let server = home.serve();
    let mut trickler = TcpStream::connect(&server.address).unwrap();
    let trickling = std::thread::spawn(move || {
        for _ in 0..10 {
            if trickler.write_all(b"G").is_err() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(300));

    let started = std::time::Instant::now();
    let answer = server.get("/status.json");

    assert_eq!(answer.status, 200);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "waited {:?} behind a client sending a byte a second",
        started.elapsed()
    );
    trickling.join().unwrap();
}

#[test]
fn a_stopped_daemon_shows_on_the_page_without_its_socket_path() {
    let home = Home::new();
    let server = home.serve();

    let page = server.get("/");

    assert_eq!(page.status, 503);
    assert!(
        page.body.contains("the daemon is not running"),
        "{}",
        page.body
    );
    assert!(!page.body.contains("daemon.sock"), "{}", page.body);
}

#[test]
fn the_server_listens_on_loopback_only() {
    let home = Home::new();
    let server = home.serve();

    assert!(
        server.address.starts_with("127.0.0.1:"),
        "{}",
        server.address
    );
    let listening = listening_addresses(server.port());
    assert_eq!(listening, ["0100007F"], "/proc/net/tcp{{,6}} listeners");
}

/// The local addresses, as `/proc/net/tcp` and `tcp6` print them, of sockets listening on
/// `port`: hexadecimal, `0100007F` for 127.0.0.1.
fn listening_addresses(port: u16) -> Vec<String> {
    const LISTEN: &str = "0A";
    let port = format!("{port:04X}");
    ["/proc/net/tcp", "/proc/net/tcp6"]
        .into_iter()
        .flat_map(|table| {
            std::fs::read_to_string(table)
                .unwrap_or_default()
                .lines()
                .skip(1)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter_map(|row| {
            let columns: Vec<&str> = row.split_whitespace().collect();
            let (address, local_port) = columns.get(1)?.split_once(':')?;
            (local_port == port && columns.get(3) == Some(&LISTEN)).then(|| address.to_owned())
        })
        .collect()
}

#[test]
fn a_session_cannot_start_the_server() {
    let home = Home::new();

    let output = home
        .command(&["status", "serve", "--port", "0"])
        .env("TORLEK_SESSION", "s-agent")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty(), "it must not have bound a port");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("owner verb"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn an_address_to_bind_is_not_an_option() {
    let home = Home::new();

    let output = home
        .command(&["status", "serve", "--bind", "0.0.0.0"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}
