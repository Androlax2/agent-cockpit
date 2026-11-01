//! `cockpit agent report-bug` and `cockpit bug-report` driven as processes against a scripted
//! daemon socket (#427). The daemon's side is proven in `cockpit-daemon/tests/bug_reports.rs`;
//! these prove each verb's request, its output, and that `send` hands the owner the URL the
//! draft carries, through stand-in `xdg-open` and `gh` programs and no network.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    BugBundleView, BugFiledView, BugIssueView, BugKindView, BugOriginView, BugReportView,
    BugStatusView, Command, Outcome, Reply,
};
use scripted::{scripted_conversation, scripted_daemon, text};

const NEW_ISSUE_URL: &str = "https://github.com/Androlax2/agent-cockpit/issues/new?template=bug-from-cockpit.yml&title=Approval%20pane%20froze&description=the%20approval%20pane%20froze";
const SEARCH_QUERY: &str = "\"cockpit-0.1.0/timeout/approvals\" in:body";
const BUNDLE: &str = "/state/cockpit/bug-reports/cockpit-bug-report-7.tar";

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    /// Where the daemon puts a socket, `relative` to its socket directory.
    fn socket(&self, relative: &str) -> PathBuf {
        let path = self.runtime_dir().join("cockpit-dev").join(relative);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    fn bin(&self) -> PathBuf {
        self.dir.path().join("bin")
    }

    /// A stand-in for `program` on `PATH`: records its arguments, one per line, then runs
    /// `then` (shell).
    fn stand_in(&self, program: &str, then: &str) {
        std::fs::create_dir_all(self.bin()).unwrap();
        let path = self.bin().join(program);
        let record = self.dir.path().join(format!("{program}.args"));
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nfor argument in \"$@\"; do printf '%s\\n' \"$argument\" >>'{}'; done\n{then}\n",
                record.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The arguments `program`'s stand-in was run with, one per line; empty when never run.
    fn ran(&self, program: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(format!("{program}.args"))).unwrap_or_default()
    }

    /// Runs `cockpit` with `arguments` in this home, with only the stand-ins on `PATH`.
    async fn cockpit(&self, arguments: &[&str], variables: &[(&str, &Path)]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.runtime_dir())
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin().display()));
        for (name, value) in variables {
            command.env(name, value);
        }
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn draft() -> BugReportView {
    BugReportView {
        id: 7,
        origin: BugOriginView::Agent,
        requested_by_owner: false,
        session: Some("s1".to_owned()),
        status: BugStatusView::Draft,
        title: "Approval pane froze".to_owned(),
        description: "the approval pane froze".to_owned(),
        steps: String::new(),
        fingerprint: "cockpit-0.1.0/timeout/approvals".to_owned(),
        environment: "cockpit 0.1.0".to_owned(),
        bundle: BugBundleView::Written {
            path: BUNDLE.to_owned(),
            contents: "14\tversion.txt".to_owned(),
        },
        occurrences: 2,
        issue_url: None,
        created_at: 1,
        changed_at: 2,
        issue: Some(BugIssueView {
            repository: "Androlax2/agent-cockpit".to_owned(),
            title: "Approval pane froze".to_owned(),
            body: "### What happened\n\nthe approval pane froze".to_owned(),
            new_issue_url: NEW_ISSUE_URL.to_owned(),
            search_url: Some("https://github.com/Androlax2/agent-cockpit/issues?q=x".to_owned()),
            search_query: Some(SEARCH_QUERY.to_owned()),
            is_truncated: false,
        }),
    }
}

fn shown() -> Outcome {
    Outcome::Ok(Reply::BugReport {
        report: Box::new(draft()),
    })
}

fn sent(issue_url: Option<&str>) -> Outcome {
    Outcome::Ok(Reply::BugReport {
        report: Box::new(BugReportView {
            status: BugStatusView::Sent,
            issue_url: issue_url.map(str::to_owned),
            issue: None,
            ..draft()
        }),
    })
}

#[tokio::test]
async fn an_agents_report_goes_to_the_agent_socket_with_its_kind_and_token() {
    let home = Home::new();
    let socket = home.socket("agent/agent.sock");
    let daemon = scripted_daemon(
        &socket,
        Outcome::Ok(Reply::BugReportFiled {
            report: 7,
            filed: BugFiledView::Merged,
        }),
    );

    let output = home
        .cockpit(
            &[
                "agent",
                "report-bug",
                "cockpit ask timed out after 30 s",
                "--kind",
                "timeout",
                "--component",
                "approvals",
                "--requested-by-owner",
            ],
            &[("COCKPIT_LAUNCH_TOKEN", Path::new("the-token"))],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "added to today's bug report draft 7; the owner reviews it before anything is sent\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::AgentReportBug {
            launch_token: Some("the-token".to_owned()),
            description: "cockpit ask timed out after 30 s".to_owned(),
            bug_kind: BugKindView::Timeout,
            component: Some("approvals".to_owned()),
            steps: String::new(),
            requested_by_owner: true,
        }
    );
}

