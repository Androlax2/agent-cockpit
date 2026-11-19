//! `torlek setup engine-masks|root-run [--uninstall]`: installs one of Torlek's root helpers on
//! the install script's path, which never calls sudo: the masks helper (#390, Theo
//! 2026-11-13) or the root-run helper (#1387, Theo 2026-11-19). The daemon names the root step
//! and the `torlek-sandbox` it runs; this terminal runs that step once with sudo, the owner
//! typing the password. A package installs each helper itself, and the step refuses there.
//! Refused inside a session: no agent runs a root step.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, Reply};

/// The helper a setup step installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Helper {
    EngineMasks,
    RootRun,
}

/// The helper `setup` names and whether it asks to remove it, or the usage problem that stops
/// it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<(Helper, bool), String> {
    let words = crate::ask::utf8(arguments)?;
    let (helper, rest) = match words.as_slice() {
        ["engine-masks", rest @ ..] => (Helper::EngineMasks, rest),
        ["root-run", rest @ ..] => (Helper::RootRun, rest),
        _ => return Err("unknown setup arguments".to_owned()),
    };
    match rest {
        [] => Ok((helper, false)),
        ["--uninstall"] => Ok((helper, true)),
        _ => Err("unknown setup arguments".to_owned()),
    }
}

pub(crate) fn run(helper: Helper, uninstall: bool) -> ExitCode {
    let socket = match crate::agent::caller_socket() {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    let (script, program) = match step(&socket, helper, uninstall) {
        Ok(step) => step,
        Err(code) => return code,
    };
    outln!("{}", explanation(helper, uninstall, &program));
    let status = std::process::Command::new("sudo")
        .args(["sh", "-c", &script, "torlek-setup", &program])
        .status();
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("torlek: the root step failed ({status}); nothing more was run");
            ExitCode::from(1)
        }
        Err(error) => {
            eprintln!("torlek: cannot run sudo: {error}");
            ExitCode::from(1)
        }
    }
}

/// The daemon's root step for `helper` and the `torlek-sandbox` it copies.
fn step(
    socket: &std::path::Path,
    helper: Helper,
    uninstall: bool,
) -> Result<(String, String), ExitCode> {
    match helper {
        Helper::EngineMasks => {
            match crate::agent::exchange(socket, Command::MasksHelperStep { uninstall }) {
                Ok(Reply::MasksHelperStep { script, helper }) => Ok((script, helper)),
                other => Err(crate::agent::failed(socket, other)),
            }
        }
        Helper::RootRun => {
            match crate::agent::exchange(socket, Command::RootRunHelperStep { uninstall }) {
                Ok(Reply::RootRunHelperStep { script, helper }) => Ok((script, helper)),
                other => Err(crate::agent::failed(socket, other)),
            }
        }
    }
}

/// What the root step does, said before sudo asks for the password.
fn explanation(helper: Helper, uninstall: bool, program: &str) -> String {
    match (helper, uninstall) {
        (Helper::EngineMasks, true) => "Removing the masks helper `torlek setup engine-masks` \
             installed: sudo stops torlek-engine-masks.socket and removes its units from \
             /etc/systemd/system and /usr/local/lib/torlek/bin/torlek-sandbox. sudo asks for \
             your password."
            .to_owned(),
        (Helper::EngineMasks, false) => format!(
            "Installing Torlek's masks helper: sudo copies {program} to \
             /usr/local/lib/torlek/bin/torlek-sandbox, owned by root, writes \
             torlek-engine-masks.socket and torlek-engine-masks@.service in \
             /etc/systemd/system and starts the socket. sudo asks for your password."
        ),
        (Helper::RootRun, true) => "Removing the root-run helper `torlek setup root-run` \
             installed: sudo removes /usr/local/lib/torlek/root-run/torlek-sandbox. sudo asks \
             for your password."
            .to_owned(),
        (Helper::RootRun, false) => format!(
            "Installing Torlek's root-run helper: sudo copies {program} to \
             /usr/local/lib/torlek/root-run/torlek-sandbox, owned by root and writable by root \
             alone. It installs no unit and no sudoers rule: sudo in an approved run asks for \
             your password each time. sudo asks for your password now."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<OsString> {
        text.split_whitespace().map(OsString::from).collect()
    }

    #[test]
    fn each_helper_installs_and_its_uninstall_removes() {
        for (named, helper) in [
            ("engine-masks", Helper::EngineMasks),
            ("root-run", Helper::RootRun),
        ] {
            assert_eq!(parse(&words(named)), Ok((helper, false)));
            assert_eq!(
                parse(&words(&format!("{named} --uninstall"))),
                Ok((helper, true))
            );
        }
    }

    #[test]
    fn anything_else_is_a_usage_problem() {
        for text in [
            "",
            "engine-masks --force",
            "root-run --force",
            "root-run --uninstall --uninstall",
            "engines",
        ] {
            assert!(parse(&words(text)).is_err(), "{text}");
        }
    }
}
