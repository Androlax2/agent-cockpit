//! Issue #38's acceptance through `cockpit todo`: the owner's verbs against the real daemon,
//! and a launched session's verbs against a scripted agent socket (the crate graph keeps the
//! CLI's tests from launching a session; the daemon's side of those is proven in
//! `cockpit-daemon/tests/tasks.rs`).
mod common;
mod scripted;

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

use cockpit_protocol::{
    Command, ErrorCode, Outcome, ProtocolError, Reply, TaskKindView, TaskNextView, TaskStatusView,
    TaskView, Unblock, UnblockedBy,
};
use common::TestHome;
use scripted::{scripted_daemon, text};

/// The real daemon with project `app` registered.
fn with_app() -> (TestHome, common::RunningDaemon) {
    let home = TestHome::new();
    let daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    (home, daemon)
}

fn the_line(output: &str, prefix: &str) -> String {
    output
        .lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix:?} line in {output:?}"))
        .to_owned()
}

#[test]
fn the_owner_adds_an_item_and_the_list_shows_it() {
    let (home, _daemon) = with_app();

    home.cockpit_ok(&[
        "todo",
        "add",
        "Fix the login",
        "--project",
        "app",
        "--kind",
        "bug",
        "--priority",
        "3",
    ]);

    assert_eq!(
        home.cockpit_ok(&["todo", "list"]),
        "1\tapp\ttodo\t3\tbug\tFix the login\n"
    );
}

#[test]
fn an_item_after_another_says_what_it_waits_for_until_that_is_done() {
    let (home, _daemon) = with_app();
    home.cockpit_ok(&["todo", "add", "Migrate", "--project", "app"]);
    home.cockpit_ok(&[
        "todo",
        "add",
        "Backfill",
        "--project",
        "app",
        "--after",
        "1",
    ]);

    let waiting = home.cockpit_ok(&["todo", "show", "2"]);
    home.cockpit_ok(&["todo", "done", "1", "--summary", "column added in 0042"]);
    let ready = home.cockpit_ok(&["todo", "show", "2"]);

    assert_eq!(
        the_line(&waiting, "next:"),
        "next: items 1 finish first, or the owner removes the wait"
    );
    assert_eq!(
        the_line(&ready, "after:"),
        "after: 1 Migrate (done): column added in 0042"
    );
    assert_eq!(
        the_line(&ready, "next:"),
        "next: the owner queues it, or a session starts it"
    );
}

#[test]
fn an_edit_made_on_a_stale_intent_revision_is_refused() {
    let (home, _daemon) = with_app();
    home.cockpit_ok(&["todo", "add", "Fix the login", "--project", "app"]);
    home.cockpit_ok(&["todo", "edit", "1", "--revision", "1", "--title", "Fix it"]);

    let refusal = home.cockpit_err(&["todo", "edit", "1", "--revision", "1", "--title", "Other"]);

    assert!(refusal.contains("refused:"), "{refusal}");
    assert!(
        home.cockpit_ok(&["todo", "show", "1"])
            .contains("title: Fix it\n")
    );
}

#[test]
fn a_queued_item_is_marked_in_the_list() {
    let (home, _daemon) = with_app();
    home.cockpit_ok(&["todo", "add", "Fix the login", "--project", "app"]);

    home.cockpit_ok(&["todo", "queue", "1"]);

    assert_eq!(
        home.cockpit_ok(&["todo", "list"]),
        "1\tapp\ttodo,queued\t2\ttask\tFix the login\n"
    );
}

#[test]
fn a_deleted_item_leaves_the_list() {
    let (home, _daemon) = with_app();
    home.cockpit_ok(&["todo", "add", "Fix the login", "--project", "app"]);

    let deleted = home.cockpit_ok(&["todo", "delete", "1"]);

    assert_eq!(deleted, "deleted\t1\n");
    assert_eq!(home.cockpit_ok(&["todo", "list", "--all"]), "");
}

#[test]
fn an_image_added_to_an_item_comes_back_byte_for_byte() {
    let (home, _daemon) = with_app();
    home.cockpit_ok(&["todo", "add", "Fix the login", "--project", "app"]);
    let png: Vec<u8> = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0, 1, 2, 255]].concat();
    let image = home.home().join("shot.png");
    std::fs::write(&image, &png).unwrap();
    let copy = home.home().join("copy.png");

    let added = home.cockpit_ok(&["todo", "image", "add", "1", image.to_str().unwrap()]);
    home.cockpit_ok(&["todo", "image", "get", "1", "1", copy.to_str().unwrap()]);

    assert_eq!(added, "item 1: [Image 1] image/png 12 bytes\n");
    assert_eq!(std::fs::read(copy).unwrap(), png);
}

