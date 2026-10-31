//! The run request state machine on a real state database: binding, consume-once, expiry,
//! fencing and restart, with report 04's stale-approval walkthrough.

mod common;

use cockpit_approvals::{
    Approved, EXPIRES_AFTER_SECONDS, Error, Interrupted, Output, Refusal, RunId, RunRequest,
    RunResult, RunState, action_digest, approve_run, ask_run, decline_run, edit_run, finish_run,
    interrupted_runs, open_runs, queue_order, reconcile_run, record_not_run,
    record_outcome_unknown, resolve_cwd, run_request, run_status, start_run,
};
use cockpit_authorization::Permission;
use cockpit_processes::{EndReason, ExecutionId, Phase};
use common::{Fixture, NOW, session};

/// The execution a started run names; these tests never launch it.
const EXECUTION: ExecutionId = ExecutionId(7);

fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> Refusal {
    match result {
        Err(Error::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn result(exit_code: i32) -> RunResult {
    RunResult {
        exit_code,
        output: Output {
            shown: "done".to_owned(),
            omitted_lines: 0,
            truncated_bytes: 0,
            redacted: 0,
        },
    }
}

async fn approve(fixture: &Fixture, id: cockpit_approvals::RunId) -> Approved {
    let write = fixture.database.write().await.unwrap();
    let run = run_request(&write, id, NOW).await.unwrap().unwrap();
    let (approved, _) = approve_run(&write, id, run.revision, &run.digest(), AFTER_GRACE)
        .await
        .unwrap();
    write.commit().await.unwrap();
    approved
}

/// The first moment a request asked at `NOW` may be approved.
const AFTER_GRACE: i64 = NOW + cockpit_approvals::APPROVAL_GRACE_SECONDS;

#[tokio::test]
async fn an_approval_right_after_the_ask_is_refused_until_the_grace_has_passed() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();

    let at_once = refusal(approve_run(&write, run.id, 1, &run.digest(), NOW).await);
    let almost = refusal(approve_run(&write, run.id, 1, &run.digest(), AFTER_GRACE - 1).await);
    let approved = approve_run(&write, run.id, 1, &run.digest(), AFTER_GRACE).await;

    assert_eq!(
        at_once,
        Refusal::TooSoon {
            wait_seconds: cockpit_approvals::APPROVAL_GRACE_SECONDS
        }
    );
    assert_eq!(almost, Refusal::TooSoon { wait_seconds: 1 });
    assert!(approved.is_ok(), "{approved:?}");
}

#[tokio::test]
async fn an_edit_starts_the_grace_again() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    let edited_at = NOW + 60;
    let edited = edit_run(&write, run.id, 1, "ls -la", edited_at)
        .await
        .unwrap();

    let at_once = refusal(approve_run(&write, run.id, 2, &edited.digest(), edited_at).await);
    let later = approve_run(
        &write,
        run.id,
        2,
        &edited.digest(),
        edited_at + cockpit_approvals::APPROVAL_GRACE_SECONDS,
    )
    .await;

    assert!(matches!(at_once, Refusal::TooSoon { .. }), "{at_once:?}");
    assert!(later.is_ok(), "{later:?}");
}

#[tokio::test]
async fn a_session_without_the_run_commands_grant_is_refused_and_nothing_is_filed() {
    let fixture = Fixture::new().await;
    let write = fixture.database.write().await.unwrap();
    let refused = refusal(ask_run(&write, &session("s1"), fixture.ask("ls"), NOW).await);
    assert!(
        matches!(
            refused,
            Refusal::NotAuthorized(cockpit_authorization::Refusal::NoGrant {
                permission: Permission::RunCommands,
                ..
            })
        ),
        "{refused:?}"
    );
    drop(write);
    let read = fixture.database.read().await.unwrap();
    assert!(open_runs(&read, NOW).await.unwrap().is_empty());
}

#[tokio::test]
async fn an_asked_command_waits_for_the_owner_bound_to_its_digest() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls -la").await;
    assert_eq!(run.state, RunState::Asking);
    assert_eq!(run.revision, 1);
    assert_eq!(run.digest(), action_digest(&fixture.cwd(), "ls -la"));
    let read = fixture.database.read().await.unwrap();
    let open = open_runs(&read, NOW).await.unwrap();
    assert_eq!(open, [run]);
}

