//! `cockpit project` and `cockpit checkout`: verbs served by the daemon over the socket.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_protocol::{
    CheckoutKindView, CheckoutView, Command, GithubTokenView, KeptReason, ProjectView, Reply,
    TrustView,
};

use crate::daemon::{
    BULK_DEADLINE, CONTROL_DEADLINE, absolute_dir, fail, request, unexpected, utf8_words,
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
        ("project", ["remove", project]) => control(Command::ProjectRemove {
            project: (*project).to_owned(),
        }),
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

/// `project show PROJECT`: the project's root and whether it has a GitHub token its
/// sessions get as `GH_TOKEN` (#754), never the value. Exit 1 when no project has that name.
pub fn show(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words = utf8_words(arguments)?;
    let [name] = words.as_slice() else {
        return Err("name one project".to_owned());
    };
    let reply = match request(Command::ProjectList, CONTROL_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return Ok(code),
    };
    let Reply::Projects { projects } = &reply else {
        return Ok(unexpected(&reply));
    };
    let Some(project) = projects.iter().find(|project| project.name == *name) else {
        return Ok(fail(&format!("no project is named {name}")));
    };
    out!("{}", project_details(project));
    Ok(ExitCode::SUCCESS)
}

/// `key<TAB>value` lines: `name`, `root`, then `github-token` with `configured`, `none` or
/// `unusable` and the file or the reason.
fn project_details(project: &ProjectView) -> String {
    let github_token = match &project.github_token {
        Some(GithubTokenView::Configured { path }) => format!("configured\t{path}"),
        Some(GithubTokenView::Missing { path }) => {
            format!("none\tsessions get no GH_TOKEN; the owner may create {path}, mode 0600")
        }
        Some(GithubTokenView::Unusable { reason }) => {
            format!("unusable\tsessions do not launch: {reason}")
        }
        None => "unknown\tthe daemon does not say; restart it on this version".to_owned(),
    };
    format!(
        "name\t{}\nroot\t{}\ngithub-token\t{github_token}\n",
        project.name, project.root
    )
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
    let Some(text) = render(&reply) else {
        return unexpected(&reply);
    };
    out!("{text}");
    match &reply {
        Reply::ProjectAdded { project, .. } if crate::network::sessions_may_be_sandboxed() => {
            offer_github_grant(&project.name)
        }
        _ => ExitCode::SUCCESS,
    }
}

/// #1256: offers the project's github.com grant once, while the owner is adding it, so its
/// GitHub token binds at the first sandboxed launch. Asked on a terminal, where declining is
/// the default; elsewhere only named, so a script never grants. The project is registered
/// either way, so only a grant the owner said yes to and that failed exits 1.
fn offer_github_grant(project: &str) -> ExitCode {
    let Ok(reply) = request(
        Command::NetworkGrants {
            project: Some(project.to_owned()),
        },
        CONTROL_DEADLINE,
    ) else {
        eprintln!("{project} is added; the github.com grant was not offered");
        return ExitCode::SUCCESS;
    };
    let Reply::NetworkGrants { grants } = &reply else {
        unexpected(&reply);
        eprintln!("{project} is added; the github.com grant was not offered");
        return ExitCode::SUCCESS;
    };
    if crate::network::holds_github_grant(grants, project) {
        return ExitCode::SUCCESS;
    }
    let offer = crate::network::github_grant_offer(project);
    let is_interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if !is_interactive {
        eprintln!("{offer}");
        return ExitCode::SUCCESS;
    }
    eprint!(
        "Allow github.com:443 always for {project}'s sandboxed sessions? Fetches work from \
         their first launch, pushes stay refused, and `{} network revoke GRANT` takes it \
         back. [y/N] ",
        crate::profile::command()
    );
    let mut answer = String::new();
    if let Err(error) = std::io::stdin().read_line(&mut answer) {
        eprintln!("no answer read ({error}): not granted. {offer}");
        return ExitCode::SUCCESS;
    }
    if !is_yes(&answer) {
        eprintln!("not granted. {offer}");
        return ExitCode::SUCCESS;
    }
    crate::network::run(crate::network::github_grant_command(project))
}

/// Only `y` or `yes`, in any case, grants; an empty line or anything else declines.
fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// The reply as printed, or `None` when it is not one a project verb gets.
fn render(reply: &Reply) -> Option<String> {
    let text = match reply {
        Reply::ProjectAdded { main, .. } => checkout_line(main),
        Reply::Projects { projects } => projects
            .iter()
            .map(|project| format!("{}\t{}\n", project.name, project.root))
            .collect(),
        Reply::ProjectRemoved {
            project,
            kept_on_disk,
        } => std::iter::once(format!("removed\t{}\t{}\n", project.name, project.root))
            .chain(
                kept_on_disk
                    .iter()
                    .map(|path| format!("kept-on-disk\t{path}\n")),
            )
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
        KeptReason::UnpushedCommits => {
            "unpushed commits (on no remote branch, and not merged into origin/HEAD)".to_owned()
        }
        KeptReason::Replaced { reason } => format!("replaced: {reason}"),
        KeptReason::Unreadable { reason } => format!("unreadable: {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::is_yes;

    #[test]
    fn only_y_or_yes_accepts_the_github_grant() {
        for accepted in ["y\n", "Y\n", "yes\n", " YES \r\n"] {
            assert!(is_yes(accepted), "{accepted:?}");
        }
        for declined in ["\n", "", "n\n", "no\n", "yep\n", "y es\n"] {
            assert!(!is_yes(declined), "{declined:?}");
        }
    }
}
