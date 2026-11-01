//! Issue #427 through the daemon: bug report drafts from agents, the daemon's own faults and
//! the owner, merged by fingerprint and never sent without the owner.
//!
//! Most tests run the daemon binary, this test process playing the launched agent (as in
//! `agent_channel.rs`). A panic and the canary need the daemon in this process: a fixture
//! handler panics in a job, and the fake provider runs an agent with a prompt.
#![cfg(target_os = "linux")]

mod common;

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_agent_channel::{LaunchToken, ProcessIdentity, identity_of};
use cockpit_client::Connection;
use cockpit_daemon::{Daemon, Handler, Instance, Limits, Options, Peer, Route, Routes, WorkClass};
use cockpit_protocol::{
    BugBundleView, BugFiledView, BugKindView, BugOriginView, BugReportView, BugStatusView, Command,
    ErrorCode, Reply, UnblockedBy, Welcome,
};
use cockpit_provider_api::ProviderName;
use cockpit_provider_api::fake::FakeProvider;
use common::{GENEROUS, TestHome, refused};

fn this_process() -> ProcessIdentity {
    identity_of(std::process::id()).unwrap()
}

fn report_bug(token: &LaunchToken, description: &str, requested_by_owner: bool) -> Command {
    Command::AgentReportBug {
        launch_token: Some(token.as_str().to_owned()),
        description: description.to_owned(),
        bug_kind: BugKindView::Timeout,
        component: Some("approvals".to_owned()),
        steps: String::new(),
        requested_by_owner,
    }
}

fn new_draft(description: &str) -> Command {
    Command::BugReportNew {
        description: description.to_owned(),
        title: None,
        steps: String::new(),
    }
}

async fn filed(agent: &Connection, command: Command) -> (i64, BugFiledView) {
    match agent.request(command, GENEROUS).await {
        Ok(Reply::BugReportFiled { report, filed }) => (report, filed),
        other => panic!("expected the filed report, got {other:?}"),
    }
}

async fn drafts(owner: &Connection) -> Vec<BugReportView> {
    reports(owner, false).await
}

async fn reports(owner: &Connection, all: bool) -> Vec<BugReportView> {
    match owner
        .request(Command::BugReportList { all }, GENEROUS)
        .await
    {
        Ok(Reply::BugReports { reports }) => reports,
        other => panic!("expected the reports, got {other:?}"),
    }
}

async fn report(owner: &Connection, command: Command) -> BugReportView {
    match owner.request(command, GENEROUS).await {
        Ok(Reply::BugReport { report }) => *report,
        other => panic!("expected a report, got {other:?}"),
    }
}

