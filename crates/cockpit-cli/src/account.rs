//! `torlek account sign-in PROVIDER ACCOUNT` (#59, #858): signs a login account in. The daemon
//! prepares a fresh home of its own; this runs the harness's sign-in there, in the owner's
//! terminal, then asks the daemon to keep the login it left. The sign-in gets only what a
//! terminal and a browser sign-in need from the owner's environment: no variable of theirs
//! that names a provider folder or holds a key reaches it.
//!
//! `torlek account grant PROJECT PROVIDER ACCOUNT` lets one project's sessions use that login
//! from its next launch, through a grant `revoke` takes back.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, DecidedThroughView, LoginGrantView, Reply};

/// The owner's variables a sign-in in their terminal keeps: finding programs, drawing the
/// terminal, its language, opening the browser, and reaching the provider through the
/// owner's proxy and trusted CAs.
const KEPT_VARIABLES: [&str; 23] = [
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
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
];

/// What an `account` verb does.
pub(crate) enum Verb {
    SignIn {
        provider: String,
        account: String,
    },
    /// A grant verb: one exchange with the daemon.
    Grants(Command),
}

/// What an `account` verb does, or the usage problem that stops it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<Verb, String> {
    let words = crate::ask::utf8(arguments)?;
    let verb = match words.as_slice() {
        ["sign-in", provider, account] => Verb::SignIn {
            provider: (*provider).to_owned(),
            account: (*account).to_owned(),
        },
        ["grant", project, provider, account] => Verb::Grants(Command::AccountGrant {
            project: (*project).to_owned(),
            provider: (*provider).to_owned(),
            account: (*account).to_owned(),
            through: Some(DecidedThroughView::Cli),
        }),
        ["grants"] => Verb::Grants(Command::AccountGrants { project: None }),
        ["grants", project] => Verb::Grants(Command::AccountGrants {
            project: Some((*project).to_owned()),
        }),
        ["revoke", grant] => Verb::Grants(Command::AccountRevoke {
            grant: grant
                .parse()
                .map_err(|_| format!("{grant} is not a number"))?,
            through: Some(DecidedThroughView::Cli),
        }),
        _ => return Err("unknown account arguments".to_owned()),
    };
    Ok(verb)
}

/// Sends a grant verb's `command` on the caller's socket and prints the answer: refused
/// inside a session, so that no agent grants itself a login.
pub(crate) fn run_grants(command: Command) -> ExitCode {
    let socket = match crate::agent::caller_socket() {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    match crate::agent::exchange(&socket, command) {
        Ok(Reply::AccountGrants { grants }) => {
            for grant in &grants {
                outln!(
                    "grant\t{}\t{}\t{}\t{}",
                    grant.id,
                    grant.project,
                    grant.provider,
                    grant.account
                );
            }
            ExitCode::SUCCESS
        }
        Ok(Reply::AccountGrant { grant }) => {
            out!("{}", grant_detail(&grant));
            ExitCode::SUCCESS
        }
        other => crate::agent::failed(&socket, other),
    }
}

fn grant_detail(grant: &LoginGrantView) -> String {
    let uses = match grant.revoked {
        Some(_) => "no longer uses",
        None => "uses",
    };
    let mut text = format!(
        "login grant {}: project {} {uses} the {} provider's account {} from its next launch\n\
         granted: {}\n",
        grant.id,
        grant.project,
        grant.provider,
        grant.account,
        crate::network::decided("granted", grant.granted)
    );
    match grant.revoked {
        Some(revoked) => text.push_str(&format!(
            "revoked: {}\n",
            crate::network::decided("revoked", revoked)
        )),
        None => text.push_str(&format!(
            "`torlek account revoke {}` takes it back\n",
            grant.id
        )),
    }
    text
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
            return discard(&socket, provider, account);
        }
        Err(error) => {
            eprintln!("torlek: could not run {program}: {error}");
            return discard(&socket, provider, account);
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

/// Removes the failed sign-in's home, which may hold a login it wrote before it failed.
fn discard(socket: &std::path::Path, provider: String, account: String) -> ExitCode {
    match crate::agent::exchange(socket, Command::AccountSignInDiscard { provider, account }) {
        Ok(Reply::AccountSignInDiscarded { .. }) => ExitCode::FAILURE,
        other => {
            crate::agent::failed(socket, other);
            ExitCode::FAILURE
        }
    }
}
