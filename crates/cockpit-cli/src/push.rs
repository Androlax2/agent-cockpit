//! `torlek push [--force]` (#155): the session's agent has the daemon push its own checkout's
//! branch. The project's push token stays with the daemon; the agent never holds it.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_protocol::{Command, Reply};

use crate::ask::Agent;
use crate::daemon::utf8_words;

/// Long enough for the daemon's remote reads and its push, each bounded by its own deadline.
const PUSH_DEADLINE: Duration = Duration::from_secs(15 * 60);

/// `torlek push [--force]`: pushes the checkout's current branch, which its `push-branch`
/// grant must cover; `--force` is a lease on the launch branch.
pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let force = match utf8_words(arguments)?.as_slice() {
        [] => false,
        ["--force"] => true,
        _ => return Err("push takes only `--force`".to_owned()),
    };
    let agent = match Agent::connect() {
        Ok(agent) => agent,
        Err(code) => return Ok(code),
    };
    Ok(
        match agent.request_within(
            Command::AgentPush {
                launch_token: crate::agent::launch_token(),
                force,
            },
            PUSH_DEADLINE,
        ) {
            Ok(Reply::Pushed {
                branch, remote_url, ..
            }) => {
                outln!("pushed {branch} to {remote_url}");
                ExitCode::SUCCESS
            }
            other => agent.failed(other),
        },
    )
}
