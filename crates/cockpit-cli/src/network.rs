//! `torlek network`: the owner reads the hosts sandboxed sessions were refused and decides
//! each, on the host-only socket (#674). "allow" lets the host through for the rest of the
//! asking session; "allow --always" for the project, through a grant "revoke" takes back.
//! "grant PROJECT github.com" makes that grant without a request (#1256), the one host
//! offered so, as `project add` does.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{
    Command, DecidedThroughView, NetworkDecisionView, NetworkGrantView, NetworkRequestStateView,
    NetworkRequestView, NetworkSubjectView, NetworkTargetView, Reply,
};

/// The command a `network` verb sends, or the usage problem that stops it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<Command, String> {
    let words = crate::ask::utf8(arguments)?;
    let command = match words.as_slice() {
        ["list"] => Command::NetworkList,
        ["list", "--all"] => Command::NetworkDecided,
        ["allow", id] => allow(id, false)?,
        ["allow", id, "--always"] | ["allow", "--always", id] => allow(id, true)?,
        ["deny", id] => Command::NetworkDeny {
            request: id_of(id)?,
            through: Some(DecidedThroughView::Cli),
        },
        ["grants"] => Command::NetworkGrants { project: None },
        ["grants", project] => Command::NetworkGrants {
            project: Some((*project).to_owned()),
        },
        ["grant", project, host] => grant_github(project, host)?,
        ["revoke", grant] => Command::NetworkRevoke {
            grant: id_of(grant)?,
            through: Some(DecidedThroughView::Cli),
        },
        _ => return Err("unknown network arguments".to_owned()),
    };
    Ok(command)
}

fn allow(id: &str, always: bool) -> Result<Command, String> {
    Ok(Command::NetworkAllow {
        request: id_of(id)?,
        always,
        through: Some(DecidedThroughView::Cli),
    })
}

/// The daemon grants this host alone without a request (`cockpit_sandbox::GITHUB_HOST`).
pub(crate) const GITHUB_HOST: &str = "github.com";
const GITHUB_PORT: u16 = 443;

/// Only github.com is granted without a request: any other host is decided on the request a
/// session's refusal filed, so the owner sees who asked for it.
fn grant_github(project: &str, host: &str) -> Result<Command, String> {
    if host != GITHUB_HOST {
        return Err(format!(
            "only {} is granted without a request, not {host}; allow another host from its \
             request with `torlek network allow ID --always`",
            GITHUB_HOST
        ));
    }
    Ok(github_grant_command(project))
}

/// The command that grants `project` github.com always, from this client.
pub(crate) fn github_grant_command(project: &str) -> Command {
    Command::NetworkGrantGithub {
        project: project.to_owned(),
        through: Some(DecidedThroughView::Cli),
    }
}

/// Whether `project` holds an active github.com grant among `grants`.
pub(crate) fn holds_github_grant(grants: &[NetworkGrantView], project: &str) -> bool {
    grants.iter().any(|grant| {
        let is_github = matches!(
            &grant.allows,
            NetworkTargetView::Host { host, port } if host == GITHUB_HOST && *port == GITHUB_PORT
        );
        grant.project == project && is_github && grant.revoked.is_none()
    })
}

/// Whether the owner's configuration may sandbox sessions, so a network grant matters: not
/// under `agents.sandbox = "off"`. A configuration that cannot be read counts as may: `init`
/// names that error, and an offer shown in vain beats one hidden.
pub(crate) fn sessions_may_be_sandboxed() -> bool {
    let config = cockpit_config::load(&crate::profile::environment(), None, &[]);
    may_be_sandboxed(&config.map_err(|error| error.to_string()))
}

pub(crate) fn may_be_sandboxed(config: &Result<cockpit_config::EffectiveConfig, String>) -> bool {
    !matches!(config, Ok(config) if config.agents_sandbox.value == cockpit_config::AgentSandbox::Off)
}

/// The offer `project add` and `init` make to a project without the github.com grant.
pub(crate) fn github_grant_offer(project: &str) -> String {
    format!(
        "{project}: its sandboxed sessions reach github.com only after you allow a network \
         request, then a relaunch; `{} network grant {project} github.com` allows it always, \
         so fetches work from the first launch (pushes stay refused)",
        crate::profile::command()
    )
}

fn id_of(text: &str) -> Result<i64, String> {
    text.parse().map_err(|_| format!("{text} is not a number"))
}

