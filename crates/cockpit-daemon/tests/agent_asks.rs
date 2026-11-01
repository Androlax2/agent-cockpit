//! Issue #286: an agent the daemon launched asks the owner through the real `cockpit ask`
//! binary and the agent socket, as soon as it starts; the ask reaches the owner's queue under
//! that session, and the owner's answer reaches the agent as `cockpit ask`'s exit code.
//! Issue #285: once the session ends, what it asked leaves the queue and cannot be approved.
//! Issue #347: the owner's show verbs read such an ask as expired, as the agent does.

mod real_daemon;

use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Command, ErrorCode, GuideStateView, GuideView, Reply, RunRequestView, RunStateView, SessionView,
};
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

/// The owner's queue as it reads now.
async fn queue(owner: &Connection) -> (Vec<RunRequestView>, Vec<GuideView>) {
    match owner.request(Command::ApprovalQueue, GENEROUS).await {
        Ok(Reply::ApprovalQueue { runs, guides }) => (runs, guides),
        other => panic!("approval-queue answered {other:?}"),
    }
}

async fn end(owner: &Connection, session: &SessionView) {
    owner
        .request(
            Command::SessionEnd {
                session: session.id.clone(),
            },
            GENEROUS,
        )
        .await
        .expect("the owner ends the session");
}

/// What a session's agent asked, as the owner's queue showed it.
struct Asked {
    run: Option<RunRequestView>,
    guide: Option<GuideView>,
}

/// A session of `project` whose agent's ask reached the owner's queue, then ended.
async fn ended_after_asking(
    home: &Home,
    owner: &Connection,
    project: &str,
) -> (SessionView, Asked) {
    let (root, _) = home.project(owner, project).await;
    let session = new_session(owner, &root).await;
    let asked = queued(home, owner, |runs, guides| {
        let run = runs.into_iter().find(|run| run.session == session.id);
        let guide = guides.into_iter().find(|guide| guide.session == session.id);
        (run.is_some() || guide.is_some()).then_some(Asked { run, guide })
    })
    .await;
    end(owner, &session).await;
    (session, asked)
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

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_run_of_a_session_that_ended_leaves_the_owners_queue() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (session, _) = ended_after_asking(&home, &owner, ASK_RUN_PROJECT).await;

    let (runs, _) = queue(&owner).await;
    assert!(runs.iter().all(|run| run.session != session.id), "{runs:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_run_of_a_session_that_ended_cannot_be_approved() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (_, asked) = ended_after_asking(&home, &owner, ASK_RUN_PROJECT).await;
    let run = asked.run.expect("the agent asked to run a command");

    let approved = owner
        .request(
            Command::ApprovalApprove {
                request: run.id,
                revision: run.revision,
                digest: run.digest.clone(),
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(&approved, Err(ClientError::Daemon(error)) if error.code == ErrorCode::Conflict),
        "{approved:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_guide_of_a_session_that_ended_leaves_the_owners_queue() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (session, _) = ended_after_asking(&home, &owner, ASK_GUIDE_PROJECT).await;

    let (_, guides) = queue(&owner).await;
    assert!(
        guides.iter().all(|guide| guide.session != session.id),
        "{guides:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_run_of_a_session_that_ended_shows_as_expired_without_build_files() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (_, asked) = ended_after_asking(&home, &owner, ASK_RUN_PROJECT).await;
    let run = asked.run.expect("the agent asked to run a command");

    let shown = match owner
        .request(Command::ApprovalShow { request: run.id }, GENEROUS)
        .await
    {
        Ok(Reply::RunRequest { request }) => request,
        other => panic!("approval-show answered {other:?}"),
    };

    assert_eq!(
        (shown.state, shown.changed_build_files),
        (RunStateView::Expired, None)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_guide_of_a_session_that_ended_shows_as_expired() {
    assert_the_cli_is_built();
    let home = Home::new();
    let _daemon = home.start();
    let owner = home.connect().await;
    let (_, asked) = ended_after_asking(&home, &owner, ASK_GUIDE_PROJECT).await;
    let guide = asked.guide.expect("the agent asked for a guide");

    let shown = match owner
        .request(Command::GuideShow { guide: guide.id }, GENEROUS)
        .await
    {
        Ok(Reply::Guide { guide }) => guide,
        other => panic!("guide-show answered {other:?}"),
    };

    assert_eq!(shown.state, GuideStateView::Expired);
}
