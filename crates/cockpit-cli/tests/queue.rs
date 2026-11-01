//! `cockpit queue` driven as a process against a scripted daemon socket. The rules are proven
//! in `cockpit-scheduling` and through the socket in `cockpit-daemon/tests/scheduling.rs`;
//! these prove each verb's request, its output and its exit status.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    Command, DemandView, ErrorCode, Outcome, ProtocolError, QueueExplanationView,
    QueuePlacementView, QueueSessionNoteView, QueueStepView, Reply,
};
use scripted::{scripted_daemon, text};

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

    fn daemon_socket(&self) -> PathBuf {
        let path = self.runtime_dir().join("cockpit/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    async fn cockpit(&self, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
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

const EVALUATED: i64 = 1_800_003_600;

/// A step no session of its project can take, in an unfamiliar project.
fn step_without_a_free_session() -> QueueExplanationView {
    QueueExplanationView {
        item: 4,
        project: "web".to_owned(),
        title: "fix login".to_owned(),
        step: Some(QueueStepView {
            run: 2,
            position: 1,
            name: "implement".to_owned(),
        }),
        reason: "no-session-free".to_owned(),
        why: "none of the 2 sessions of project web can take it now".to_owned(),
        limited_by: None,
        sessions: vec![
            QueueSessionNoteView {
                session: "s0123456789ab".to_owned(),
                why: "its agent is working".to_owned(),
            },
            QueueSessionNoteView {
                session: "s0123456789ac".to_owned(),
                why: "you are viewing it".to_owned(),
            },
        ],
        behind: None,
        action: "wait: a later pass places it once a session is free".to_owned(),
        action_by: "cockpit".to_owned(),
        priority: 2,
        aging_boost: 4,
        demand: DemandView {
            cpu_millis: 750,
            memory_mib: 1536,
        },
        headroom_percent: 50,
        waiting_since: Some(EVALUATED - 3600),
        evaluated_at: EVALUATED,
        next_evaluation_at: EVALUATED + 30,
    }
}

/// An item outside the queue.
fn unqueued_item() -> QueueExplanationView {
    QueueExplanationView {
        item: 9,
        project: "api".to_owned(),
        title: "write docs".to_owned(),
        step: None,
        reason: "not-queued".to_owned(),
        why: "it is not queued".to_owned(),
        limited_by: None,
        sessions: vec![],
        behind: None,
        action: "queue it: `cockpit todo queue 9`".to_owned(),
        action_by: "owner".to_owned(),
        priority: 3,
        aging_boost: 0,
        demand: DemandView {
            cpu_millis: 0,
            memory_mib: 0,
        },
        headroom_percent: 0,
        waiting_since: None,
        evaluated_at: EVALUATED,
        next_evaluation_at: EVALUATED + 30,
    }
}

#[tokio::test]
async fn queue_explain_prints_one_block_per_waiting_item_in_dispatch_order() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::QueueExplanations {
            explanations: vec![step_without_a_free_session(), unqueued_item()],
        }),
    );

    let output = home.cockpit(&["queue", "explain"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "item 4 web: fix login\n\
         \x20 waits as step 1 implement of workflow run 2\n\
         \x20 no-session-free: none of the 2 sessions of project web can take it now\n\
         \x20 session s0123456789ab: its agent is working\n\
         \x20 session s0123456789ac: you are viewing it\n\
         \x20 priority 2 + 4 aging\tcharge 750 millicores 1536 MiB (50% startup headroom)\n\
         \x20 waited 60 min\tnext pass in 30 s\n\
         \x20 action (cockpit): wait: a later pass places it once a session is free\n\
         \n\
         item 9 api: write docs\n\
         \x20 not-queued: it is not queued\n\
         \x20 priority 3 + 0 aging\tcharge 0 millicores 0 MiB\n\
         \x20 next pass in 30 s\n\
         \x20 action (owner): queue it: `cockpit todo queue 9`\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::QueueExplain { item: None });
}

#[tokio::test]
async fn queue_explain_of_a_held_item_names_its_limit_and_what_it_waits_behind() {
    let home = Home::new();
    let explanation = QueueExplanationView {
        item: 5,
        project: "web".to_owned(),
        title: "small fix".to_owned(),
        step: None,
        reason: "behind".to_owned(),
        why: "item 3 waits for room first and gathers capacity".to_owned(),
        limited_by: Some("cpu in the shared budget".to_owned()),
        behind: Some(3),
        action: "wait: it starts once item 3 has started".to_owned(),
        action_by: "cockpit".to_owned(),
        aging_boost: 0,
        headroom_percent: 0,
        demand: DemandView {
            cpu_millis: 500,
            memory_mib: 1024,
        },
        waiting_since: Some(EVALUATED - 30),
        sessions: vec![],
        ..unqueued_item()
    };
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::QueueExplanations {
            explanations: vec![explanation],
        }),
    );

    let output = home.cockpit(&["queue", "explain", "5"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "item 5 web: small fix\n\
         \x20 behind: item 3 waits for room first and gathers capacity\n\
         \x20 limited by cpu in the shared budget\n\
         \x20 behind item 3\n\
         \x20 priority 3 + 0 aging\tcharge 500 millicores 1024 MiB\n\
         \x20 waited 30 s\tnext pass in 30 s\n\
         \x20 action (cockpit): wait: it starts once item 3 has started\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::QueueExplain { item: Some(5) }
    );
}

#[tokio::test]
async fn queue_explain_says_so_when_nothing_waits() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::QueueExplanations {
            explanations: vec![],
        }),
    );

    let output = home.cockpit(&["queue", "explain"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "nothing waits\n");
}

#[tokio::test]
async fn queue_explain_of_an_item_without_an_explanation_exits_1_with_the_daemons_reason() {
    let home = Home::new();
    let message = "item 5 has no explanation: the last pass found it placed, being done, \
                   finished or gone; `cockpit todo show 5` says which";
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Error(ProtocolError::new(ErrorCode::NotFound, message)),
    );

    let output = home.cockpit(&["queue", "explain", "5"]).await;

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains(message),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn queue_explain_refuses_an_item_that_is_not_a_number() {
    let output = Home::new().cockpit(&["queue", "explain", "five"]).await;

    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).starts_with("cockpit: five: not an item number\n"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn queue_dispatch_prints_what_the_pass_placed_and_ended() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::QueueDispatched {
            placed: vec![
                QueuePlacementView {
                    id: 7,
                    item: 4,
                    project: "web".to_owned(),
                    step: None,
                    session: "s0123456789ab".to_owned(),
                    demand: DemandView {
                        cpu_millis: 500,
                        memory_mib: 1024,
                    },
                    placed_at: EVALUATED,
                },
                QueuePlacementView {
                    id: 8,
                    item: 6,
                    project: "api".to_owned(),
                    step: Some(QueueStepView {
                        run: 3,
                        position: 2,
                        name: "check".to_owned(),
                    }),
                    session: "s0123456789ac".to_owned(),
                    demand: DemandView {
                        cpu_millis: 750,
                        memory_mib: 1536,
                    },
                    placed_at: EVALUATED,
                },
            ],
            ended: vec![5],
            waiting: 2,
        }),
    );

    let output = home.cockpit(&["queue", "dispatch"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "placed item 4 (web) on session s0123456789ab\tcharge 500 millicores 1024 MiB\n\
         placed step 2 check of workflow run 3 of item 6 (api) on session s0123456789ac\tcharge 750 millicores 1536 MiB\n\
         ended placement 5\n\
         2 items wait: `cockpit queue explain` says why\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::QueueDispatch);
}
