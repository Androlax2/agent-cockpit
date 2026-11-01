//! `cockpit agent report-bug` (a session, on the agent socket) and `cockpit bug-report` (the
//! owner, on the host-only socket): bug reports about Cockpit itself (#427). A report is a
//! draft until the owner sends it; `send` opens the prefilled issue in the owner's browser,
//! or files it with the owner's own `gh` login.

use std::ffi::OsString;
use std::process::{ExitCode, Stdio};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    BugBundleView, BugFiledView, BugIssueView, BugKindView, BugReportView, BugStatusView, Command,
    Reply,
};

use crate::agent::{agent_socket, exchange, launch_token};
use crate::daemon::{CONTROL_DEADLINE, fail, failed, request, unexpected, utf8_words};

/// The kinds a session reports; the daemon observes panics, stalls and crashes itself.
const AGENT_KINDS: [BugKindView; 4] = [
    BugKindView::Timeout,
    BugKindView::Unreachable,
    BugKindView::Internal,
    BugKindView::Other,
];

/// `cockpit agent report-bug DESCRIPTION [--kind K] [--component NAME] [--steps TEXT]
/// [--requested-by-owner]`.
pub(crate) fn agent_report(arguments: &[OsString]) -> Result<ExitCode, String> {
    let command = parse_agent_report(&utf8_words(arguments)?)?;
    let socket = match agent_socket() {
        Ok(socket) => socket,
        Err(code) => return Ok(code),
    };
    Ok(match exchange(&socket, command) {
        Ok(Reply::BugReportFiled { report, filed }) => {
            let what = match filed {
                BugFiledView::Created => "opened bug report draft",
                BugFiledView::Merged => "added to today's bug report draft",
                BugFiledView::AlreadyHandled => {
                    "the owner already handled today's report of this bug; nothing opened, see draft"
                }
            };
            outln!("{what} {report}; the owner reviews it before anything is sent");
            ExitCode::SUCCESS
        }
        Err(ClientError::Connect { source, .. }) => {
            eprintln!(
                "cockpit: cannot reach the daemon at {}: {source}",
                socket.display()
            );
            eprintln!(
                "cockpit: no draft was opened: tell the owner in the conversation what happened"
            );
            ExitCode::FAILURE
        }
        other => crate::agent::failed(&socket, other),
    })
}

fn parse_agent_report(words: &[&str]) -> Result<Command, String> {
    let mut description = None;
    let mut bug_kind = None;
    let mut component = None;
    let mut steps = None;
    let mut requested_by_owner = false;
    let mut words = words.iter();
    while let Some(&word) = words.next() {
        if word == "--requested-by-owner" {
            if requested_by_owner {
                return Err("--requested-by-owner given twice".to_owned());
            }
            requested_by_owner = true;
            continue;
        }
        if !word.starts_with("--") {
            if description.replace(word.to_owned()).is_some() {
                return Err("give the description as one quoted argument".to_owned());
            }
            continue;
        }
        let value = words
            .next()
            .ok_or_else(|| format!("{word} needs a value"))?;
        match word {
            "--kind" => {
                let kind = AGENT_KINDS
                    .into_iter()
                    .find(|kind| kind.as_str() == *value)
                    .ok_or_else(|| {
                        format!("--kind {value}: use timeout, unreachable, internal or other")
                    })?;
                if bug_kind.replace(kind).is_some() {
                    return Err("--kind given twice".to_owned());
                }
            }
            "--component" => set_once(&mut component, word, value)?,
            "--steps" => set_once(&mut steps, word, value)?,
            _ => return Err(format!("unknown argument {word}")),
        }
    }
    Ok(Command::AgentReportBug {
        launch_token: launch_token(),
        description: description
            .ok_or("say what you were doing and what happened, as one quoted argument")?,
        bug_kind: bug_kind.unwrap_or(BugKindView::Other),
        component,
        steps: steps.unwrap_or_default(),
        requested_by_owner,
    })
}

fn set_once(field: &mut Option<String>, flag: &str, value: &str) -> Result<(), String> {
    if field.replace(value.to_owned()).is_some() {
        return Err(format!("{flag} given twice"));
    }
    Ok(())
}

/// What `cockpit bug-report` was asked to do.
#[derive(Debug)]
pub enum Verb {
    /// A command whose answer is printed as it is.
    Daemon(Command),
    Send(Send),
}

/// `send ID [--gh] [--print] [--anyway]`.
#[derive(Debug)]
pub struct Send {
    report: i64,
    /// File with `gh issue create` instead of opening the browser.
    is_gh: bool,
    /// Print the URL instead of opening it.
    is_print_only: bool,
    /// Send although an issue with the same fingerprint exists.
    is_anyway: bool,
}