/// The drafts once one of `origin` exists; the daemon files its faults off the request path.
async fn once_drafted(owner: &Connection, origin: BugOriginView) -> Vec<BugReportView> {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let drafts = drafts(owner).await;
        if drafts.iter().any(|draft| draft.origin == origin) {
            return drafts;
        }
        assert!(Instant::now() < deadline, "no {origin:?} draft: {drafts:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn an_agent_report_opens_one_draft_and_an_identical_one_merges_into_it() {
    let home = TestHome::new();
    let token = home.launch("s1", &this_process(), None).await;
    let _daemon = home.start();
    let agent = home.connect_agent().await;

    let first = filed(
        &agent,
        report_bug(&token, "cockpit ask timed out after 30 s", false),
    )
    .await;
    let second = filed(
        &agent,
        report_bug(&token, "cockpit ask timed out after 30 s", false),
    )
    .await;

    assert_eq!(first.1, BugFiledView::Created);
    assert_eq!(second, (first.0, BugFiledView::Merged));
    let drafts = drafts(&home.connect().await).await;
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    assert_eq!(drafts[0].occurrences, 2);
    assert_eq!(drafts[0].session.as_deref(), Some("s1"));
    assert_eq!(drafts[0].status, BugStatusView::Draft);
    assert!(
        drafts[0].fingerprint.ends_with("/timeout/approvals"),
        "{}",
        drafts[0].fingerprint
    );
}

#[tokio::test]
async fn an_owner_requested_draft_and_a_new_one_open_beside_todays_detected_draft() {
    let home = TestHome::new();
    let token = home.launch("s1", &this_process(), None).await;
    let _daemon = home.start();
    let agent = home.connect_agent().await;
    let owner = home.connect().await;
    filed(
        &agent,
        report_bug(&token, "the daemon did not answer", false),
    )
    .await;

    let requested = filed(
        &agent,
        report_bug(&token, "the owner asked me to report it", true),
    )
    .await;
    let owner_draft = report(&owner, new_draft("the approval pane froze")).await;

    assert_eq!(requested.1, BugFiledView::Created);
    let drafts = drafts(&owner).await;
    assert_eq!(drafts.len(), 3, "{drafts:?}");
    assert!(
        drafts
            .iter()
            .any(|draft| draft.id == requested.0 && draft.requested_by_owner)
    );
    assert_eq!(owner_draft.origin, BugOriginView::Owner);
    assert!(
        drafts
            .iter()
            .all(|draft| draft.status == BugStatusView::Draft),
        "nothing is sent without the owner"
    );
}

#[tokio::test]
async fn a_new_draft_carries_the_versions_and_a_support_bundle() {
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = home.connect().await;

    let draft = report(&owner, new_draft("the session list stays empty")).await;

    assert!(
        draft.environment.contains("cockpit "),
        "{}",
        draft.environment
    );
    let BugBundleView::Written { path, contents } = &draft.bundle else {
        panic!("the bundle was not written: {:?}", draft.bundle);
    };
    assert!(Path::new(path).is_file(), "{path}");
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(contents.contains("version.txt"), "{contents}");
}

#[tokio::test]
async fn show_hands_the_owner_the_issue_and_discard_removes_the_bundle() {
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let draft = report(&owner, new_draft("the feed stopped")).await;

    let shown = report(&owner, Command::BugReportShow { report: draft.id }).await;
    let discarded = report(&owner, Command::BugReportDiscard { report: draft.id }).await;

    let issue = shown.issue.expect("show carries the issue");
    assert!(
        issue.new_issue_url.starts_with(
            "https://github.com/Androlax2/agent-cockpit/issues/new?template=bug-from-cockpit.yml&"
        ),
        "{}",
        issue.new_issue_url
    );
    assert_eq!(discarded.status, BugStatusView::Discarded);
    let BugBundleView::Written { path, .. } = &draft.bundle else {
        panic!("the bundle was not written: {:?}", draft.bundle);
    };
    assert!(!Path::new(path).exists(), "{path} is left behind");
    assert_eq!(drafts(&owner).await, Vec::new());
}

#[tokio::test]
async fn an_edit_replaces_only_what_it_names() {
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let draft = report(&owner, new_draft("the feed stopped")).await;

    let edited = report(
        &owner,
        Command::BugReportEdit {
            report: draft.id,
            title: Some("Feed stops after a reload".to_owned()),
            description: None,
            steps: Some("1. cockpit config reload".to_owned()),
        },
    )
    .await;

    assert_eq!(edited.title, "Feed stops after a reload");
    assert_eq!(edited.description, "the feed stopped");
    assert_eq!(edited.steps, "1. cockpit config reload");
}

#[tokio::test]
async fn a_sent_draft_records_its_issue_and_leaves_the_drafts() {
    let home = TestHome::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let draft = report(&owner, new_draft("the feed stopped")).await;
    let issue_url = "https://github.com/Androlax2/agent-cockpit/issues/9999";

    let sent = report(
        &owner,
        Command::BugReportSent {
            report: draft.id,
            issue_url: Some(issue_url.to_owned()),
        },
    )
    .await;

    assert_eq!(sent.status, BugStatusView::Sent);
    assert_eq!(sent.issue_url.as_deref(), Some(issue_url));
    assert_eq!(drafts(&owner).await, Vec::new());
    assert_eq!(reports(&owner, true).await.len(), 1);
}

#[tokio::test]
async fn an_agent_reads_no_draft_and_the_owner_files_none_as_an_agent() {
    let home = TestHome::new();
    let token = home.launch("s1", &this_process(), None).await;
    let _daemon = home.start();

    let agent_list = refused(
        home.connect_agent()
            .await
            .request(Command::BugReportList { all: true }, GENEROUS)
            .await,
    );
    let owner_report = refused(
        home.connect()
            .await
            .request(report_bug(&token, "pretending", false), GENEROUS)
            .await,
    );

    assert_eq!(agent_list.code, ErrorCode::NotAuthorized);
    assert_eq!(agent_list.unblock.unwrap().by, UnblockedBy::Owner);
    assert_eq!(owner_report.code, ErrorCode::NotAuthorized);
}

#[tokio::test]
async fn an_agent_cannot_claim_a_fault_only_the_daemon_sees() {
    let home = TestHome::new();
    let token = home.launch("s1", &this_process(), None).await;
    let _daemon = home.start();

    let refusal = refused(
        home.connect_agent()
            .await
            .request(
                Command::AgentReportBug {
                    launch_token: Some(token.as_str().to_owned()),
                    description: "the daemon panicked".to_owned(),
                    bug_kind: BugKindView::Panic,
                    component: None,
                    steps: String::new(),
                    requested_by_owner: false,
                },
                GENEROUS,
            )
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn a_daemon_killed_opens_a_crash_draft_at_its_next_start() {
    let home = TestHome::new();
    home.start().kill();

    let _daemon = home.start();

    let drafts = once_drafted(&home.connect().await, BugOriginView::Daemon).await;
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    assert!(
        drafts[0].fingerprint.ends_with("/crash/daemon"),
        "{}",
        drafts[0].fingerprint
    );
}

#[tokio::test]
async fn a_daemon_stopped_with_sigterm_opens_no_crash_draft() {
    let home = TestHome::new();
    let mut stopped = home.start();
    let pid = stopped.child.id().to_string();
    let status = std::process::Command::new("kill")
        .args(["-TERM", &pid])
        .status()
        .unwrap();
    assert!(status.success());
    let exited = stopped.child.wait().unwrap();
    assert!(
        exited.success(),
        "SIGTERM stops the daemon cleanly: {exited}"
    );

    let _daemon = home.start();

    let owner = home.connect().await;
    tokio::time::sleep(common::SETTLE).await;
    assert_eq!(drafts(&owner).await, Vec::new());
}

/// Panics in every `subscribe` job; everything else is the daemon's own.
struct PanickingSubscribe {
    routes: Routes,
}

impl Handler for PanickingSubscribe {
    fn route(&self, command: Command, welcome: &Welcome, peer: Peer) -> Route {
        if !matches!(command, Command::Subscribe { .. }) {
            return self.routes.route(command, welcome, peer);
        }
        Route::Run {
            class: WorkClass::Bulk,
            job: Box::pin(async { panic!("fixture: the job failed") }),
        }
    }
}

fn options_in(dir: &Path) -> Options {
    Options {
        runtime_dir: dir.join("run/cockpit"),
        state_dir: dir.join("state/cockpit"),
        limits: Limits::default(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_that_panics_opens_a_daemon_draft() {
    let dir = tempfile::tempdir().unwrap();
    let daemon = Instance::acquire(options_in(dir.path()))
        .unwrap()
        .start(common::wiring_in(dir.path(), Vec::new()), |capabilities| {
            Arc::new(PanickingSubscribe {
                routes: Routes::new(capabilities, cockpit_config::Environment::default()),
            })
        })
        .await
        .unwrap();
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    let failed = refused(
        owner
            .request(
                Command::Subscribe {
                    from: cockpit_protocol::FeedPosition::Snapshot,
                },
                GENEROUS,
            )
            .await,
    );

    assert_eq!(failed.code, ErrorCode::Internal);
    let drafts = once_drafted(&owner, BugOriginView::Daemon).await;
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    assert!(
        drafts[0].fingerprint.ends_with("/panic/subscribe"),
        "{}",
        drafts[0].fingerprint
    );
    assert!(
        !drafts[0].description.contains("fixture: the job failed"),
        "a panic's message is not the daemon's to publish: {}",
        drafts[0].description
    );
}

const PROMPT_CANARY: &str = "canary-prompt-5b2e";
const OUTPUT_CANARY: &str = "canary-output-d81c";

/// A session runs with a canary prompt and prints a canary, the daemon logs to its own log
/// files as the product does, and a detected and an owner's draft are opened: neither the
/// drafts, the issue a send would file, nor the bundles carry either canary (report 12).
#[tokio::test(flavor = "multi_thread")]
async fn a_draft_and_its_bundle_carry_no_prompt_or_agent_output() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let _tmux = PrivateTmux(root.clone());
    let state_home = root.join("state");
    let log_dir = cockpit_diagnostics::log_dir_in(&state_home, "daemon").unwrap();
    let diagnostics = cockpit_diagnostics::start(cockpit_diagnostics::Options::new(
        log_dir.clone(),
        cockpit_diagnostics::Directives::from_env_value(Some("debug")).unwrap(),
    ))
    .unwrap();
    tracing::dispatcher::set_global_default(diagnostics.dispatch().clone()).unwrap();
    // A run that did not stop cleanly: the start opens a crash draft.
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(state_home.join("cockpit"))
        .unwrap();
    std::fs::write(state_home.join("cockpit/daemon.running"), "").unwrap();
    let agent = root.join("canary-agent");
    std::fs::write(
        &agent,
        format!("#!/bin/sh\necho {OUTPUT_CANARY}\nexec sleep 60\n"),
    )
    .unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
    let agent_log = root.join("agent.log");
    let provider =
        FakeProvider::new(ProviderName::new("fake").unwrap(), agent_log.clone()).running(agent);
    let environment = cockpit_config::Environment {
        home: Some(root.join("home").into()),
        ..cockpit_config::Environment::default()
    };
    let daemon: Daemon = Instance::acquire(options_in(&root))
        .unwrap()
        .start(
            common::wiring_in(&root, vec![Arc::new(provider)]),
            move |capabilities| Arc::new(Routes::new(capabilities, environment)),
        )
        .await
        .unwrap();
    let owner = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let project = git_project(&root.join("project"));
    owner
        .request(
            Command::ProjectAdd {
                path: project.display().to_string(),
                name: None,
            },
            GENEROUS,
        )
        .await
        .unwrap();
    let started = owner
        .request(
            Command::SessionNew {
                dir: project.display().to_string(),
                agent: Some("fake".to_owned()),
                prompt: Some(PROMPT_CANARY.to_owned()),
                clone: None,
            },
            GENEROUS,
        )
        .await;
    assert!(matches!(started, Ok(Reply::Session { .. })), "{started:?}");
    let launches = common::once_logged(1, || FakeProvider::launches(&agent_log)).await;
    assert_eq!(
        launches[0].message, PROMPT_CANARY,
        "the agent got the canary"
    );

    once_drafted(&owner, BugOriginView::Daemon).await;
    report(&owner, new_draft("the session's card shows nothing")).await;

    let drafts = drafts(&owner).await;
    assert_eq!(drafts.len(), 2, "{drafts:?}");
    for draft in &drafts {
        let shown = report(&owner, Command::BugReportShow { report: draft.id }).await;
        let BugBundleView::Written { path, .. } = &shown.bundle else {
            panic!("the bundle was not written: {:?}", shown.bundle);
        };
        let bundle = String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned();
        assert!(
            bundle.contains("cockpit.log"),
            "the bundle holds the daemon's log"
        );
        let shown = format!("{shown:?}");
        for canary in [PROMPT_CANARY, OUTPUT_CANARY] {
            assert!(
                !shown.contains(canary),
                "{canary} reached draft {}",
                draft.id
            );
            assert!(
                !bundle.contains(canary),
                "{canary} reached the bundle {path}"
            );
        }
    }
    drop(daemon);
}

/// The private tmux server `wiring_in` starts in its directory, killed with the fake agent
/// in it however the test ends.
struct PrivateTmux(PathBuf);

impl Drop for PrivateTmux {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .arg("-S")
            .arg(self.0.join("tmux.sock"))
            .arg("kill-server")
            .env_remove("TMUX")
            .output();
    }
}

/// A Git work tree with one commit at `root`.
fn git_project(root: &Path) -> PathBuf {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("README"), "fixture\n").unwrap();
    for arguments in [
        &["init", "--quiet", "--initial-branch=main"][..],
        &["add", "."],
        &["commit", "--quiet", "-m", "initial"],
    ] {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    root.to_path_buf()
}