#[tokio::test]
async fn an_agents_report_without_a_daemon_says_to_tell_the_owner() {
    let home = Home::new();
    home.socket("agent/agent.sock");

    let output = home
        .cockpit(&["agent", "report-bug", "cockpit ask hung"], &[])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("tell the owner in the conversation"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn an_agent_cannot_report_a_kind_only_the_daemon_observes() {
    let home = Home::new();

    let output = home
        .cockpit(
            &["agent", "report-bug", "it panicked", "--kind", "panic"],
            &[],
        )
        .await;

    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("--kind panic: use timeout, unreachable, internal or other"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn new_asks_the_daemon_for_a_draft_and_prints_it() {
    let home = Home::new();
    let daemon = scripted_daemon(&home.socket("daemon.sock"), shown());

    let output = home
        .cockpit(
            &[
                "bug-report",
                "new",
                "the approval pane froze",
                "--steps",
                "1. open it",
            ],
            &[],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let printed = text(&output.stdout);
    assert!(
        printed.starts_with(
            "7 draft agent s1 x2 cockpit-0.1.0/timeout/approvals: Approval pane froze\n"
        ),
        "{printed}"
    );
    assert!(
        printed.contains("Nothing is sent until you run `cockpit bug-report send 7`."),
        "{printed}"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::BugReportNew {
            description: "the approval pane froze".to_owned(),
            title: None,
            steps: "1. open it".to_owned(),
        }
    );
}

#[tokio::test]
async fn list_prints_one_line_per_draft() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.socket("daemon.sock"),
        Outcome::Ok(Reply::BugReports {
            reports: vec![draft()],
        }),
    );

    let output = home.cockpit(&["bug-report", "list"], &[]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "7 draft agent s1 x2 cockpit-0.1.0/timeout/approvals: Approval pane froze\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::BugReportList { all: false });
}

#[tokio::test]
async fn send_opens_the_drafts_issue_url_in_the_browser_and_records_it_sent() {
    let home = Home::new();
    home.stand_in("xdg-open", "exit 0");
    home.stand_in("gh", "exit 0");
    let daemon = scripted_conversation(&home.socket("daemon.sock"), vec![shown(), sent(None)]);

    let output = home.cockpit(&["bug-report", "send", "7"], &[]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(home.ran("xdg-open"), format!("{NEW_ISSUE_URL}\n"));
    assert!(
        home.ran("gh").contains(SEARCH_QUERY),
        "the duplicate search runs first: {}",
        home.ran("gh")
    );
    let printed = text(&output.stdout);
    assert!(
        printed.contains("fingerprint: cockpit-0.1.0/timeout/approvals\n"),
        "{printed}"
    );
    assert!(
        printed.contains(&format!(
            "attach the support bundle to the issue: {BUNDLE}\n"
        )),
        "{printed}"
    );
    assert_eq!(
        daemon.await.unwrap(),
        vec![
            Command::BugReportShow { report: 7 },
            Command::BugReportSent {
                report: 7,
                issue_url: None,
            },
        ]
    );
}

#[tokio::test]
async fn send_print_prints_the_url_and_opens_nothing() {
    let home = Home::new();
    home.stand_in("xdg-open", "exit 0");
    let daemon = scripted_conversation(&home.socket("daemon.sock"), vec![shown(), sent(None)]);

    let output = home
        .cockpit(&["bug-report", "send", "7", "--print"], &[])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stdout).contains(&format!("{NEW_ISSUE_URL}\n")),
        "{}",
        text(&output.stdout)
    );
    assert_eq!(home.ran("xdg-open"), "");
    assert_eq!(daemon.await.unwrap().len(), 2);
}

#[tokio::test]
async fn send_offers_an_existing_issue_instead_of_a_duplicate() {
    let home = Home::new();
    home.stand_in("xdg-open", "exit 0");
    home.stand_in(
        "gh",
        "printf 'https://github.com/Androlax2/agent-cockpit/issues/41\\tApproval pane froze\\n'",
    );
    let daemon = scripted_conversation(&home.socket("daemon.sock"), vec![shown(), sent(None)]);

    let output = home.cockpit(&["bug-report", "send", "7"], &[]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stdout).contains("https://github.com/Androlax2/agent-cockpit/issues/41"),
        "{}",
        text(&output.stdout)
    );
    assert_eq!(home.ran("xdg-open"), "", "no new issue is opened");
    assert_eq!(
        daemon.await.unwrap(),
        vec![Command::BugReportShow { report: 7 }],
        "the draft stays a draft"
    );
}

#[tokio::test]
async fn send_with_gh_files_the_issue_with_the_owners_login_and_records_its_url() {
    let home = Home::new();
    let filed = "https://github.com/Androlax2/agent-cockpit/issues/42";
    // `issue list` finds nothing; `issue create` prints the new issue's URL.
    home.stand_in(
        "gh",
        &format!("[ \"$2\" = create ] && printf '{filed}\\n'; exit 0"),
    );
    let daemon = scripted_conversation(
        &home.socket("daemon.sock"),
        vec![shown(), sent(Some(filed))],
    );

    let output = home
        .cockpit(&["bug-report", "send", "7", "--gh"], &[])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let ran = home.ran("gh");
    assert!(
        ran.contains("create\n--repo\nAndrolax2/agent-cockpit\n--title\nApproval pane froze\n"),
        "{ran}"
    );
    assert!(
        text(&output.stdout).contains(&format!("filed {filed}\n")),
        "{}",
        text(&output.stdout)
    );
    assert_eq!(
        daemon.await.unwrap(),
        vec![
            Command::BugReportShow { report: 7 },
            Command::BugReportSent {
                report: 7,
                issue_url: Some(filed.to_owned()),
            },
        ]
    );
}

#[tokio::test]
async fn an_edit_that_names_nothing_is_a_usage_error() {
    let home = Home::new();

    let output = home.cockpit(&["bug-report", "edit", "7"], &[]).await;

    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("edit needs --title, --description or --steps"),
        "{}",
        text(&output.stderr)
    );
}
