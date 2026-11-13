//! `torlek setup engine-masks [--uninstall]`: installs Torlek's masks helper (#390) on the
//! install script's path, which never calls sudo (Theo, 2026-11-13). The daemon names the
//! root step and the `torlek-sandbox` it runs; this terminal runs that step once with sudo,
//! the owner typing the password. A package installs the helper itself, and the step refuses
//! there. Refused inside a session: no agent runs a root step.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, Reply};

/// Whether `setup` asks to remove the helper, or the usage problem that stops it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<bool, String> {
    let words = crate::ask::utf8(arguments)?;
    match words.as_slice() {
        ["engine-masks"] => Ok(false),
        ["engine-masks", "--uninstall"] => Ok(true),
        _ => Err("unknown setup arguments".to_owned()),
    }
}

pub(crate) fn run(uninstall: bool) -> ExitCode {
    let socket = match crate::agent::caller_socket() {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    let (script, helper) =
        match crate::agent::exchange(&socket, Command::MasksHelperStep { uninstall }) {
            Ok(Reply::MasksHelperStep { script, helper }) => (script, helper),
            other => return crate::agent::failed(&socket, other),
        };
    if uninstall {
        outln!(
            "Removing the masks helper `torlek setup engine-masks` installed: sudo stops \
             torlek-engine-masks.socket and removes its units from /etc/systemd/system and \
             /usr/local/lib/torlek/bin/torlek-sandbox. sudo asks for your password."
        );
    } else {
        outln!(
            "Installing Torlek's masks helper: sudo copies {helper} to \
             /usr/local/lib/torlek/bin/torlek-sandbox, owned by root, writes \
             torlek-engine-masks.socket and torlek-engine-masks@.service in \
             /etc/systemd/system and starts the socket. sudo asks for your password."
        );
    }
    let status = std::process::Command::new("sudo")
        .args(["sh", "-c", &script, "torlek-setup", &helper])
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

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<OsString> {
        text.split_whitespace().map(OsString::from).collect()
    }

    #[test]
    fn engine_masks_installs_and_its_uninstall_removes() {
        assert_eq!(parse(&words("engine-masks")), Ok(false));
        assert_eq!(parse(&words("engine-masks --uninstall")), Ok(true));
    }

    #[test]
    fn anything_else_is_a_usage_problem() {
        assert!(parse(&words("")).is_err());
        assert!(parse(&words("engine-masks --force")).is_err());
        assert!(parse(&words("engines")).is_err());
    }
}
