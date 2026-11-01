//! Issue #410: a coordinator's request to start a session waits in the `ask run` queue,
//! bound like a command, decided only while its `start-sessions` grant covers the session,
//! and finished by the daemon's start instead of a run.

mod common;

use cockpit_approvals::{
    Error, Refusal, RunRequest, RunState, StartAsk, StartSession, approve_run, ask_start, edit_run,
    finish_start, interrupted_runs, open_runs, reconcile_run, run_request,
};
use cockpit_authorization::{Permission, Scope, SessionId};
use common::{Fixture, NOW, session};

const AFTER_GRACE: i64 = NOW + cockpit_approvals::APPROVAL_GRACE_SECONDS;

fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> Refusal {
    match result {
        Err(Error::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn start(prompt: Option<&str>, clone: Option<&str>) -> StartSession {
    StartSession {
        prompt: prompt.map(str::to_owned),
        clone: clone.map(str::to_owned),
    }
}

/// `coordinator`, granted `start-sessions`, asks to start `start` in the fixture's directory.
async fn asked_start(
    fixture: &Fixture,
    coordinator: &SessionId,
    start: StartSession,
) -> RunRequest {
    let write = fixture.database.write().await.unwrap();
    let grant = cockpit_authorization::grant(
        &write,
        coordinator,
        Permission::StartSessions,
        &Scope::Owner,
    )
    .await
    .unwrap();
    let run = ask_start(
        &write,
        coordinator,
        StartAsk {
            dir: fixture.cwd(),
            start,
            reason: "session coordinator asks to start a worker".to_owned(),
            project: None,
        },
        grant.id,
        NOW,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    run
}

#[tokio::test]
async fn a_start_request_reads_as_the_session_new_command_it_stands_for() {
    let fixture = Fixture::new().await;

    let run = asked_start(
        &fixture,
        &session("coordinator"),
        start(Some("it's the review"), Some("feature/x")),
    )
    .await;

    assert_eq!(
        run.command,
        format!(
            "cockpit session new '{}' --prompt 'it'\\''s the review' --clone 'feature/x'",
            fixture.cwd().display()
        )
    );
    assert_eq!(
        run.start,
        Some(start(Some("it's the review"), Some("feature/x")))
    );
    let read = fixture.database.read().await.unwrap();
    assert_eq!(
        run_request(&read, run.id, NOW).await.unwrap(),
        Some(run.clone()),
        "the returned request is the stored one"
    );
    assert_eq!(open_runs(&read, NOW).await.unwrap(), vec![run]);
}

#[tokio::test]
async fn an_approved_start_is_finished_with_the_session_it_started() {
    let fixture = Fixture::new().await;
    let run = asked_start(&fixture, &session("coordinator"), start(None, None)).await;
    let write = fixture.database.write().await.unwrap();
    let (approved, _) = approve_run(&write, run.id, run.revision, &run.digest(), AFTER_GRACE)
        .await
        .unwrap();

    finish_start(&write, &approved, "s9", AFTER_GRACE)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let read = fixture.database.read().await.unwrap();
    let finished = run_request(&read, run.id, AFTER_GRACE)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(finished.state, RunState::Finished);
    let result = finished.result.expect("a finished start has a result");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output.shown, "started session s9");
}

#[tokio::test]
async fn a_start_request_is_never_edited() {
    let fixture = Fixture::new().await;
    let run = asked_start(&fixture, &session("coordinator"), start(None, None)).await;
    let write = fixture.database.write().await.unwrap();

    let refused = refusal(edit_run(&write, run.id, run.revision, "rm -rf /", AFTER_GRACE).await);

    assert!(
        matches!(
            refused,
            Refusal::InvalidRequest {
                field: "command",
                ..
            }
        ),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_start_request_leaves_the_queue_once_its_start_grant_is_revoked() {
    let fixture = Fixture::new().await;
    let coordinator = session("coordinator");
    let run = asked_start(&fixture, &coordinator, start(None, None)).await;
    fixture.revoke_all(&coordinator).await;
    let write = fixture.database.write().await.unwrap();

    assert!(open_runs(&write, AFTER_GRACE).await.unwrap().is_empty());
    let refused =
        refusal(approve_run(&write, run.id, run.revision, &run.digest(), AFTER_GRACE).await);
    assert_eq!(refused, Refusal::AskerMayNotAsk);
}

#[tokio::test]
async fn a_run_commands_grant_does_not_keep_a_start_request_decidable() {
    let fixture = Fixture::new().await;
    let coordinator = session("coordinator");
    fixture.grant(&coordinator).await;
    let run = asked_start(&fixture, &coordinator, start(None, None)).await;
    let write = fixture.database.write().await.unwrap();
    let start_grant = cockpit_authorization::active_grant(
        &write,
        &coordinator,
        Permission::StartSessions,
        &Scope::Owner,
    )
    .await
    .unwrap()
    .unwrap();
    cockpit_authorization::revoke(&write, start_grant.id)
        .await
        .unwrap();

    let refused =
        refusal(approve_run(&write, run.id, run.revision, &run.digest(), AFTER_GRACE).await);

    assert_eq!(refused, Refusal::AskerMayNotAsk);
}

#[tokio::test]
async fn an_approved_start_a_restart_finds_is_not_run_never_started_again() {
    let fixture = Fixture::new().await;
    let run = asked_start(&fixture, &session("coordinator"), start(None, None)).await;
    let write = fixture.database.write().await.unwrap();
    approve_run(&write, run.id, run.revision, &run.digest(), AFTER_GRACE)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let write = fixture.database.write().await.unwrap();
    let interrupted = interrupted_runs(&write).await.unwrap();
    assert_eq!(interrupted.len(), 1);
    assert_eq!(interrupted[0].run.start, Some(start(None, None)));
    let state = reconcile_run(&write, &interrupted[0], None, AFTER_GRACE)
        .await
        .unwrap();

    assert_eq!(state, RunState::NotRun);
}

#[tokio::test]
async fn a_start_prompt_and_branch_are_validated_before_filing() {
    let fixture = Fixture::new().await;
    let coordinator = session("coordinator");
    let write = fixture.database.write().await.unwrap();
    let grant = cockpit_authorization::grant(
        &write,
        &coordinator,
        Permission::StartSessions,
        &Scope::Owner,
    )
    .await
    .unwrap();

    for (bad, field) in [
        (start(Some("  "), None), "prompt"),
        (start(None, Some("two words")), "clone"),
    ] {
        let refused = refusal(
            ask_start(
                &write,
                &coordinator,
                StartAsk {
                    dir: fixture.cwd(),
                    start: bad,
                    reason: "starts a worker".to_owned(),
                    project: None,
                },
                grant.id,
                NOW,
            )
            .await,
        );
        assert!(
            matches!(refused, Refusal::InvalidRequest { field: f, .. } if f == field),
            "{refused:?}"
        );
    }
}
