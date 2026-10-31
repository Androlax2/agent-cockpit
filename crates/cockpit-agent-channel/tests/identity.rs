//! Issue #32: who a sender is comes from its launch token and its verified process tree, never
//! from a payload. Real processes on a real state database.
#![cfg(target_os = "linux")]

mod common;

use cockpit_agent_channel::{
    Error, ProcessError, ProcessIdentity, Refusal, ancestry, ancestry_of, mint,
};
use common::{
    Process, identify_committed, launch, open, refusal, session, this_process,
    this_process_identity,
};

#[tokio::test]
async fn a_token_binds_from_its_root_and_then_the_tree_alone_identifies() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let token = launch(&database, &session("s1"), &this_process_identity()).await;

    let bound = identify_committed(&database, &this_process(), Some(token.as_str())).await;
    let later = identify_committed(&database, &this_process(), None).await;

    assert_eq!(bound.unwrap(), session("s1"));
    assert_eq!(later.unwrap(), session("s1"));
}

#[tokio::test]
async fn a_spent_token_identifies_nobody_outside_the_tree() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let agent = Process::sleeper();
    let token = launch(&database, &session("s1"), &agent.identity()).await;
    identify_committed(&database, &agent.ancestry(), Some(token.as_str()))
        .await
        .unwrap();

    let replayed = identify_committed(&database, &this_process(), Some(token.as_str())).await;

    assert_eq!(refusal(replayed), Refusal::UnknownLaunchToken);
}

#[tokio::test]
async fn a_token_presented_from_outside_its_tree_is_refused_and_stays_unspent() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let agent = Process::shell("sleep 60");
    let token = launch(&database, &session("s1"), &agent.identity()).await;
    let agent_child = ancestry(agent.first_child());

    let leaked = identify_committed(&database, &this_process(), Some(token.as_str())).await;
    let genuine = identify_committed(&database, &agent_child, Some(token.as_str())).await;

    assert_eq!(refusal(leaked), Refusal::OutsideLaunchTree);
    assert_eq!(genuine.unwrap(), session("s1"));
}

#[tokio::test]
async fn a_descendant_of_a_bound_root_acts_as_that_session() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let agent = Process::shell("sleep 60");
    let token = launch(&database, &session("s1"), &agent.identity()).await;
    identify_committed(&database, &agent.ancestry(), Some(token.as_str()))
        .await
        .unwrap();

    let nested = identify_committed(&database, &ancestry(agent.first_child()), None).await;

    assert_eq!(nested.unwrap(), session("s1"));
}

#[tokio::test]
async fn the_nearest_bound_root_names_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let outer = this_process().unwrap()[1].clone();
    let outer_token = launch(&database, &session("outer"), &outer).await;
    let inner_token = launch(&database, &session("inner"), &this_process_identity()).await;
    identify_committed(&database, &this_process(), Some(outer_token.as_str()))
        .await
        .unwrap();
    identify_committed(&database, &this_process(), Some(inner_token.as_str()))
        .await
        .unwrap();

    let identified = identify_committed(&database, &this_process(), None).await;

    assert_eq!(identified.unwrap(), session("inner"));
}

#[tokio::test]
async fn a_reused_pid_with_another_start_time_matches_no_launch() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let me = this_process_identity();
    let earlier_process = ProcessIdentity {
        start: me.start - 1,
        ..me
    };
    let token = launch(&database, &session("s1"), &earlier_process).await;

    let with_token = identify_committed(&database, &this_process(), Some(token.as_str())).await;

    assert_eq!(refusal(with_token), Refusal::OutsideLaunchTree);
}

#[tokio::test]
async fn a_process_in_no_bound_tree_without_a_token_is_unidentified() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    launch(&database, &session("s1"), &this_process_identity()).await;

    let identified = identify_committed(&database, &this_process(), None).await;

    assert_eq!(refusal(identified), Refusal::Unidentified);
}

#[tokio::test]
async fn a_malformed_token_is_unknown_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;

    let identified = identify_committed(&database, &this_process(), Some("s1")).await;

    assert_eq!(refusal(identified), Refusal::UnknownLaunchToken);
}

#[tokio::test]
async fn a_token_before_its_root_is_recorded_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let write = database.write().await.unwrap();
    let token = mint(&write, &session("s1")).await.unwrap();
    write.commit().await.unwrap();

    let identified = identify_committed(&database, &this_process(), Some(token.as_str())).await;

    assert_eq!(
        refusal(identified),
        Refusal::LaunchNotStarted {
            session: session("s1")
        }
    );
}

#[tokio::test]
async fn launching_a_session_again_ends_its_previous_binding() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let first = launch(&database, &session("s1"), &this_process_identity()).await;
    identify_committed(&database, &this_process(), Some(first.as_str()))
        .await
        .unwrap();
    let agent = Process::sleeper();
    launch(&database, &session("s1"), &agent.identity()).await;

    let old_tree = identify_committed(&database, &this_process(), None).await;

    assert_eq!(refusal(old_tree), Refusal::Unidentified);
}

#[tokio::test]
async fn a_binding_survives_a_daemon_restart() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let token = launch(&database, &session("s1"), &this_process_identity()).await;
    identify_committed(&database, &this_process(), Some(token.as_str()))
        .await
        .unwrap();
    drop(database);
    let reopened = open(dir.path()).await;

    let identified = identify_committed(&reopened, &this_process(), None).await;

    assert_eq!(identified.unwrap(), session("s1"));
}

#[tokio::test]
async fn an_unreadable_sender_is_refused_as_unverifiable() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let gone = Err(ProcessError::Gone { pid: 1 });

    let identified = identify_committed(&database, &gone, None).await;

    assert_eq!(
        refusal(identified),
        Refusal::PeerUnverifiable(ProcessError::Gone { pid: 1 })
    );
}

#[test]
fn the_ancestry_of_an_exited_process_is_gone() {
    let mut exited = std::process::Command::new("true").spawn().unwrap();
    let pid = exited.id();
    exited.wait().unwrap();

    assert_eq!(ancestry(pid), Err(ProcessError::Gone { pid }));
}

#[test]
fn a_pid_now_naming_a_later_process_is_gone() {
    let mut connector = this_process_identity();
    connector.start -= 1;

    assert_eq!(
        ancestry_of(&connector),
        Err(ProcessError::Gone { pid: connector.pid })
    );
}

#[test]
fn the_ancestry_of_a_live_process_starts_at_it() {
    let process = this_process_identity();

    assert_eq!(ancestry_of(&process).unwrap()[0], process);
}

#[test]
fn the_ancestry_runs_from_the_process_up_to_a_root_without_parent() {
    let chain = this_process().unwrap();

    assert_eq!(chain[0].pid, std::process::id());
    assert!(
        chain.windows(2).all(|pair| pair[1].start <= pair[0].start),
        "{chain:?}"
    );
}

#[tokio::test]
async fn a_process_roots_at_most_one_launch() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    launch(&database, &session("s1"), &this_process_identity()).await;
    let write = database.write().await.unwrap();
    cockpit_agent_channel::mint(&write, &session("s2"))
        .await
        .unwrap();

    let recorded =
        cockpit_agent_channel::record_root(&write, &session("s2"), &this_process_identity()).await;

    assert!(matches!(recorded, Err(Error::Database(_))), "{recorded:?}");
}
