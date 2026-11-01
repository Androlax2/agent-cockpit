//! `cockpit --skill`, `cockpit docs [TOPIC]` and `cockpit brief` (#49). The running daemon
//! serves the instructions, so what an agent reads always matches the release that launched
//! it. A session asks on the agent socket, everyone else on the host-only one.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_protocol::{Command, Reply};

use crate::agent::{exchange, failed, launch_token};
use crate::todo::socket;

/// `cockpit --skill`: every topic at once, for an agent whose harness offers nothing else.
pub(crate) fn skill(arguments: &[OsString]) -> Result<ExitCode, String> {
    if let Some(argument) = arguments.first() {
        return Err(format!(
            "--skill takes no argument, not {}",
            argument.to_string_lossy()
        ));
    }
    Ok(read(None))
}

/// `cockpit docs`: the topics; `cockpit docs TOPIC`: one of them.
pub(crate) fn docs(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words = crate::daemon::utf8_words(arguments)?;
    match words[..] {
        [] => Ok(catalog()),
        [topic] => Ok(read(Some(topic.to_owned()))),
        _ => Err("docs takes at most one topic".to_owned()),
    }
}

/// `cockpit brief [--measure]`: this session's launch brief, or with `--measure` how much
/// context Cockpit adds.
pub(crate) fn brief(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words = crate::daemon::utf8_words(arguments)?;
    let measures = match words[..] {
        [] => false,
        ["--measure"] => true,
        _ => return Err("brief takes no argument but --measure".to_owned()),
    };
    let token = launch_token();
    let socket = match socket(token.is_some()) {
        Ok(socket) => socket,
        Err(code) => return Ok(code),
    };
    Ok(
        match exchange(
            &socket,
            Command::AgentBrief {
                launch_token: token,
            },
        ) {
            Ok(Reply::AgentBrief {
                brief,
                manifest,
                budget,
            }) => {
                if measures {
                    outln!(
                        "always loaded: {} bytes (brief {}, skill catalog entry {})",
                        budget.brief_bytes + budget.catalog_bytes,
                        budget.brief_bytes,
                        budget.catalog_bytes
                    );
                    outln!(
                        "on demand: {} bytes (the whole manual)",
                        budget.on_demand_bytes
                    );
                    outln!("instructions manifest: {manifest}");
                } else {
                    out!("{brief}");
                }
                ExitCode::SUCCESS
            }
            other => failed(&socket, other),
        },
    )
}

fn catalog() -> ExitCode {
    let socket = match socket(launch_token().is_some()) {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    match exchange(&socket, Command::InstructionCatalog) {
        Ok(Reply::InstructionCatalog {
            version,
            manifest,
            topics,
        }) => {
            let width = topics
                .iter()
                .map(|topic| topic.name.len())
                .max()
                .unwrap_or(0);
            for topic in &topics {
                outln!("{:width$}  {}", topic.name, topic.summary);
            }
            outln!(
                "\nRead one with `cockpit docs TOPIC`; `cockpit --skill` prints them all. \
                 Cockpit {version}, instructions {manifest}."
            );
            ExitCode::SUCCESS
        }
        other => failed(&socket, other),
    }
}

fn read(topic: Option<String>) -> ExitCode {
    let socket = match socket(launch_token().is_some()) {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    match exchange(&socket, Command::InstructionsRead { topic }) {
        Ok(Reply::Instructions { text, .. }) => {
            out!("{text}");
            ExitCode::SUCCESS
        }
        other => failed(&socket, other),
    }
}