#[test]
fn a_draft_is_marked_stale_once_its_item_changes() {
    let (home, _daemon) = with_app();
    home.cockpit_ok(&["todo", "add", "Fix the login", "--project", "app"]);
    home.cockpit_ok(&[
        "todo",
        "draft",
        "save",
        "1",
        "--title",
        "Fix the login page",
    ]);

    let fresh = home.cockpit_ok(&["todo", "drafts"]);
    home.cockpit_ok(&["todo", "edit", "1", "--notes", "the page, not the API"]);
    let stale = home.cockpit_ok(&["todo", "drafts"]);

    assert_eq!(fresh, "item:1\tFix the login page\n");
    assert_eq!(stale, "item:1\tFix the login page\tstale\n");
}

#[test]
fn the_owner_cannot_file_in_an_unregistered_project() {
    let (home, _daemon) = with_app();

    let refusal = home.cockpit_err(&["todo", "add", "Anything", "--project", "nope"]);

    assert!(refusal.contains("no registered project nope"), "{refusal}");
}

#[test]
fn a_flag_another_verb_takes_is_a_usage_error() {
    let home = TestHome::new();

    let output = home.cockpit(&["todo", "add", "Fix the login", "--all"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("unknown argument --all"));
}

/// A home with only the agent socket, as a sandbox sees it.
struct Sandbox {
    dir: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        Sandbox {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn agent_socket(&self) -> PathBuf {
        let path = self.dir.path().join("run/cockpit-dev/agent/agent.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    /// Runs `cockpit` as an agent launched with `the-token`, off the test's runtime threads.
    async fn cockpit(&self, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"))
            .env("COCKPIT_LAUNCH_TOKEN", "the-token");
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn proposed_view() -> TaskView {
    TaskView {
        id: 4,
        project: "app".to_owned(),
        title: "Cache the build".to_owned(),
        notes: String::new(),
        kind: TaskKindView::Improvement,
        priority: 2,
        status: TaskStatusView::Proposed,
        is_blocker_report: false,
        why: Some("every run rebuilds".to_owned()),
        filed_by: Some("s1".to_owned()),
        filed_from: Some("app".to_owned()),
        doing_session: None,
        summary: None,
        is_queued: false,
        waits: Vec::new(),
        images: Vec::new(),
        intent_revision: 1,
        state_revision: 1,
        created_at: 10,
        changed_at: 10,
        next: TaskNextView::DecideProposal,
    }
}

#[tokio::test]
async fn a_session_proposes_on_the_agent_socket_with_its_launch_token() {
    let sandbox = Sandbox::new();
    let daemon = scripted_daemon(
        &sandbox.agent_socket(),
        Outcome::Ok(Reply::Task {
            task: Box::new(proposed_view()),
        }),
    );

    let output = sandbox
        .cockpit(&[
            "todo",
            "propose",
            "Cache the build",
            "--why",
            "every run rebuilds",
            "--kind",
            "improvement",
        ])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::TaskPropose {
            launch_token: Some("the-token".to_owned()),
            project: None,
            title: "Cache the build".to_owned(),
            why: "every run rebuilds".to_owned(),
            notes: String::new(),
            item_kind: Some(TaskKindView::Improvement),
        }
    );
    assert_eq!(
        the_line(&text(&output.stdout), "next:"),
        "next: the owner approves or declines the proposal"
    );
}

#[tokio::test]
async fn a_launched_session_adds_on_the_agent_socket() {
    let sandbox = Sandbox::new();
    let daemon = scripted_daemon(
        &sandbox.agent_socket(),
        Outcome::Ok(Reply::Tasks { tasks: Vec::new() }),
    );

    let output = sandbox.cockpit(&["todo", "list", "--all"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::TaskList {
            launch_token: Some("the-token".to_owned()),
            project: None,
            all: true,
        }
    );
}

#[tokio::test]
async fn a_refused_blocker_report_says_who_unblocks_it() {
    let sandbox = Sandbox::new();
    let _daemon = scripted_daemon(
        &sandbox.agent_socket(),
        Outcome::Error(ProtocolError {
            code: ErrorCode::Refused,
            message: "no project takes blocker reports".to_owned(),
            unblock: Some(Unblock {
                action: "set tasks.blocker_project in the owner configuration".to_owned(),
                by: UnblockedBy::Owner,
            }),
        }),
    );

    let output = sandbox
        .cockpit(&[
            "todo",
            "blocked",
            "Reach the docker socket",
            "--why",
            "permission denied",
        ])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "cockpit: refused: no project takes blocker reports\n\
         cockpit: to unblock (the owner): set tasks.blocker_project in the owner configuration\n"
    );
}

#[tokio::test]
async fn a_blocker_report_without_why_is_a_usage_error() {
    let sandbox = Sandbox::new();

    let output = sandbox
        .cockpit(&["todo", "blocked", "Reach the docker socket"])
        .await;

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("--why is required"));
}