/// A `bug-report` verb, or the usage problem that stops it.
pub fn parse(arguments: &[OsString]) -> Result<Verb, String> {
    let words = utf8_words(arguments)?;
    let Some((&verb, rest)) = words.split_first() else {
        return Err("bug-report needs a verb".to_owned());
    };
    Ok(Verb::Daemon(match (verb, rest) {
        ("new", rest) => {
            let (description, flags) = text_and_flags(rest, &["--title", "--steps"])?;
            Command::BugReportNew {
                description: description.ok_or("say what happened, as one quoted argument")?,
                title: flags[0].clone(),
                steps: flags[1].clone().unwrap_or_default(),
            }
        }
        ("list", []) => Command::BugReportList { all: false },
        ("list", ["--all"]) => Command::BugReportList { all: true },
        ("show", [report]) => Command::BugReportShow {
            report: report_id(report)?,
        },
        ("edit", [report, rest @ ..]) => {
            let (text, flags) = text_and_flags(rest, &["--title", "--description", "--steps"])?;
            if let Some(text) = text {
                return Err(format!("unknown argument {text}"));
            }
            let [title, description, steps] = flags;
            if title.is_none() && description.is_none() && steps.is_none() {
                return Err("edit needs --title, --description or --steps".to_owned());
            }
            Command::BugReportEdit {
                report: report_id(report)?,
                title,
                description,
                steps,
            }
        }
        ("send", [report, flags @ ..]) => return parse_send(report, flags),
        ("discard", [report]) => Command::BugReportDiscard {
            report: report_id(report)?,
        },
        _ => return Err("unknown bug-report arguments".to_owned()),
    }))
}

fn parse_send(report: &str, flags: &[&str]) -> Result<Verb, String> {
    let mut send = Send {
        report: report_id(report)?,
        is_gh: false,
        is_print_only: false,
        is_anyway: false,
    };
    for &flag in flags {
        let field = match flag {
            "--gh" => &mut send.is_gh,
            "--print" => &mut send.is_print_only,
            "--anyway" => &mut send.is_anyway,
            _ => return Err(format!("unknown argument {flag}")),
        };
        if std::mem::replace(field, true) {
            return Err(format!("{flag} given twice"));
        }
    }
    if send.is_gh && send.is_print_only {
        return Err("--gh files the issue itself; --print only prints its URL".to_owned());
    }
    Ok(Verb::Send(send))
}

/// The one free argument, and the value of each of `names`, each at most once.
fn text_and_flags<const N: usize>(
    words: &[&str],
    names: &[&str; N],
) -> Result<(Option<String>, [Option<String>; N]), String> {
    let mut text = None;
    let mut values: [Option<String>; N] = std::array::from_fn(|_| None);
    let mut words = words.iter();
    while let Some(&word) = words.next() {
        let Some(index) = names.iter().position(|name| *name == word) else {
            if word.starts_with("--") {
                return Err(format!("unknown argument {word}"));
            }
            if text.replace(word.to_owned()).is_some() {
                return Err("give the text as one quoted argument".to_owned());
            }
            continue;
        };
        let value = words
            .next()
            .ok_or_else(|| format!("{word} needs a value"))?;
        set_once(&mut values[index], word, value)?;
    }
    Ok((text, values))
}

fn report_id(word: &str) -> Result<i64, String> {
    word.parse()
        .map_err(|_| format!("{word}: not a bug report number"))
}

/// Runs `verb` and prints what it answers. Exit 1 when the daemon refuses.
pub fn run(verb: Verb) -> ExitCode {
    let command = match verb {
        Verb::Daemon(command) => command,
        Verb::Send(send) => return send_report(&send),
    };
    let reply = match request(command, CONTROL_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return code,
    };
    match &reply {
        Reply::BugReports { reports } if reports.is_empty() => outln!("no bug report drafts"),
        Reply::BugReports { reports } => {
            for report in reports {
                outln!("{}", summary_line(report));
            }
        }
        Reply::BugReport { report } => out!("{}", report_text(report)),
        _ => return unexpected(&reply),
    }
    ExitCode::SUCCESS
}

/// `ID status origin xN fingerprint: title`.
fn summary_line(report: &BugReportView) -> String {
    let mut origin = report.origin.as_str().to_owned();
    if let Some(session) = &report.session {
        origin.push_str(&format!(" {session}"));
    }
    if report.requested_by_owner {
        origin.push_str(" (asked by the owner)");
    }
    format!(
        "{} {} {origin} x{} {}: {}",
        report.id,
        report.status.as_str(),
        report.occurrences,
        report.fingerprint,
        report.title
    )
}

fn report_text(report: &BugReportView) -> String {
    let mut text = format!("{}\n\n{}\n", summary_line(report), report.description);
    if !report.steps.is_empty() {
        text.push_str(&format!("\nSteps to reproduce:\n{}\n", report.steps));
    }
    text.push_str(&format!(
        "\nVersions and platform:\n{}\n",
        report.environment
    ));
    text.push_str(&format!(
        "\nSupport bundle: {}\n",
        bundle_text(&report.bundle)
    ));
    if let Some(url) = &report.issue_url {
        text.push_str(&format!("\nFiled as {url}\n"));
    }
    if let Some(issue) = &report.issue {
        text.push_str(&format!("\nNew issue URL:\n{}\n", issue.new_issue_url));
        if let Some(search) = &issue.search_url {
            text.push_str(&format!(
                "Existing issues with this fingerprint:\n{search}\n"
            ));
        }
    }
    if report.status == BugStatusView::Draft {
        text.push_str(&format!(
            "\nNothing is sent until you run `cockpit bug-report send {}`.\n",
            report.id
        ));
    }
    text
}

