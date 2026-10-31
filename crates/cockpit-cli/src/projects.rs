//! `cockpit project` and `cockpit checkout`: verbs served by the daemon over the socket.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_protocol::{CheckoutKindView, CheckoutView, Command, KeptReason, Reply, TrustView};

use crate::daemon::{
    BULK_DEADLINE, CONTROL_DEADLINE, absolute_dir, request, unexpected, utf8_words,
};

/// A verb's command, or the usage problem that stops it.
pub fn parse(verb: &str, arguments: &[OsString]) -> Result<(Command, Duration), String> {
    let words = utf8_words(arguments)?;
    let control = |command| Ok((command, CONTROL_DEADLINE));
    let bulk = |command| Ok((command, BULK_DEADLINE));
    match (verb, words.as_slice()) {
        ("project", ["add", dir]) => bulk(project_add(dir, None)?),
        ("project", ["add", dir, "--name", name]) => bulk(project_add(dir, Some(name))?),
        ("project", ["list"]) => control(Command::ProjectList),
        ("checkout", ["list"]) => control(Command::CheckoutList { project: None }),
        ("checkout", ["list", project]) => control(Command::CheckoutList {
            project: Some((*project).to_owned()),
        }),
        ("checkout", ["clone", project, branch]) => bulk(Command::CheckoutClone {
            project: (*project).to_owned(),
            branch: (*branch).to_owned(),
        }),
        ("checkout", ["review", project, number]) => bulk(Command::CheckoutReview {
            project: (*project).to_owned(),
            pull_request: number
                .parse()
                .map_err(|_| format!("{number} is not a pull request number"))?,
        }),
        ("checkout", ["claim", checkout, holder]) => control(Command::CheckoutClaim {
            checkout: checkout_id(checkout)?,
            holder: (*holder).to_owned(),
        }),
        ("checkout", ["release", checkout, holder]) => control(Command::CheckoutRelease {
            checkout: checkout_id(checkout)?,
            holder: (*holder).to_owned(),
        }),
        ("checkout", ["sweep"]) => bulk(Command::CheckoutSweep { dry_run: false }),
        ("checkout", ["sweep", "--dry-run"]) => bulk(Command::CheckoutSweep { dry_run: true }),
        ("checkout", ["push", checkout]) => bulk(Command::CheckoutPush {
            checkout: checkout_id(checkout)?,
        }),
        _ => Err(format!("unknown {verb} arguments")),
    }
}

fn project_add(dir: &str, name: Option<&str>) -> Result<Command, String> {
    Ok(Command::ProjectAdd {
        path: absolute_dir(dir)?,
        name: name.map(str::to_owned),
    })
}

fn checkout_id(text: &str) -> Result<i64, String> {
    text.parse()
        .map_err(|_| format!("{text} is not a checkout id"))
}

/// Sends `command` to the daemon and prints its answer; exit 1 when the daemon refuses or
/// cannot be reached.
pub fn run(command: Command, deadline: Duration) -> ExitCode {
    let reply = match request(command, deadline) {
        Ok(reply) => reply,
        Err(code) => return code,
    };
    match render(&reply) {
        Some(text) => {
            out!("{text}");
            ExitCode::SUCCESS
        }
        None => unexpected(&reply),
    }
}

/// The reply as printed, or `None` when it is not one a project verb gets.
fn render(reply: &Reply) -> Option<String> {
    let text = match reply {
        Reply::ProjectAdded { main, .. } => checkout_line(main),
        Reply::Projects { projects } => projects
            .iter()
            .map(|project| format!("{}\t{}\n", project.name, project.root))
            .collect(),
        Reply::Checkouts { checkouts } => checkouts.iter().map(checkout_line).collect(),
        Reply::Checkout { checkout } => checkout_line(checkout),
        Reply::Released { checkout } => format!("released\t{checkout}\n"),
        Reply::Swept {
            dry_run,
            removed,
            kept,
        } => {
            let verb = if *dry_run { "would remove" } else { "removed" };
            let removed_lines = removed
                .iter()
                .map(|checkout| format!("{verb}\t{}\t{}\n", checkout.id, checkout.path));
            let kept_lines = kept.iter().map(|kept| {
                format!(
                    "kept\t{}\t{}\t{}\n",
                    kept.checkout.id,
                    kept.checkout.path,
                    kept_reason(&kept.reason)
                )
            });
            removed_lines.chain(kept_lines).collect()
        }
        Reply::Pushed {
            branch, remote_url, ..
        } => format!("pushed {branch} to {remote_url}\n"),
        _ => return None,
    };
    Some(text)
}

/// `id  project  kind  incarnation  occupant  trust  path`, tab-separated.
fn checkout_line(checkout: &CheckoutView) -> String {
    let kind = match &checkout.kind {
        CheckoutKindView::Main => "main".to_owned(),
        CheckoutKindView::Parallel { branch } => format!("parallel:{branch}"),
        CheckoutKindView::Review { pull_request } => format!("review:#{pull_request}"),
    };
    let trust = match &checkout.trust {
        TrustView::Trusted => "trusted".to_owned(),
        TrustView::Inherited { from } => format!("inherited:{from}"),
        TrustView::Untrusted => "untrusted".to_owned(),
        TrustView::NeverForReview => "never".to_owned(),
    };
    format!(
        "{}\t{}\t{kind}\t{}\t{}\t{trust}\t{}\n",
        checkout.id,
        checkout.project,
        checkout.incarnation,
        checkout.occupant.as_deref().unwrap_or("-"),
        checkout.path
    )
}

fn kept_reason(reason: &KeptReason) -> String {
    match reason {
        KeptReason::Occupied { holder } => format!("occupied by {holder}"),
        KeptReason::UncommittedChanges => "uncommitted changes".to_owned(),
        KeptReason::UnpushedCommits => "unpushed commits".to_owned(),
        KeptReason::Replaced { reason } => format!("replaced: {reason}"),
        KeptReason::Unreadable { reason } => format!("unreadable: {reason}"),
    }
}