/// Sends `command` on the caller's socket and prints the answer: the owner's verbs, refused
/// inside a session so that no agent decides its own request.
pub(crate) fn run(command: Command) -> ExitCode {
    let is_decided_list = command == Command::NetworkDecided;
    let socket = match crate::agent::caller_socket() {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    match crate::agent::exchange(&socket, command) {
        Ok(Reply::NetworkRequests { requests }) => {
            // Only the decided list carries a wait; an asking request has none yet.
            let line = if is_decided_list {
                decided_request_line
            } else {
                request_line
            };
            for request in &requests {
                outln!("{}", line(request));
                out!("{}", reason_lines(request));
            }
            ExitCode::SUCCESS
        }
        Ok(Reply::NetworkRequest { request }) => {
            out!("{}", request_detail(&request));
            ExitCode::SUCCESS
        }
        Ok(Reply::NetworkGrants { grants }) => {
            for grant in &grants {
                outln!("{}", grant_line(grant));
            }
            ExitCode::SUCCESS
        }
        Ok(Reply::NetworkGrant { grant }) => {
            out!("{}", grant_detail(&grant));
            ExitCode::SUCCESS
        }
        other => crate::agent::failed(&socket, other),
    }
}

/// `network  id  state  session  project  host:port`, tab-separated, as `approval list`
/// shows it beside run requests.
pub(crate) fn request_line(request: &NetworkRequestView) -> String {
    format!(
        "network\t{}\t{}\t{}\t{}\t{}",
        request.id,
        state_name(request.state),
        subject(&request.subject),
        request.project,
        request.target
    )
}

/// `network  id  state  session  project  host:port  waited`, tab-separated: the state is the
/// owner's decision, and `waited` the seconds from the ask to it.
fn decided_request_line(request: &NetworkRequestView) -> String {
    let waited = request.decision.map_or_else(
        || "-".to_owned(),
        |decision| crate::approvals::seconds_between(request.asked_at, decision.at),
    );
    format!("{}\t{waited}", request_line(request))
}

/// The agent's reason for a service it asked for ahead, indented under its request's line
/// with the display rules' warnings, so the owner reads it before deciding; empty otherwise.
pub(crate) fn reason_lines(request: &NetworkRequestView) -> String {
    let Some(reason) = &request.shown_reason else {
        return String::new();
    };
    format!(
        "  reason: {}\n{}",
        reason.text,
        crate::approvals::warnings(reason)
    )
}

/// `grant  id  project  host:port|record  request N`, tab-separated; `granted directly` in place of
/// the request for a grant made without one.
fn grant_line(grant: &NetworkGrantView) -> String {
    format!(
        "grant\t{}\t{}\t{}\t{}",
        grant.id,
        grant.project,
        grant.target,
        grant_origin(grant)
    )
}

fn grant_origin(grant: &NetworkGrantView) -> String {
    match grant.request {
        Some(request) => format!("request {request}"),
        None => "granted directly".to_owned(),
    }
}

fn request_detail(request: &NetworkRequestView) -> String {
    let mut text = format!(
        "network request {}: {} for {} (project {})\nstate: {}\n",
        request.id,
        request.target,
        requester(&request.subject),
        request.project,
        state_name(request.state)
    );
    if let Some(reason) = &request.shown_reason {
        text.push_str(&format!("reason: {}\n", reason.text));
        text.push_str(&crate::approvals::warnings(reason));
    }
    let verdict = match request.state {
        NetworkRequestStateView::Asking => None,
        NetworkRequestStateView::AllowedOnce => Some("allowed for the rest of this session"),
        NetworkRequestStateView::AllowedAlways => Some("allowed always for the project"),
        NetworkRequestStateView::Denied => Some("denied"),
    };
    if let (Some(verdict), Some(decision)) = (verdict, request.decision) {
        text.push_str(&format!("decided: {}\n", decided(verdict, decision)));
    }
    if matches!(request.asks, NetworkTargetView::Service { .. }) {
        text.push_str("the service sees a local client: the host's own, not the network\n");
    }
    if let Some(grant) = request.grant {
        text.push_str(&format!(
            "grant {grant}; `torlek network revoke {grant}` takes it back\n"
        ));
    }
    text
}

fn grant_detail(grant: &NetworkGrantView) -> String {
    let mut text = format!(
        "network grant {}: {} for project {} ({})\ngranted: {}\n",
        grant.id,
        grant.target,
        grant.project,
        grant_origin(grant),
        decided("allowed always", grant.granted)
    );
    if let Some(revoked) = grant.revoked {
        text.push_str(&format!("revoked: {}\n", decided("revoked", revoked)));
    }
    text
}

/// `<verdict> by the owner through the CLI at <local time>` (#657).
pub(crate) fn decided(verdict: &str, decision: NetworkDecisionView) -> String {
    let through = match decision.through {
        Some(DecidedThroughView::Tui) => " through the TUI",
        Some(DecidedThroughView::Cli) => " through the CLI",
        None => "",
    };
    let unnamed = if decision.through.is_none() {
        " (the client did not name itself)"
    } else {
        ""
    };
    format!(
        "{verdict} by the owner{through} at {}{unnamed}",
        crate::approvals::local_time(decision.at)
    )
}

/// `session ID`, `check NAME (run RUN)` for a check run's request (#352), or
/// `containers of PROJECT` for a project's containers' (#390).
fn requester(subject: &NetworkSubjectView) -> String {
    match subject {
        NetworkSubjectView::Session { id } => format!("session {id}"),
        NetworkSubjectView::Check { .. } | NetworkSubjectView::Containers { .. } => {
            self::subject(subject)
        }
    }
}

/// The session's id, `check NAME (run RUN)` for a check run's request (#352), or
/// `containers of PROJECT` for a project's containers' (#390).
fn subject(subject: &NetworkSubjectView) -> String {
    match subject {
        NetworkSubjectView::Session { id } => id.clone(),
        NetworkSubjectView::Check { run, check } => format!("check {check} (run {run})"),
        NetworkSubjectView::Containers { project } => format!("containers of {project}"),
    }
}

fn state_name(state: NetworkRequestStateView) -> &'static str {
    match state {
        NetworkRequestStateView::Asking => "asking",
        NetworkRequestStateView::AllowedOnce => "allowed-once",
        NetworkRequestStateView::AllowedAlways => "allowed-always",
        NetworkRequestStateView::Denied => "denied",
    }
}
