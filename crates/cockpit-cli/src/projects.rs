//! `cockpit project` and `cockpit checkout`: verbs served by the daemon over the socket.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_protocol::{
    CheckoutKindView, CheckoutView, Command, GithubTokenView, KeptReason, ProjectView,
    ProtectOutcomeView, Reply, TrustView,
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
        ("project", ["push-branch", "add", project, pattern]) => {
            control(Command::ProjectPushBranchAdd {
                project: (*project).to_owned(),
                pattern: (*pattern).to_owned(),
            })
        }
        ("project", ["push-branch", "remove", project, pattern]) => {
            control(Command::ProjectPushBranchRemove {
                project: (*project).to_owned(),
                pattern: (*pattern).to_owned(),
            })
        }
        ("project", ["push-branch", "list", project]) => control(Command::ProjectPushBranchList {
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
            expected: None,
        }),
        ("checkout", ["push", checkout, "--branch", branch, "--commit", commit]) => {
            bulk(Command::CheckoutPush {
                checkout: checkout_id(checkout)?,
                expected: Some(cockpit_protocol::PushExpected {
                    branch: (*branch).to_owned(),
                    commit: (*commit).to_owned(),
                }),
            })
        }
        _ => Err(format!("unknown {verb} arguments")),
    }
}

/// The longest protect may take: up to five `gh` calls of 30 s at most (the adapter's probe,
/// the default branch, the rulesets, Torlek's ruleset and the write), and a margin.
const PROTECT_DEADLINE: Duration = Duration::from_secs(5 * 30 + 30);

/// `project protect PROJECT --check NAME [--check NAME]...` (#695): asks for a token that may
/// change the repository's settings on a prompt that does not echo it, or reads it from
/// standard input, and has the daemon protect the default branch with it once. The token is
/// never stored (Theo, 2026-11-23).
pub fn protect(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words = utf8_words(arguments)?;
    let Some((project, flags)) = words.split_first() else {
        return Err("name one project".to_owned());
    };
    if project.starts_with("--") {
        return Err("name the project first".to_owned());
    }
    let mut required_checks = Vec::new();
    for pair in flags.chunks(2) {
        match pair {
            ["--check", check] if !check.starts_with("--") => {
                required_checks.push((*check).to_owned());
            }
            ["--check", ..] => return Err("--check needs a check's name".to_owned()),
            [flag, ..] => return Err(format!("{flag}: only --check NAME follows the project")),
            [] => unreachable!("chunks are never empty"),
        }
    }
    if required_checks.is_empty() {
        return Err(format!(
            "name each check CI must pass on the default branch: \
             `torlek project protect {project} --check NAME`"
        ));
    }
    // A token that may change settings is not asked for on a name that leads nowhere.
    let reply = match request(Command::ProjectList, CONTROL_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return Ok(code),
    };
    let Reply::Projects { projects } = &reply else {
        return Ok(unexpected(&reply));
    };
    if !projects.iter().any(|known| known.name == *project) {
        return Ok(fail(&format!(
            "no project is named {project}: `torlek project list` names them"
        )));
    }
    let token = crate::credentials::read_secret(&format!(
        "a token that may change {project}'s repository settings, used once and never stored"
    ))?;
    let command = Command::ProjectProtect {
        project: (*project).to_owned(),
        required_checks,
        token,
    };
    Ok(match request(command, PROTECT_DEADLINE) {
        Ok(Reply::ProjectProtected {
            project,
            branch,
            outcome,
        }) => {
            let done = match outcome {
                ProtectOutcomeView::Created => "protected: Torlek's rule was created",
                ProtectOutcomeView::Updated => "protected: Torlek's rule was updated",
                ProtectOutcomeView::Unchanged => "already protected: nothing was changed",
            };
            outln!("{project} {branch}: {done}");
            outln!("revoke the token now: Torlek did not keep it");
            ExitCode::SUCCESS
        }
        Ok(other) => unexpected(&other),
        Err(code) => code,
    })
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
/// GitHub token binds at the first sandboxed launch.
fn offer_github_grant(project: &str) -> ExitCode {
    crate::network::offer_grant(crate::network::GrantOffer {
        project,
        not_offered: format!("{project} is added; the github.com grant was not offered"),
        holds: crate::network::holds_github_grant,
        question: format!(
            "Allow github.com:443 always for {project}'s sandboxed sessions? Fetches work from \
             their first launch, pushes stay refused, and `{} network revoke GRANT` takes it \
             back.",
            crate::profile::command()
        ),
        offer: crate::network::github_grant_offer(project),
        grant: crate::network::github_grant_command(project),
    })
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
        Reply::PushBranches {
            project,
            patterns,
            changed,
        } => {
            let unchanged = if *changed == Some(false) {
                "nothing changed\n"
            } else {
                ""
            };
            let listed: String = if patterns.is_empty() {
                format!(
                    "{project}'s agents push their launch branch alone; add more with `torlek \
                     project push-branch add {project} PATTERN`\n"
                )
            } else {
                patterns
                    .iter()
                    .map(|pattern| format!("{pattern}\n"))
                    .collect()
            };
            format!("{unchanged}{listed}")
        }
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