#[tokio::test]
async fn the_digest_changes_with_the_directory_as_well_as_the_command() {
    let cwd = std::path::Path::new("/tmp");
    let digest = action_digest(cwd, "ls");
    assert_eq!(digest.len(), 64);
    assert_ne!(digest, action_digest(cwd, "ls "));
    assert_ne!(digest, action_digest(std::path::Path::new("/"), "ls"));
    // The length prefix keeps the boundary between the two parts.
    assert_ne!(
        action_digest(std::path::Path::new("/a"), "b"),
        action_digest(std::path::Path::new("/"), "ab")
    );
}

#[tokio::test]
async fn an_approval_is_consumed_once() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let approved = approve(&fixture, run.id).await;
    assert_eq!(approved.command, "ls");

    let write = fixture.database.write().await.unwrap();
    let again = refusal(approve_run(&write, run.id, 1, &run.digest(), NOW).await);
    assert_eq!(again, Refusal::AlreadyDecided { state: "approved" });
}

/// Report 04, walkthrough 3: a client that read the request before the owner edited it
/// approves the old revision; the approval fails its binding, the client reads again and
/// the approval of what it now shows runs the edited command.
#[tokio::test]
async fn an_offline_approval_of_an_edited_request_fails_its_revision_then_the_replay_runs_the_edit()
{
    let fixture = Fixture::new().await;
    let stale_view = fixture.asked(&session("s1"), "rm -rf build").await;

    let write = fixture.database.write().await.unwrap();
    let edited = edit_run(&write, stale_view.id, 1, "rm -rf build/tmp", NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
    assert_eq!(edited.revision, 2);
    assert!(edited.was_edited());

    let write = fixture.database.write().await.unwrap();
    let refused = refusal(
        approve_run(
            &write,
            stale_view.id,
            stale_view.revision,
            &stale_view.digest(),
            NOW,
        )
        .await,
    );
    assert_eq!(refused, Refusal::StaleRevision { current: 2 });
    drop(write);

    let read = fixture.database.read().await.unwrap();
    let replayed = run_request(&read, stale_view.id, NOW)
        .await
        .unwrap()
        .unwrap();
    drop(read);
    assert_eq!(
        replayed.state,
        RunState::Asking,
        "the stale approval changed nothing"
    );
    let write = fixture.database.write().await.unwrap();
    let (approved, _) = approve_run(
        &write,
        replayed.id,
        replayed.revision,
        &replayed.digest(),
        AFTER_GRACE,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    assert_eq!(approved.command, "rm -rf build/tmp");
    assert_eq!(approved.revision, 2);
}

#[tokio::test]
async fn a_digest_other_than_the_current_revisions_is_refused() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    let other = action_digest(&fixture.cwd(), "ls -la");
    assert_eq!(
        refusal(approve_run(&write, run.id, 1, &other, NOW).await),
        Refusal::DigestMismatch
    );
}

#[tokio::test]
async fn a_request_nobody_asks_about_expires_and_cannot_be_approved() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let later = NOW + EXPIRES_AFTER_SECONDS;
    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        refusal(approve_run(&write, run.id, 1, &run.digest(), later).await),
        Refusal::Expired
    );
    assert!(open_runs(&write, later).await.unwrap().is_empty());
    let seen = run_status(&write, &session("s1"), run.id, later)
        .await
        .unwrap();
    assert_eq!(seen.state, RunState::Expired);
}

