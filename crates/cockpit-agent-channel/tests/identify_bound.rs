//! Issue #174: a status poll identifies its sender in a read transaction, so it never waits
//! for the writer. Real processes on a real state database.
#![cfg(target_os = "linux")]

mod common;

use cockpit_agent_channel::{
    Error, PeerAncestry, ProcessError, Refusal, identify, identify_bound, mint,
};
use cockpit_authorization::SessionId;
use cockpit_turso::Database;
use common::{Process, launch, open, session, this_process, this_process_identity};

async fn bind(database: &Database, peer: &PeerAncestry, token: &str) {
    let write = database.write().await.unwrap();
    identify(&write, peer, Some(token)).await.unwrap();
    write.commit().await.unwrap();
}

async fn identify_read_only(
    database: &Database,
    peer: &PeerAncestry,
    token: Option<&str>,
) -> Result<Option<SessionId>, Error> {
    let read = database.read().await.unwrap();
    identify_bound(&read, peer, token).await
}

#[tokio::test]
async fn a_bound_launch_identifies_while_another_writer_holds_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let token = launch(&database, &session("s1"), &this_process_identity()).await;
    bind(&database, &this_process(), token.as_str()).await;
    let _writer = database.write().await.unwrap();

    let identified = identify_read_only(&database, &this_process(), None).await;

    assert_eq!(identified.unwrap(), Some(session("s1")));
}

#[tokio::test]
async fn a_pending_token_is_left_unspent_for_identify_to_bind() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let token = launch(&database, &session("s1"), &this_process_identity()).await;

    let read_only = identify_read_only(&database, &this_process(), Some(token.as_str())).await;
    let write = database.write().await.unwrap();
    let bound = identify(&write, &this_process(), Some(token.as_str())).await;

    assert_eq!(read_only.unwrap(), None);
    assert_eq!(bound.unwrap(), session("s1"));
}

#[tokio::test]
async fn a_relaunch_pending_its_new_token_is_left_to_identify() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let first = launch(&database, &session("s1"), &this_process_identity()).await;
    bind(&database, &this_process(), first.as_str()).await;
    let second = launch(&database, &session("s1"), &this_process_identity()).await;

    let read_only = identify_read_only(&database, &this_process(), Some(second.as_str())).await;

    assert_eq!(read_only.unwrap(), None);
}

/// Each case refuses the same way read-only as through [`identify`], so a leaked or early
/// token never sends a poll to the writer.
#[tokio::test]
async fn it_refuses_as_identify_does() {
    let dir = tempfile::tempdir().unwrap();
    let database = open(dir.path()).await;
    let agent = Process::sleeper();
    let spent = launch(&database, &session("s1"), &agent.identity()).await;
    bind(&database, &agent.ancestry(), spent.as_str()).await;
    let other_agent = Process::sleeper();
    let leaked = launch(&database, &session("s2"), &other_agent.identity()).await;
    let unstarted = {
        let write = database.write().await.unwrap();
        let token = mint(&write, &session("s3")).await.unwrap();
        write.commit().await.unwrap();
        token
    };
    let unverifiable: PeerAncestry = Err(ProcessError::Gone { pid: 1 });
    let cases: [(&str, PeerAncestry, Option<&str>, Refusal); 6] = [
        (
            "no token outside every tree",
            this_process(),
            None,
            Refusal::Unidentified,
        ),
        (
            "a spent token outside its tree",
            this_process(),
            Some(spent.as_str()),
            Refusal::UnknownLaunchToken,
        ),
        (
            "a malformed token",
            this_process(),
            Some("not-a-token"),
            Refusal::UnknownLaunchToken,
        ),
        (
            "a pending token outside its tree",
            this_process(),
            Some(leaked.as_str()),
            Refusal::OutsideLaunchTree,
        ),
        (
            "a pending token before its root is recorded",
            this_process(),
            Some(unstarted.as_str()),
            Refusal::LaunchNotStarted {
                session: session("s3"),
            },
        ),
        (
            "an unverifiable peer",
            unverifiable.clone(),
            Some(spent.as_str()),
            Refusal::PeerUnverifiable(ProcessError::Gone { pid: 1 }),
        ),
    ];

    for (case, peer, token, expected) in cases {
        let read_only = identify_read_only(&database, &peer, token).await;
        let write = database.write().await.unwrap();
        let written = identify(&write, &peer, token).await;
        write.rollback().await.unwrap();

        assert!(
            matches!(read_only, Err(Error::Refused(ref refusal)) if *refusal == expected),
            "{case}: read-only gave {read_only:?}"
        );
        assert!(
            matches!(written, Err(Error::Refused(ref refusal)) if *refusal == expected),
            "{case}: identify gave {written:?}"
        );
    }
}