fn bundle_text(bundle: &BugBundleView) -> String {
    match bundle {
        BugBundleView::Written { path, contents } => format!("{path}\n{contents}"),
        BugBundleView::Pending => "not written yet".to_owned(),
        BugBundleView::Failed { problem } => format!("none: {problem}"),
    }
}

/// Shows the fingerprint, offers an existing issue instead of a duplicate, then opens the
/// prefilled issue (or files it with `gh`) and records the draft as sent. One connection:
/// the draft read is the draft sent.
fn send_report(send: &Send) -> ExitCode {
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
    runtime.block_on(async {
        let connection = match Connection::open(&socket, CONTROL_DEADLINE).await {
            Ok(connection) => connection,
            Err(error) => return failed(&error),
        };
        let shown = connection
            .request(
                Command::BugReportShow {
                    report: send.report,
                },
                CONTROL_DEADLINE,
            )
            .await;
        let report = match shown {
            Ok(Reply::BugReport { report }) => report,
            Ok(reply) => return unexpected(&reply),
            Err(error) => return failed(&error),
        };
        if report.status != BugStatusView::Draft {
            return fail(&format!(
                "bug report {} is {}, not a draft",
                report.id,
                report.status.as_str()
            ));
        }
        let Some(issue) = &report.issue else {
            return fail("the daemon sent the draft without its issue");
        };
        outln!("fingerprint: {}", report.fingerprint);
        if !send.is_anyway && offers_existing_issue(issue) {
            return ExitCode::SUCCESS;
        }
        let issue_url = if send.is_gh {
            match gh_create(issue) {
                Ok(url) => Some(url),
                Err(problem) => return fail(&problem),
            }
        } else {
            open_in_browser(&issue.new_issue_url, send.is_print_only);
            None
        };
        let sent = connection
            .request(
                Command::BugReportSent {
                    report: report.id,
                    issue_url: issue_url.clone(),
                },
                CONTROL_DEADLINE,
            )
            .await;
        match sent {
            Ok(Reply::BugReport { .. }) => {}
            Ok(reply) => return unexpected(&reply),
            Err(error) => return failed(&error),
        }
        if let Some(url) = &issue_url {
            outln!("filed {url}");
        }
        if let BugBundleView::Written { path, .. } = &report.bundle {
            outln!("attach the support bundle to the issue: {path}");
        }
        outln!("bug report {} marked sent", report.id);
        ExitCode::SUCCESS
    })
}

/// Whether an issue with the draft's fingerprint exists, found with the owner's `gh`; prints
/// them. Without `gh`, prints the search to check by hand and goes on.
fn offers_existing_issue(issue: &BugIssueView) -> bool {
    let Some(query) = &issue.search_query else {
        return false;
    };
    let found = std::process::Command::new("gh")
        .args([
            "issue",
            "list",
            "--repo",
            &issue.repository,
            "--state",
            "all",
        ])
        .args(["--limit", "5", "--search", query])
        .args(["--json", "url,title", "--template"])
        .arg("{{range .}}{{.url}}\t{{.title}}\n{{end}}")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let listed = match found {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        _ => {
            if let Some(search) = &issue.search_url {
                outln!("check for an existing issue first: {search}");
            }
            return false;
        }
    };
    if listed.trim().is_empty() {
        return false;
    }
    outln!("an issue with this fingerprint exists; add to it instead of filing a duplicate:");
    out!("{listed}");
    outln!("to file a new one anyway: cockpit bug-report send <id> --anyway");
    true
}

/// `gh issue create` with the owner's login; the URL it prints.
fn gh_create(issue: &BugIssueView) -> Result<String, String> {
    let output = std::process::Command::new("gh")
        .args(["issue", "create", "--repo", &issue.repository])
        .args(["--title", &issue.title, "--body", &issue.body])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("cannot run gh: {error}"))?;
    if !output.status.success() {
        return Err(format!("gh issue create failed ({})", output.status));
    }
    let url = String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .find(|line| line.starts_with("https://"))
        .map(str::to_owned);
    url.ok_or_else(|| "gh issue create printed no issue URL".to_owned())
}

/// Opens `url` with the desktop's opener; prints it when asked to, or when none opens it.
fn open_in_browser(url: &str, is_print_only: bool) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let opened = !is_print_only
        && std::process::Command::new(opener)
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    if opened {
        outln!("opened the prefilled issue in your browser; review it and submit it there");
    } else {
        outln!("open this URL to review and submit the issue:\n{url}");
    }
}
