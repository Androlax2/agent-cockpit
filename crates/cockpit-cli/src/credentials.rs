//! `torlek provider credential` and `torlek project token` (#1251): save, show and clear the
//! owner's secret files without `mkdir`, `chmod` or a value typed into the shell's history.
//! The daemon writes them, on the owner's socket alone; nothing here prints a value.

use std::ffi::OsString;
use std::io::{IsTerminal, Read};
use std::process::ExitCode;

use cockpit_protocol::{
    Command, CredentialStateView, CredentialTarget, CredentialView, Reply, SecretText,
};

use crate::daemon::{CONTROL_DEADLINE, request, unexpected, utf8_words};

/// Far above any token; more on standard input is not one.
const MAX_SECRET_BYTES: u64 = 16 * 1024;

enum Verb {
    Set,
    Show,
    Clear,
}

/// `provider credential set|show|clear PROVIDER [--variable NAME]`.
pub(crate) fn provider(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words = utf8_words(arguments)?;
    let (verb, provider, variable) = match words.as_slice() {
        [verb, provider] => (*verb, *provider, None),
        [verb @ ("set" | "clear"), provider, "--variable", variable] => {
            (*verb, *provider, Some((*variable).to_owned()))
        }
        _ => return Err("unknown provider credential arguments".to_owned()),
    };
    let target = CredentialTarget::Provider {
        provider: provider.to_owned(),
        variable,
    };
    let hint = format!("`torlek provider credential set {provider}` saves one");
    run(
        verb_of(verb)?,
        target,
        &format!("{provider}'s credential"),
        &hint,
    )
}

/// `project token set|show|clear PROJECT [--api]`.
pub(crate) fn project_token(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words = utf8_words(arguments)?;
    let (verb, project, api) = match words.as_slice() {
        [verb, project] => (*verb, *project, false),
        [verb, project, "--api"] => (*verb, *project, true),
        _ => return Err("unknown project token arguments".to_owned()),
    };
    let (what, flag) = if api {
        (format!("{project}'s GitHub API token"), " --api")
    } else {
        (format!("{project}'s GitHub token"), "")
    };
    let target = CredentialTarget::GithubToken {
        project: project.to_owned(),
        api,
    };
    let hint = format!("`torlek project token set {project}{flag}` saves one");
    run(verb_of(verb)?, target, &what, &hint)
}

fn verb_of(word: &str) -> Result<Verb, String> {
    match word {
        "set" => Ok(Verb::Set),
        "show" => Ok(Verb::Show),
        "clear" => Ok(Verb::Clear),
        _ => Err(format!("{word}: not set, show or clear")),
    }
}

fn run(verb: Verb, target: CredentialTarget, what: &str, hint: &str) -> Result<ExitCode, String> {
    let is_set = matches!(verb, Verb::Set);
    let command = match verb {
        Verb::Set => Command::CredentialSet {
            target,
            value: read_secret(what)?,
        },
        Verb::Show => Command::CredentialShow { target },
        Verb::Clear => Command::CredentialClear { target },
    };
    Ok(match request(command, CONTROL_DEADLINE) {
        Ok(Reply::Credential { credential }) => {
            if is_set {
                outln!("saved {what}");
            }
            print_view(&credential, hint)
        }
        Ok(Reply::CredentialCleared { removed }) => {
            if removed.is_empty() {
                outln!("no file held {what}: nothing to clear");
            }
            for path in removed {
                outln!("removed {path}");
            }
            ExitCode::SUCCESS
        }
        Ok(other) => unexpected(&other),
        Err(code) => code,
    })
}

/// Each file with its mode, then whether a launch would use it: exit 0 only when it would.
fn print_view(credential: &CredentialView, hint: &str) -> ExitCode {
    for file in &credential.files {
        match file.mode {
            Some(mode) => outln!("{}\tmode {mode:04o}", file.path),
            None => outln!("{}\tabsent", file.path),
        }
    }
    match &credential.state {
        CredentialStateView::Usable { path } => {
            outln!("usable: launches read {path}");
            ExitCode::SUCCESS
        }
        CredentialStateView::Missing => {
            outln!("missing: none of these files exists; {hint}");
            ExitCode::FAILURE
        }
        CredentialStateView::Unusable { reason } => {
            outln!("unusable, so the sessions that need it do not launch: {reason}");
            ExitCode::FAILURE
        }
    }
}

/// The secret, from a prompt that does not echo it on a terminal, else from standard input:
/// never an argument, which would sit in the shell's history and every process listing.
fn read_secret(what: &str) -> Result<SecretText, String> {
    let stdin = std::io::stdin();
    let text = if stdin.is_terminal() {
        crate::terminal::read_hidden_line(&stdin, &format!("{what} (not shown): "))
            .map_err(|error| format!("cannot read {what} from the terminal: {error}"))?
    } else {
        let mut text = String::new();
        stdin
            .lock()
            .take(MAX_SECRET_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|error| format!("cannot read {what} from standard input: {error}"))?;
        if text.len() as u64 > MAX_SECRET_BYTES {
            return Err(format!("standard input is larger than {what}"));
        }
        text
    };
    let value = text.trim();
    if value.is_empty() {
        return Err(format!(
            "{what} is empty: type it, or pipe it on standard input"
        ));
    }
    Ok(SecretText::new(value.to_owned()))
}
