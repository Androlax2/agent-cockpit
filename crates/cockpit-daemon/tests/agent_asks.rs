//! Issue #286: an agent the daemon launched asks the owner through the real `cockpit ask`
//! binary and the agent socket, as soon as it starts; the ask reaches the owner's queue under
//! that session, and the owner's answer reaches the agent as `cockpit ask`'s exit code.

mod real_daemon;

use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_protocol::{Command, GuideView, Reply, RunRequestView};
use real_daemon::{
    ASK_GUIDE_PROJECT, ASK_RUN_PROJECT, ASKED_COMMAND, GENEROUS, Home, cockpit_program, new_session,
};

/// `cockpit ask`'s exit code when the owner declined or closed the ask.
const DECLINED: &str = "125";

fn assert_the_cli_is_built() {
    let cockpit = cockpit_program();
    assert!(
        cockpit.is_file(),
        "{} is missing: run the workspace's tests (`make crates-test`), which build it",
        cockpit.display()
    );
}

/// The owner's queue once it holds an ask of `session` that `pick` finds.
async fn queued<T>(
    home: &Home,
    owner: &Connection,
    pick: impl Fn(Vec<RunRequestView>, Vec<GuideView>) -> Option<T>,
) -> T {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let (runs, guides) = match owner.request(Command::ApprovalQueue, GENEROUS).await {
            Ok(Reply::ApprovalQueue { runs, guides }) => (runs, guides),
            other => panic!("approval-queue answered {other:?}"),
        };
        if let Some(found) = pick(runs, guides) {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "the agent's ask did not reach the owner's queue; it printed {:?}",
            std::fs::read_to_string(home.ask_log()).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The exit code the agent's `cockpit ask <verb>` ended with, from its `<verb>=<code>` line.
async fn exit_code_of(home: &Home, verb: &str) -> String {
    let prefix = format!("{verb}=");
    let deadline = Instant::now() + GENEROUS;
    loop {
        let log = std::fs::read_to_string(home.ask_log()).unwrap_or_default();
        if let Some(code) = log.lines().find_map(|line| line.strip_prefix(&prefix)) {
            return code.to_owned();
        }
        assert!(
            Instant::now() < deadline,
            "the agent's `cockpit ask {verb}` did not end; it printed {log:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launched_agents_ask_run_reaches_the_owner_and_hears_the_decline() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (root, project) = home.project(&owner, ASK_RUN_PROJECT).await;
    let session = new_session(&owner, &root).await;

    let request = queued(&home, &owner, |runs, _| {
        runs.into_iter().find(|run| run.session == session.id)
    })
    .await;
    assert_eq!(request.command, ASKED_COMMAND);
    assert_eq!(request.project.as_deref(), Some(project.as_str()));
    owner
        .request(
            Command::ApprovalDecline {
                request: request.id,
            },
            GENEROUS,
        )
        .await
        .expect("the owner declines the ask");

    assert_eq!(exit_code_of(&home, "run").await, DECLINED);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launched_agents_ask_guide_reaches_the_owner_and_hears_the_close() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (root, _) = home.project(&owner, ASK_GUIDE_PROJECT).await;
    let session = new_session(&owner, &root).await;

    let guide = queued(&home, &owner, |_, guides| {
        guides.into_iter().find(|guide| guide.session == session.id)
    })
    .await;
    assert_eq!(guide.steps.len(), 1);
    owner
        .request(Command::GuideClose { guide: guide.id }, GENEROUS)
        .await
        .expect("the owner closes the guide");

    assert_eq!(exit_code_of(&home, "guide").await, DECLINED);
}
