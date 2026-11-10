//! `torlek network`: the owner reads the hosts sandboxed sessions were refused and decides
//! each, on the host-only socket (#674). "allow" lets the host through for the rest of the
//! asking session; "allow --always" for the project, through a grant "revoke" takes back.
//! "grant PROJECT github.com" makes that grant without a request (#1256), the one host
//! offered so, as `project add` does.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{
    Command, DecidedThroughView, NetworkDecisionView, NetworkGrantView, NetworkRequestStateView,
    NetworkRequestView, NetworkSubjectView, Reply,
};

/// The command a `network` verb sends, or the usage problem that stops it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<Command, String> {
    let words = crate::ask::utf8(arguments)?;
    let command = match words.as_slice() {
        ["list"] => Command::NetworkList,
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
        grant.project == project
            && grant.host == GITHUB_HOST
            && grant.port == GITHUB_PORT
            && grant.revoked.is_none()
    })
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
    let socket = match crate::agent::caller_socket() {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    match crate::agent::exchange(&socket, command) {
        Ok(Reply::NetworkRequests { requests }) => {
            for request in &requests {
                outln!("{}", request_line(request));
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

/// `grant  id  project  host:port  request N`, tab-separated; `granted directly` in place of
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
        "network request {}: {} for session {} (project {})\nstate: {}\n",
        request.id,
        request.target,
        subject(&request.subject),
        request.project,
        state_name(request.state)
    );
    let verdict = match request.state {
        NetworkRequestStateView::Asking => None,
        NetworkRequestStateView::AllowedOnce => Some("allowed for the rest of this session"),
        NetworkRequestStateView::AllowedAlways => Some("allowed always for the project"),
        NetworkRequestStateView::Denied => Some("denied"),
    };
    if let (Some(verdict), Some(decision)) = (verdict, request.decision) {
        text.push_str(&format!("decided: {}\n", decided(verdict, decision)));
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

fn subject(subject: &NetworkSubjectView) -> &str {
    match subject {
        NetworkSubjectView::Session { id } => id,
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