#[tokio::test]
async fn an_agent_waiting_for_its_answer_keeps_the_request_from_expiring() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let polled_at = NOW + EXPIRES_AFTER_SECONDS - 1;
    let write = fixture.database.write().await.unwrap();
    run_status(&write, &session("s1"), run.id, polled_at)
        .await
        .unwrap();
    let past_the_first_expiry = NOW + EXPIRES_AFTER_SECONDS + 60;
    approve_run(&write, run.id, 1, &run.digest(), past_the_first_expiry)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_poll_while_the_expiry_is_fresh_writes_nothing() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let before = fixture.revision().await;

    let write = fixture.database.write().await.unwrap();
    let seen = run_status(&write, &session("s1"), run.id, NOW + 59)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(seen, run);
    assert_eq!(fixture.revision().await, before);
}

#[tokio::test]
async fn a_poll_a_minute_after_the_last_refresh_moves_the_expiry() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let polled_at = NOW + 60;

    let write = fixture.database.write().await.unwrap();
    let seen = run_status(&write, &session("s1"), run.id, polled_at)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(seen.expires_at, polled_at + EXPIRES_AFTER_SECONDS);
    let read = fixture.database.read().await.unwrap();
    assert_eq!(
        run_request(&read, run.id, polled_at).await.unwrap(),
        Some(seen)
    );
}

/// The request `id` as stored at `NOW`, for comparing with what a mutation returned.
async fn stored(write: &cockpit_turso::WriteTransaction<'_>, id: RunId) -> RunRequest {
    run_request(write, id, NOW).await.unwrap().unwrap()
}

#[tokio::test]
async fn an_edit_returns_the_request_as_it_now_reads() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    let edited = edit_run(&write, run.id, 1, "ls -la", NOW).await.unwrap();
    assert_eq!(edited, stored(&write, run.id).await);
}

#[tokio::test]
async fn a_decline_returns_the_request_as_it_now_reads() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    let declined = decline_run(&write, run.id, NOW).await.unwrap();
    assert_eq!(declined, stored(&write, run.id).await);
}

#[tokio::test]
async fn an_approval_returns_the_request_as_it_now_reads() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    let (_, approved) = approve_run(&write, run.id, 1, &run.digest(), AFTER_GRACE)
        .await
        .unwrap();
    assert_eq!(approved, stored(&write, run.id).await);
}

#[tokio::test]
async fn a_declined_request_cannot_be_approved() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    let declined = decline_run(&write, run.id, NOW).await.unwrap();
    assert_eq!(declined.state, RunState::Declined);
    assert_eq!(
        refusal(approve_run(&write, run.id, 1, &run.digest(), NOW).await),
        Refusal::AlreadyDecided { state: "declined" }
    );
}

#[tokio::test]
async fn a_result_is_recorded_once_for_the_approved_revision() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let approved = approve(&fixture, run.id).await;
    let write = fixture.database.write().await.unwrap();
    start_run(&write, &approved, EXECUTION).await.unwrap();
    finish_run(&write, &approved, &result(0), NOW)
        .await
        .unwrap();

    let late = refusal(finish_run(&write, &approved, &result(1), NOW).await);
    assert_eq!(late, Refusal::AlreadyDecided { state: "finished" });
    let stored = run_request(&write, run.id, NOW).await.unwrap().unwrap();
    assert_eq!(stored.result.map(|result| result.exit_code), Some(0));
}

#[tokio::test]
async fn a_started_run_whose_end_was_not_observed_is_outcome_unknown() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "make deploy").await;
    let approved = approve(&fixture, run.id).await;
    let write = fixture.database.write().await.unwrap();
    start_run(&write, &approved, EXECUTION).await.unwrap();

    record_outcome_unknown(&write, &approved, "the exit was not observed", NOW)
        .await
        .unwrap();

    let stored = run_request(&write, run.id, NOW).await.unwrap().unwrap();
    assert_eq!(stored.state, RunState::OutcomeUnknown);
    assert_eq!(stored.note.as_deref(), Some("the exit was not observed"));
}

