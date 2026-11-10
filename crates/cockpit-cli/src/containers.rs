//! `torlek containers record PROJECT`: the owner records the engine that answers as the
//! project's own rootless engine (#390), on the host-only socket. Recording again replaces it,
//! which is how a reinstalled engine is accepted.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, DecidedThroughView, Reply};

/// The command a `containers` verb sends, or the usage problem that stops it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<Command, String> {
    let words = crate::ask::utf8(arguments)?;
    match words.as_slice() {
        ["record", project] => Ok(Command::ContainersRecord {
            project: (*project).to_owned(),
            through: Some(DecidedThroughView::Cli),
        }),
        _ => Err("unknown containers arguments".to_owned()),
    }
}

/// Sends `command` on the caller's socket and prints the answer; refused inside a session, so
/// no agent picks the engine its project uses.
pub(crate) fn run(command: Command) -> ExitCode {
    let socket = match crate::agent::caller_socket() {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    match crate::agent::exchange(&socket, command) {
        Ok(Reply::ContainerEngineRecorded {
            project,
            user,
            engine_id,
            replaced,
        }) => {
            outln!("{project}: recorded engine {engine_id}, run by {user}");
            if let Some(replaced) = replaced {
                outln!("it replaces engine {replaced}");
            }
            ExitCode::SUCCESS
        }
        other => crate::agent::failed(&socket, other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<OsString> {
        text.split_whitespace().map(OsString::from).collect()
    }

    #[test]
    fn record_names_the_project_and_this_client() {
        assert_eq!(
            parse(&words("record shop")),
            Ok(Command::ContainersRecord {
                project: "shop".to_owned(),
                through: Some(DecidedThroughView::Cli),
            })
        );
    }

    #[test]
    fn anything_else_is_a_usage_problem() {
        assert!(parse(&words("record")).is_err());
        assert!(parse(&words("record shop extra")).is_err());
        assert!(parse(&words("list")).is_err());
    }
}
