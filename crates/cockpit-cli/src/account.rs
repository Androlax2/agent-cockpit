//! `torlek account sign-in PROVIDER ACCOUNT` (#59, #858): signs a login account in. The daemon
//! prepares a fresh home of its own; this runs the harness's sign-in there, in the owner's
//! terminal, then asks the daemon to keep the login it left. The sign-in gets only what a
//! terminal and a browser sign-in need from the owner's environment: no variable of theirs
//! that names a provider folder or holds a key reaches it.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, Reply};

/// The owner's variables a sign-in in their terminal keeps: finding programs, drawing the
/// terminal, its language, and opening the browser.
const KEPT_VARIABLES: [&str; 12] = [
    "PATH",
    "TERM",
    "COLORTERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "BROWSER",
];

/// The provider and account `account sign-in` names, or the usage problem that stops it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<(String, String), String> {
    let words = crate::ask::utf8(arguments)?;
    match words.as_slice() {
        ["sign-in", provider, account] => Ok(((*provider).to_owned(), (*account).to_owned())),
        _ => Err("unknown account arguments".to_owned()),
    }
}

pub(crate) fn sign_in(provider: String, account: String) -> ExitCode {
    let socket = match crate::agent::caller_socket() {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    let begin = Command::AccountSignIn {
        provider: provider.clone(),
        account: account.clone(),
    };
    let (program, arguments, home, steps) = match crate::agent::exchange(&socket, begin) {
        Ok(Reply::AccountSignInPlan {
            program,
            arguments,
            home,
            steps,
        }) => (program, arguments, home, steps),
        other => return crate::agent::failed(&socket, other),
    };
    outln!(
        "Signing in the {provider} provider's account {account}, in a fresh folder of Torlek's: {steps}."
    );
    let status = std::process::Command::new(&program)
        .args(&arguments)
        .env_clear()
        .envs(
            KEPT_VARIABLES
                .iter()
                .filter_map(|name| std::env::var_os(name).map(|value| (name, value))),
        )
        .env("HOME", &home)
        .current_dir(&home)
        .status();
    let again = format!("run `torlek account sign-in {provider} {account}` again");
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            let ended = match status.code() {
                Some(code) => format!("exited with status {code}"),
                None => "was stopped by a signal".to_owned(),
            };
            eprintln!("torlek: the {provider} sign-in {ended}; nothing was kept: {again}");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("torlek: could not run {program}: {error}");
            return ExitCode::FAILURE;
        }
    }
    let finish = Command::AccountSignedIn { provider, account };
    match crate::agent::exchange(&socket, finish) {
        Ok(Reply::AccountSignedIn {
            provider,
            account,
            login,
        }) => {
            outln!(
                "The {provider} provider's account {account} is signed in; its login is kept in \
                 {login}, which no agent reads. No session uses a login account until a project \
                 is granted it."
            );
            ExitCode::SUCCESS
        }
        other => crate::agent::failed(&socket, other),
    }
}