#[tokio::test]
async fn a_run_that_never_started_cannot_be_outcome_unknown() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "make deploy").await;
    let approved = approve(&fixture, run.id).await;
    let write = fixture.database.write().await.unwrap();

    let refused = refusal(record_outcome_unknown(&write, &approved, "unobserved", NOW).await);

    assert_eq!(refused, Refusal::AlreadyDecided { state: "approved" });
}

#[tokio::test]
async fn a_run_of_another_revision_cannot_start_or_complete_the_request() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let approved = approve(&fixture, run.id).await;
    let impostor = Approved {
        revision: approved.revision + 1,
        ..approved.clone()
    };
    let write = fixture.database.write().await.unwrap();
    assert!(matches!(
        refusal(start_run(&write, &impostor, EXECUTION).await),
        Refusal::AlreadyDecided { .. }
    ));
    start_run(&write, &approved, EXECUTION).await.unwrap();
    assert!(matches!(
        refusal(finish_run(&write, &impostor, &result(0), NOW).await),
        Refusal::AlreadyDecided { .. }
    ));
}

/// A run the previous daemon left approved, and one it left running with its execution.
async fn interrupted(fixture: &Fixture) -> Vec<Interrupted> {
    let never_started = fixture.asked(&session("s1"), "make build").await;
    let started = fixture.asked(&session("s1"), "make deploy").await;
    approve(fixture, never_started.id).await;
    let running = approve(fixture, started.id).await;
    let write = fixture.database.write().await.unwrap();
    start_run(&write, &running, EXECUTION).await.unwrap();
    write.commit().await.unwrap();
    let read = fixture.database.read().await.unwrap();
    interrupted_runs(&read).await.unwrap()
}

#[tokio::test]
async fn a_restart_finds_each_interrupted_run_with_the_execution_it_started() {
    let fixture = Fixture::new().await;

    let found = interrupted(&fixture).await;

    let found: Vec<(RunState, Option<ExecutionId>)> =
        found.iter().map(|run| (run.state, run.execution)).collect();
    assert_eq!(
        found,
        [
            (RunState::Approved, None),
            (RunState::Running, Some(EXECUTION))
        ]
    );
}

#[tokio::test]
async fn a_restart_never_starts_an_approved_run() {
    let fixture = Fixture::new().await;
    let [approved, _] = &interrupted(&fixture).await[..] else {
        panic!("expected two interrupted runs");
    };
    let write = fixture.database.write().await.unwrap();

    let state = reconcile_run(&write, approved, None, NOW).await.unwrap();

    assert_eq!(state, RunState::NotRun);
    let stored = run_request(&write, approved.run.id, NOW)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, RunState::NotRun);
}

/// What a restart makes of a running run, from its execution's observed phase (`None`: the
/// run has no execution, or none with that id exists).
#[tokio::test]
async fn a_restart_ends_a_running_run_from_what_its_execution_did() {
    let cases = [
        (Some(Phase::Launching), RunState::Running),
        (Some(Phase::Running), RunState::Running),
        (
            Some(Phase::Ended(EndReason::NeverStarted)),
            RunState::NotRun,
        ),
        (
            Some(Phase::Ended(EndReason::Exited)),
            RunState::OutcomeUnknown,
        ),
        (
            Some(Phase::Ended(EndReason::Terminated)),
            RunState::OutcomeUnknown,
        ),
        (
            Some(Phase::Ended(EndReason::HostRestarted)),
            RunState::OutcomeUnknown,
        ),
        (None, RunState::OutcomeUnknown),
    ];
    for (phase, expected) in cases {
        let fixture = Fixture::new().await;
        let [_, running] = &interrupted(&fixture).await[..] else {
            panic!("expected two interrupted runs");
        };
        let write = fixture.database.write().await.unwrap();

        let state = reconcile_run(&write, running, phase.as_ref(), NOW)
            .await
            .unwrap();

        assert_eq!(state, expected, "{phase:?}");
        let stored = run_request(&write, running.run.id, NOW)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, expected, "{phase:?}");
        assert_eq!(
            stored.note.is_some(),
            expected != RunState::Running,
            "{phase:?}"
        );
    }
}

