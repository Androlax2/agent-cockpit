//! `cockpit project` and `cockpit checkout`: verbs served by the daemon over the socket.

use std::ffi::OsString;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{CheckoutKindView, CheckoutView, Command, KeptReason, Reply, TrustView};

/// Control verbs answer within this; the daemon answers `busy` at once when it is saturated.
const CONTROL_DEADLINE: Duration = Duration::from_secs(30);
/// Cloning or pushing a large repository can take minutes.
const BULK_DEADLINE: Duration = Duration::from_secs(30 * 60);

/// A verb's command, or the usage problem that stops it.
pub fn parse(verb: &str, arguments: &[OsString]) -> Result<(Command, Duration), String> {
    let words: Vec<&str> = arguments
        .iter()
        .map(|word| {
            word.to_str()
                .ok_or_else(|| format!("{}: not UTF-8", word.to_string_lossy()))
        })
        .collect::<Result<_, _>>()?;
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

/// The daemon resolves nothing relative to its own directory: a relative `dir` is made
/// absolute against this command's working directory here.
fn project_add(dir: &str, name: Option<&str>) -> Result<Command, String> {
    let path = std::path::absolute(dir).map_err(|error| format!("{dir}: {error}"))?;
    let path = path
        .to_str()
        .ok_or_else(|| format!("{}: not UTF-8", path.display()))?
        .to_owned();
    Ok(Command::ProjectAdd {
        path,
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
    let socket = match crate::agent::socket_dir() {
        Ok(dir) => dir.join(cockpit_config::DAEMON_SOCKET),
        Err(code) => return code,
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return fail(&format!("cannot start the client runtime: {error}")),
    };
    let answer = runtime.block_on(async {
        let connection = Connection::open(&socket, CONTROL_DEADLINE).await?;
        connection.request(command, deadline).await
    });
    match answer {
        Ok(reply) => match render(&reply) {
            Some(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            None => fail(&format!("the daemon answered with an unexpected {reply:?}")),
        },
        Err(ClientError::Daemon(error)) => {
            fail(&format!("{}: {}", error.code.as_str(), error.message))
        }
        Err(error) => fail(&error.to_string()),
    }
}

fn fail(problem: &str) -> ExitCode {
    eprintln!("cockpit: {problem}");
    ExitCode::FAILURE
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
