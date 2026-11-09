//! `cockpit agent sessions [--watch]` driven as a process against a scripted daemon socket
//! whose listing changes between reads (#707). A watch ends here when the scripted daemon
//! closes the connection after its last listing.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    AgentState, ObservedSessionView, Outcome, Reply, SessionReport, SessionStateView,
};
use scripted::{scripted_conversation, text};

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

    fn agent_socket(&self) -> PathBuf {
        let path = self.runtime_dir().join("torlek-dev/agent/agent.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    /// Runs `cockpit` with `arguments` in this home, off the test's runtime threads.
    async fn cockpit(&self, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.runtime_dir());
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn observed(id: &str, doing: &str) -> ObservedSessionView {
    ObservedSessionView {
        id: id.to_owned(),
        agent: "claude".to_owned(),
        state: SessionStateView::Running,
        working_dir: format!("/work/{id}"),
        report: Some(SessionReport {
            compaction: None,
            session: id.to_owned(),
            state: Some(AgentState::Working),
            label: None,
            doing: Some(doing.to_owned()),
            needs_input: None,
            outcome: None,
            question_source: None,
            covered_question: None,
            held_by: None,
            reported_at: 1,
        }),
        inbound_message: None,
        harness_trust: None,
    }
}

fn listing(sessions: Vec<ObservedSessionView>) -> Outcome {
    Outcome::Ok(Reply::AgentSessions { sessions })
}

#[tokio::test]
async fn a_watch_prints_only_the_session_whose_line_changed() {
    let home = Home::new();
    let _daemon = scripted_conversation(
        &home.agent_socket(),
        vec![
            listing(vec![observed("s1", "tests"), observed("s2", "review")]),
            listing(vec![observed("s1", "tests"), observed("s2", "merging")]),
        ],
    );

    let output = home.cockpit(&["agent", "sessions", "--watch"]).await;

    assert_eq!(
        text(&output.stdout),
        "s1\trunning\tclaude\tworking\t/work/s1\tdoing=\"tests\"\n\
         s2\trunning\tclaude\tworking\t/work/s2\tdoing=\"review\"\n\
         s2\trunning\tclaude\tworking\t/work/s2\tdoing=\"merging\"\n"
    );
}

#[tokio::test]
async fn a_watch_prints_a_new_session_and_says_when_one_ended() {
    let home = Home::new();
    let _daemon = scripted_conversation(
        &home.agent_socket(),
        vec![
            listing(vec![observed("s1", "tests"), observed("s2", "review")]),
            listing(vec![observed("s1", "tests"), observed("s3", "starting")]),
        ],
    );

    let output = home.cockpit(&["agent", "sessions", "--watch"]).await;

    assert_eq!(
        text(&output.stdout),
        "s1\trunning\tclaude\tworking\t/work/s1\tdoing=\"tests\"\n\
         s2\trunning\tclaude\tworking\t/work/s2\tdoing=\"review\"\n\
         s3\trunning\tclaude\tworking\t/work/s3\tdoing=\"starting\"\n\
         s2\tended\n"
    );
}

#[tokio::test]
async fn without_watch_every_session_is_listed_once() {
    let home = Home::new();
    let _daemon = scripted_conversation(
        &home.agent_socket(),
        vec![listing(vec![
            observed("s1", "tests"),
            observed("s2", "review"),
        ])],
    );

    let output = home.cockpit(&["agent", "sessions"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s1\trunning\tclaude\tworking\t/work/s1\tdoing=\"tests\"\n\
         s2\trunning\tclaude\tworking\t/work/s2\tdoing=\"review\"\n"
    );
}

#[tokio::test]
async fn the_latest_launch_harness_trust_note_is_listed_with_its_session() {
    let home = Home::new();
    let mut withheld = observed("s1", "tests");
    withheld.harness_trust = Some("not recorded: its settings differ".to_owned());
    let _daemon = scripted_conversation(&home.agent_socket(), vec![listing(vec![withheld])]);

    let output = home.cockpit(&["agent", "sessions"]).await;

    assert_eq!(
        text(&output.stdout),
        "s1\trunning\tclaude\tworking\t/work/s1\tdoing=\"tests\"\
         \tharness-trust=\"not recorded: its settings differ\"\n"
    );
}