#[tokio::test]
async fn a_run_that_could_not_start_is_recorded_as_not_run() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let approved = approve(&fixture, run.id).await;
    let write = fixture.database.write().await.unwrap();
    start_run(&write, &approved, EXECUTION).await.unwrap();
    record_not_run(&write, &approved, "the directory moved", NOW)
        .await
        .unwrap();
    let stored = run_request(&write, run.id, NOW).await.unwrap().unwrap();
    assert_eq!(stored.state, RunState::NotRun);
    assert_eq!(stored.note.as_deref(), Some("the directory moved"));
}

#[tokio::test]
async fn another_sessions_request_is_not_found_for_the_asker() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        refusal(run_status(&write, &session("s2"), run.id, NOW).await),
        Refusal::NotFound {
            kind: "run request",
            id: run.id.0
        }
    );
}

#[tokio::test]
async fn an_invalid_ask_names_the_field_at_fault() {
    let fixture = Fixture::new().await;
    fixture.grant(&session("s1")).await;
    let cases = [
        ("command", fixture.ask("   ")),
        ("command", fixture.ask(&"x".repeat(16 * 1024 + 1))),
        ("command", fixture.ask("ls\0")),
        (
            "reason",
            cockpit_approvals::RunAsk {
                reason: " ".to_owned(),
                ..fixture.ask("ls")
            },
        ),
        (
            "reason",
            cockpit_approvals::RunAsk {
                reason: "x".repeat(1001),
                ..fixture.ask("ls")
            },
        ),
    ];
    for (expected_field, ask) in cases {
        let write = fixture.database.write().await.unwrap();
        match refusal(ask_run(&write, &session("s1"), ask, NOW).await) {
            Refusal::InvalidRequest { field, .. } => assert_eq!(field, expected_field),
            other => panic!("expected an invalid {expected_field}, got {other:?}"),
        }
    }
}

#[test]
fn a_working_directory_is_resolved_or_refused() {
    let dir = tempfile::tempdir().unwrap();
    let real = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir(real.join("target")).unwrap();
    std::os::unix::fs::symlink(real.join("target"), real.join("link")).unwrap();
    assert_eq!(
        resolve_cwd(real.join("link").to_str().unwrap()).unwrap(),
        real.join("target")
    );
    for cwd in ["relative/dir", "/nonexistent-cockpit-dir"] {
        assert!(
            matches!(
                resolve_cwd(cwd),
                Err(Refusal::InvalidRequest { field: "cwd", .. })
            ),
            "{cwd}"
        );
    }
    std::fs::write(real.join("file"), "").unwrap();
    assert!(resolve_cwd(real.join("file").to_str().unwrap()).is_err());
}

#[tokio::test]
async fn a_command_waiting_for_input_comes_back_to_the_front_of_the_queue() {
    let fixture = Fixture::new().await;
    let first = fixture.asked(&session("s1"), "make test").await;
    let prompting = fixture.asked(&session("s1"), "sudo make install").await;
    let approved = approve(&fixture, prompting.id).await;
    let write = fixture.database.write().await.unwrap();
    start_run(&write, &approved, EXECUTION).await.unwrap();
    let mut queue = open_runs(&write, NOW).await.unwrap();

    queue_order(&mut queue, &[]);
    assert_eq!(
        queue.iter().map(|run| run.id).collect::<Vec<_>>(),
        [first.id, prompting.id],
        "a running command stays behind the asking ones"
    );
    queue_order(&mut queue, &[prompting.id]);
    assert_eq!(
        queue.iter().map(|run| run.id).collect::<Vec<_>>(),
        [prompting.id, first.id]
    );
}
