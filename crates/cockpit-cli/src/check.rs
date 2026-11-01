//! `cockpit check`: the project's declared checks and the gate their receipts make (#313).
//! Running a check is the owner's, on the host-only socket: the daemon runs it and records the
//! receipt. A launched session (one with `COCKPIT_LAUNCH_TOKEN`) asks for the gate of the
//! checkout it works in and files claims, on the agent socket; a claim is never a pass.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_protocol::{
    CheckClaimView, CheckOutcomeView, CheckReceiptView, Command, GateOutcomeView, GateView, Reply,
};

use cockpit_client::Connection;

use crate::agent::{DEADLINE, exchange, failed, launch_token};
use crate::daemon::{absolute_dir, utf8_words};
use crate::todo::{Flags, socket};

/// How often `check run` asks whether its run has ended.
const RECEIPT_POLL: Duration = Duration::from_millis(200);

pub(crate) enum Request {
    /// Starts the check and waits for its receipt.
    Run {
        project: String,
        check: String,
        checkout: Option<String>,
    },
    Gate(Command),
    Claim(Command),
}

/// The request for `cockpit check <arguments>`, or the usage problem that stops it.
pub(crate) fn parse(arguments: &[OsString]) -> Result<Request, String> {
    let words = utf8_words(arguments)?;
    let (verb, rest) = words.split_first().ok_or("name a check verb")?;
    match (*verb, rest) {
        ("run", [check, rest @ ..]) => {
            let mut flags = Flags::parse(rest, &["--project", "--checkout"], &[], &[])?;
            flags.no_positional()?;
            Ok(Request::Run {
                project: flags.required("--project", "the project whose check to run")?,
                check: (*check).to_owned(),
                checkout: flags
                    .take("--checkout")
                    .map(|dir| absolute_dir(&dir))
                    .transpose()?,
            })
        }
        ("gate", rest) => {
            let mut flags = Flags::parse(rest, &["--project", "--checkout"], &[], &[])?;
            flags.no_positional()?;
            Ok(Request::Gate(Command::CheckGate {
                launch_token: launch_token(),
                project: flags.take("--project"),
                checkout: flags
                    .take("--checkout")
                    .map(|dir| absolute_dir(&dir))
                    .transpose()?,
            }))
        }
        ("claim", [check, statement]) => Ok(Request::Claim(Command::CheckClaim {
            launch_token: launch_token(),
            check: (*check).to_owned(),
            statement: (*statement).to_owned(),
        })),
        _ => Err(format!("unknown check arguments: {}", words.join(" "))),
    }
}

/// Sends the request and prints the answer. `run` and `gate` exit 0 only on a pass.
pub(crate) fn run(request: Request) -> ExitCode {
    match request {
        Request::Run {
            project,
            check,
            checkout,
        } => run_check(project, check, checkout),
        Request::Gate(command) => {
            let is_session = launch_token().is_some();
            ask(is_session, command, |reply| match reply {
                Reply::CheckGate { gate } => Some(gate_report(gate)),
                _ => None,
            })
        }
        // A claim is a session's statement about its own work.
        Request::Claim(command) => ask(true, command, |reply| match reply {
            Reply::CheckClaimed { claim } => Some((claim_text(claim), ExitCode::SUCCESS)),
            _ => None,
        }),
    }
}

/// Sends `command` and prints what `render` makes of the reply, exiting with its code.
fn ask(
    is_session: bool,
    command: Command,
    render: impl Fn(&Reply) -> Option<(String, ExitCode)>,
) -> ExitCode {
    let socket = match socket(is_session) {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    match exchange(&socket, command) {
        Ok(reply) => match render(&reply) {
            Some((text, code)) => {
                out!("{text}");
                code
            }
            None => failed(&socket, Ok(reply)),
        },
        outcome => failed(&socket, outcome),
    }
}

/// Starts the check on the owner's socket, then asks for its receipt on one connection until
/// the run has ended. A daemon that stops while the check runs records nothing for it (#351);
/// the connection it held fails, and so does the wait, rather than asking a new daemon forever.
fn run_check(project: String, check: String, checkout: Option<String>) -> ExitCode {
    let socket = match socket(false) {
        Ok(socket) => socket,
        Err(code) => return code,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a single-threaded runtime starts");
    let waited = runtime.block_on(async {
        let connection = Connection::open(&socket, DEADLINE).await?;
        let started = connection
            .request(
                Command::CheckRun {
                    project: project.clone(),
                    check,
                    checkout,
                },
                DEADLINE,
            )
            .await?;
        let Reply::CheckRunStarted { run } = started else {
            return Ok(started);
        };
        loop {
            let asked = connection
                .request(
                    Command::CheckReceipt {
                        project: project.clone(),
                        run: run.clone(),
                    },
                    DEADLINE,
                )
                .await?;
            if !matches!(asked, Reply::CheckReceipt { receipt: None }) {
                return Ok(asked);
            }
            tokio::time::sleep(RECEIPT_POLL).await;
        }
    });
    match waited {
        Ok(Reply::CheckReceipt {
            receipt: Some(receipt),
        }) => receipt_report(&receipt),
        outcome => failed(&socket, outcome),
    }
}

fn receipt_report(receipt: &CheckReceiptView) -> ExitCode {
    out!("{}", receipt.output);
    if !receipt.output.is_empty() && !receipt.output.ends_with('\n') {
        out!("\n");
    }
    let exit = receipt
        .exit_code
        .map_or_else(|| "no exit code".to_owned(), |code| format!("exit {code}"));
    outln!(
        "check {} {} ({exit}): receipt {} for run {} on {}{}",
        receipt.check,
        receipt.outcome.as_str(),
        receipt.id,
        receipt.run,
        receipt.head,
        if receipt.dirty.is_some() {
            " with uncommitted changes"
        } else {
            ""
        }
    );
    if receipt.output_bytes > receipt.output.len() as u64 {
        outln!(
            "output: the last {} of {} bytes; the rest is in {}",
            receipt.output.len(),
            receipt.output_bytes,
            receipt.output_references.join(", ")
        );
    }
    if receipt.outcome == CheckOutcomeView::Passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `gate OUTCOME for PROJECT at CHECKOUT (HEAD[, uncommitted changes])`, then one
/// `name  status  [receipt N]  [next: ACTION]` line per check, tab-separated. Exit 0 only when
/// the gate passed.
fn gate_report(gate: &GateView) -> (String, ExitCode) {
    let mut text = format!(
        "gate {} for {} at {} ({}{})\n",
        gate.outcome.as_str(),
        gate.project,
        gate.checkout,
        gate.head,
        if gate.dirty.is_some() {
            ", uncommitted changes"
        } else {
            ""
        }
    );
    for check in &gate.checks {
        text.push_str(&check.name);
        text.push('\t');
        text.push_str(&check.status.summary());
        if let Some(receipt) = check.receipt {
            text.push_str(&format!("\treceipt {receipt}"));
        }
        if let Some(action) = &check.next_action {
            text.push_str(&format!("\tnext: {action}"));
        }
        text.push('\n');
    }
    let code = if gate.outcome == GateOutcomeView::Passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    };
    (text, code)
}

fn claim_text(claim: &CheckClaimView) -> String {
    format!(
        "claim {} recorded for {} in {}: unverified until the owner runs it\n",
        claim.id, claim.check, claim.project
    )
}
