//! The run request state machine on a real state database: binding, consume-once, expiry,
//! fencing and restart, with report 04's stale-approval walkthrough.

mod common;

use cockpit_approvals::{
    Approved, DecidedThrough, EXPIRES_AFTER_SECONDS, Error, Interrupted, Output, Refusal,
    RunDecision, RunId, RunRequest, RunResult, RunState, action_digest, approve_run, ask_run,
    decided_runs, decline_run, edit_run, finish_run, interrupted_runs, open_runs, queue_order,
    reconcile_run, record_checks, record_not_run, record_outcome_unknown, resolve_cwd, run_request,
    run_status, shown_run, start_run,
};
use cockpit_approvals::{Finding, RootFile, RootRuns, Severity};
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
        output_retired: false,
        stopped_by_owner: false,
        sudo_ran_unasked: 0,
        typed_secret_redacted: 0,
        limit_hits: Vec::new(),
    }
}

async fn approve(fixture: &Fixture, id: cockpit_approvals::RunId) -> Approved {
    let write = fixture.database.write().await.unwrap();
    let run = run_request(&write, id, NOW).await.unwrap().unwrap();
    let (approved, _) = approve_run(&write, None, id, run.revision, &run.digest(), AFTER_GRACE)
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

    let at_once = refusal(approve_run(&write, None, run.id, 1, &run.digest(), NOW).await);
    let almost =
        refusal(approve_run(&write, None, run.id, 1, &run.digest(), AFTER_GRACE - 1).await);
    let approved = approve_run(&write, None, run.id, 1, &run.digest(), AFTER_GRACE).await;

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

    let at_once = refusal(approve_run(&write, None, run.id, 2, &edited.digest(), edited_at).await);
    let later = approve_run(
        &write,
        None,
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
    let again = refusal(approve_run(&write, None, run.id, 1, &run.digest(), NOW).await);
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
            None,
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
        None,
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
        refusal(approve_run(&write, None, run.id, 1, &other, NOW).await),
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
        refusal(approve_run(&write, None, run.id, 1, &run.digest(), later).await),
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
    approve_run(
        &write,
        None,
        run.id,
        1,
        &run.digest(),
        past_the_first_expiry,
    )
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
async fn an_ask_returns_the_request_as_it_now_reads() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    assert_eq!(run, stored(&write, run.id).await);
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
    let declined = decline_run(&write, None, run.id, NOW).await.unwrap();
    assert_eq!(declined, stored(&write, run.id).await);
}

#[tokio::test]
async fn an_approval_returns_the_request_as_it_now_reads() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    let (_, approved) = approve_run(&write, None, run.id, 1, &run.digest(), AFTER_GRACE)
        .await
        .unwrap();
    assert_eq!(approved, stored(&write, run.id).await);
}

#[tokio::test]
async fn an_asking_request_records_no_decision() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    assert_eq!(run.decision, None);
}

#[tokio::test]
async fn an_approval_records_when_and_through_which_client() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    approve_run(
        &write,
        Some(DecidedThrough::Tui),
        run.id,
        1,
        &run.digest(),
        AFTER_GRACE,
    )
    .await
    .unwrap();
    assert_eq!(
        stored(&write, run.id).await.decision,
        Some(RunDecision {
            approved: true,
            at: AFTER_GRACE,
            through: Some(DecidedThrough::Tui),
        })
    );
}

#[tokio::test]
async fn a_decline_records_when_and_through_which_client() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    decline_run(&write, Some(DecidedThrough::Cli), run.id, NOW)
        .await
        .unwrap();
    assert_eq!(
        stored(&write, run.id).await.decision,
        Some(RunDecision {
            approved: false,
            at: NOW,
            through: Some(DecidedThrough::Cli),
        })
    );
}

#[tokio::test]
async fn a_declined_request_cannot_be_approved() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    let write = fixture.database.write().await.unwrap();
    let declined = decline_run(&write, None, run.id, NOW).await.unwrap();
    assert_eq!(declined.state, RunState::Declined);
    assert_eq!(
        refusal(approve_run(&write, None, run.id, 1, &run.digest(), NOW).await),
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
async fn a_result_keeps_how_many_sudo_ran_without_asking() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "make install").await;
    let approved = approve(&fixture, run.id).await;
    let write = fixture.database.write().await.unwrap();
    start_run(&write, &approved, EXECUTION).await.unwrap();
    let flagged = RunResult {
        sudo_ran_unasked: 2,
        ..result(0)
    };

    finish_run(&write, &approved, &flagged, NOW).await.unwrap();

    let stored = run_request(&write, run.id, NOW).await.unwrap().unwrap();
    assert_eq!(stored.result.map(|result| result.sudo_ran_unasked), Some(2));
}

#[tokio::test]
async fn a_result_keeps_how_many_times_the_typed_secret_was_redacted() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "make install").await;
    let approved = approve(&fixture, run.id).await;
    let write = fixture.database.write().await.unwrap();
    start_run(&write, &approved, EXECUTION).await.unwrap();
    let flagged = RunResult {
        typed_secret_redacted: 3,
        ..result(0)
    };

    finish_run(&write, &approved, &flagged, NOW).await.unwrap();

    let stored = run_request(&write, run.id, NOW).await.unwrap().unwrap();
    assert_eq!(
        stored.result.map(|result| result.typed_secret_redacted),
        Some(3)
    );
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

#[tokio::test]
async fn an_ask_whose_session_may_no_longer_ask_leaves_the_owners_queue() {
    let fixture = Fixture::new().await;
    fixture.asked(&session("s1"), "ls").await;
    fixture.revoke_all(&session("s1")).await;

    let read = fixture.database.read().await.unwrap();
    assert_eq!(open_runs(&read, NOW).await.unwrap(), []);
}

#[tokio::test]
async fn an_ask_whose_session_may_no_longer_ask_cannot_be_approved() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    fixture.revoke_all(&session("s1")).await;

    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        refusal(approve_run(&write, None, run.id, 1, &run.digest(), AFTER_GRACE).await),
        Refusal::AskerMayNotAsk
    );
}

#[tokio::test]
async fn an_ask_whose_session_may_no_longer_ask_cannot_be_edited() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    fixture.revoke_all(&session("s1")).await;

    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        refusal(edit_run(&write, run.id, 1, "ls -la", NOW).await),
        Refusal::AskerMayNotAsk
    );
}

#[tokio::test]
async fn an_ask_whose_session_may_no_longer_ask_can_still_be_declined() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    fixture.revoke_all(&session("s1")).await;

    let write = fixture.database.write().await.unwrap();
    let declined = decline_run(&write, None, run.id, NOW).await.unwrap();
    assert_eq!(declined.state, RunState::Declined);
}

#[tokio::test]
async fn an_ask_whose_session_may_no_longer_ask_shows_as_expired() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    fixture.revoke_all(&session("s1")).await;

    let read = fixture.database.read().await.unwrap();
    let shown = shown_run(&read, run.id, NOW).await.unwrap().unwrap();
    assert_eq!(shown.state, RunState::Expired);
}

#[tokio::test]
async fn an_ask_whose_session_may_still_ask_shows_as_asking() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;

    let read = fixture.database.read().await.unwrap();
    let shown = shown_run(&read, run.id, NOW).await.unwrap().unwrap();
    assert_eq!(shown.state, RunState::Asking);
}

#[tokio::test]
async fn an_approved_run_stays_in_the_queue_after_its_session_may_no_longer_ask() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    approve(&fixture, run.id).await;
    fixture.revoke_all(&session("s1")).await;

    let read = fixture.database.read().await.unwrap();
    let open = open_runs(&read, NOW).await.unwrap();
    assert_eq!(
        open.iter()
            .map(|run| (run.id, run.state))
            .collect::<Vec<_>>(),
        [(run.id, RunState::Approved)]
    );
}

#[tokio::test]
async fn the_queue_keeps_the_asks_of_sessions_that_may_still_ask() {
    let fixture = Fixture::new().await;
    fixture.asked(&session("s1"), "ls").await;
    let kept = fixture.asked(&session("s2"), "pwd").await;
    fixture.revoke_all(&session("s1")).await;

    let read = fixture.database.read().await.unwrap();
    assert_eq!(open_runs(&read, NOW).await.unwrap(), [kept]);
}

#[tokio::test]
async fn an_ask_stays_out_of_the_queue_when_its_session_is_granted_again() {
    let fixture = Fixture::new().await;
    fixture.asked(&session("s1"), "ls").await;
    fixture.revoke_all(&session("s1")).await;
    fixture.grant(&session("s1")).await;

    let read = fixture.database.read().await.unwrap();
    assert_eq!(open_runs(&read, NOW).await.unwrap(), []);
}

#[tokio::test]
async fn an_ask_cannot_be_approved_once_its_session_is_granted_again() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    fixture.revoke_all(&session("s1")).await;
    fixture.grant(&session("s1")).await;

    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        refusal(approve_run(&write, None, run.id, 1, &run.digest(), AFTER_GRACE).await),
        Refusal::AskerMayNotAsk
    );
}

#[tokio::test]
async fn a_stale_approval_of_an_ask_whose_session_may_no_longer_ask_names_the_session() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    fixture.revoke_all(&session("s1")).await;

    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        refusal(approve_run(&write, None, run.id, 2, &run.digest(), AFTER_GRACE).await),
        Refusal::AskerMayNotAsk
    );
}

#[tokio::test]
async fn a_poll_of_an_ask_whose_session_may_no_longer_ask_reads_expired_and_writes_nothing() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "ls").await;
    fixture.revoke_all(&session("s1")).await;
    let before = fixture.revision().await;

    let write = fixture.database.write().await.unwrap();
    let seen = run_status(&write, &session("s1"), run.id, NOW + 60)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(
        (seen.state, seen.expires_at),
        (RunState::Expired, run.expires_at)
    );
    assert_eq!(fixture.revision().await, before);
}

mod repeated_asks {
    use super::*;
    use cockpit_approvals::ask_run_unless_pending;

    async fn ask_once(
        fixture: &Fixture,
        who: &str,
        command: &str,
        now: i64,
    ) -> cockpit_approvals::FiledRun {
        let write = fixture.database.write().await.unwrap();
        let filed = ask_run_unless_pending(&write, &session(who), fixture.ask(command), now)
            .await
            .unwrap();
        write.commit().await.unwrap();
        filed
    }

    #[tokio::test]
    async fn an_identical_pending_ask_is_returned_and_files_nothing() {
        let fixture = Fixture::new().await;
        fixture.grant(&session("s1")).await;
        let first = ask_once(&fixture, "s1", "ls", NOW).await;
        let revision = fixture.revision().await;

        let repeat = ask_once(&fixture, "s1", "ls", NOW + 1).await;

        assert!(!first.already_pending);
        assert!(repeat.already_pending);
        assert_eq!(repeat.run.id, first.run.id);
        assert_eq!(fixture.revision().await, revision);
    }

    #[tokio::test]
    async fn another_session_or_command_files_a_new_ask() {
        let fixture = Fixture::new().await;
        fixture.grant(&session("s1")).await;
        fixture.grant(&session("s2")).await;
        let first = ask_once(&fixture, "s1", "ls", NOW).await;

        let other_command = ask_once(&fixture, "s1", "pwd", NOW).await;
        let other_session = ask_once(&fixture, "s2", "ls", NOW).await;

        for filed in [&other_command, &other_session] {
            assert!(!filed.already_pending);
            assert_ne!(filed.run.id, first.run.id);
        }
    }

    #[tokio::test]
    async fn a_decided_or_expired_ask_is_never_matched() {
        let fixture = Fixture::new().await;
        fixture.grant(&session("s1")).await;
        let declined = ask_once(&fixture, "s1", "ls", NOW).await;
        let write = fixture.database.write().await.unwrap();
        decline_run(&write, None, declined.run.id, NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();

        let after_decline = ask_once(&fixture, "s1", "ls", NOW).await;
        let after_expiry = ask_once(&fixture, "s1", "ls", NOW + EXPIRES_AFTER_SECONDS + 1).await;

        assert!(!after_decline.already_pending);
        assert!(!after_expiry.already_pending);
        assert_ne!(after_expiry.run.id, after_decline.run.id);
    }

    #[tokio::test]
    async fn an_ask_the_owner_edited_is_not_matched() {
        let fixture = Fixture::new().await;
        fixture.grant(&session("s1")).await;
        let first = ask_once(&fixture, "s1", "ls", NOW).await;
        let write = fixture.database.write().await.unwrap();
        edit_run(&write, first.run.id, 1, "ls -a", NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();

        let repeat = ask_once(&fixture, "s1", "ls", NOW + 1).await;

        assert!(!repeat.already_pending);
        assert_ne!(repeat.run.id, first.run.id);
    }

    #[tokio::test]
    async fn a_session_without_the_grant_is_refused_even_for_a_repeat() {
        let fixture = Fixture::new().await;
        fixture.grant(&session("s1")).await;
        ask_once(&fixture, "s1", "ls", NOW).await;
        fixture.revoke_all(&session("s1")).await;
        let write = fixture.database.write().await.unwrap();

        let refused =
            ask_run_unless_pending(&write, &session("s1"), fixture.ask("ls"), NOW + 1).await;

        assert!(matches!(refused, Err(Error::Refused(_))), "{refused:?}");
    }
}

/// An approved-and-finished request at `AFTER_GRACE`, a declined one 50 s later and one still
/// asking: the owner's list of decisions.
async fn two_decided_and_one_asking(fixture: &Fixture) -> (RunId, RunId) {
    let finished = fixture.asked(&session("s1"), "ls").await;
    let declined = fixture.asked(&session("s2"), "rm -rf target").await;
    fixture.asked(&session("s3"), "pwd").await;
    let write = fixture.database.write().await.unwrap();
    let (approved, _) = approve_run(
        &write,
        None,
        finished.id,
        1,
        &finished.digest(),
        AFTER_GRACE,
    )
    .await
    .unwrap();
    start_run(&write, &approved, EXECUTION).await.unwrap();
    finish_run(&write, &approved, &result(3), AFTER_GRACE + 1)
        .await
        .unwrap();
    decline_run(&write, None, declined.id, AFTER_GRACE + 50)
        .await
        .unwrap();
    write.commit().await.unwrap();
    (finished.id, declined.id)
}

#[tokio::test]
async fn the_decided_list_holds_every_decision_oldest_first_and_no_asking_request() {
    let fixture = Fixture::new().await;
    let (finished, declined) = two_decided_and_one_asking(&fixture).await;
    let read = fixture.database.read().await.unwrap();

    let decided = decided_runs(&read, None, AFTER_GRACE + 100).await.unwrap();

    let ids: Vec<RunId> = decided.iter().map(|run| run.id).collect();
    assert_eq!(ids, [finished, declined]);
    assert_eq!(decided[0].state, RunState::Finished);
    assert_eq!(decided[0].result.as_ref().unwrap().exit_code, 3);
    assert_eq!(decided[1].state, RunState::Declined);
    assert_eq!(decided[1].result, None);
}

#[tokio::test]
async fn the_decided_list_since_a_time_keeps_the_decisions_made_from_then_on() {
    let fixture = Fixture::new().await;
    let (_, declined) = two_decided_and_one_asking(&fixture).await;
    let read = fixture.database.read().await.unwrap();

    let decided = decided_runs(&read, Some(AFTER_GRACE + 50), AFTER_GRACE + 100)
        .await
        .unwrap();

    let ids: Vec<RunId> = decided.iter().map(|run| run.id).collect();
    assert_eq!(ids, [declined]);
}

fn flagged() -> RootRuns {
    RootRuns {
        editable: vec![RootFile {
            path: "/clone/x.sh".into(),
            sha256: Some("ab".repeat(32)),
        }],
        cannot_tell: true,
    }
}

fn pins() -> Vec<RootFile> {
    vec![
        RootFile {
            path: "/clone/x.sh".into(),
            sha256: Some("ab".repeat(32)),
        },
        RootFile {
            path: "/clone/Makefile".into(),
            sha256: None,
        },
    ]
}

fn found() -> Vec<Finding> {
    vec![Finding {
        rule: "download-runs".to_owned(),
        severity: Severity::High,
        reason: "runs code it downloads".to_owned(),
        excerpt: "curl x | sh".to_owned(),
        file: None,
        line: None,
    }]
}

#[tokio::test]
async fn what_a_revision_runs_as_root_and_its_findings_are_kept_with_the_request() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "sudo ./x.sh").await;
    assert_eq!(
        (&run.root_runs, &run.findings, &run.pinned),
        (&None, &None, &None)
    );
    let write = fixture.database.write().await.unwrap();

    let recorded = record_checks(&write, run.id, 1, &flagged(), &found(), &pins(), NOW)
        .await
        .unwrap();

    assert_eq!(recorded.root_runs, Some(flagged()));
    assert_eq!(recorded.findings, Some(found()));
    assert_eq!(recorded.pinned, Some(pins()));
    assert_eq!(recorded, stored(&write, run.id).await);
}

#[tokio::test]
async fn an_edit_drops_what_the_old_revision_ran_as_root_and_its_findings() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "sudo ./x.sh").await;
    let write = fixture.database.write().await.unwrap();
    record_checks(&write, run.id, 1, &flagged(), &found(), &pins(), NOW)
        .await
        .unwrap();

    let edited = edit_run(&write, run.id, 1, "sudo ./y.sh", NOW)
        .await
        .unwrap();

    assert_eq!(
        (&edited.root_runs, &edited.findings, &edited.pinned),
        (&None, &None, &None)
    );
    assert_eq!(
        refusal(record_checks(&write, run.id, 1, &flagged(), &found(), &pins(), NOW).await),
        Refusal::StaleRevision { current: 2 }
    );
}

#[tokio::test]
async fn an_approval_keeps_what_the_run_runs_as_root_and_its_findings_in_the_record() {
    let fixture = Fixture::new().await;
    let run = fixture.asked(&session("s1"), "sudo ./x.sh").await;
    let write = fixture.database.write().await.unwrap();
    record_checks(&write, run.id, 1, &flagged(), &found(), &pins(), NOW)
        .await
        .unwrap();

    let approved_at = NOW + cockpit_approvals::APPROVAL_GRACE_SECONDS;
    let (_, approved) = approve_run(&write, None, run.id, 1, &run.digest(), approved_at)
        .await
        .unwrap();

    assert_eq!(approved.root_runs, Some(flagged()));
    assert_eq!(approved.findings, Some(found()));
    assert_eq!(approved.pinned, Some(pins()));
    assert!(approved.decision.is_some_and(|decision| decision.approved));
    assert!(matches!(
        refusal(
            record_checks(
                &write,
                run.id,
                1,
                &RootRuns::default(),
                &[],
                &[],
                approved_at
            )
            .await
        ),
        Refusal::AlreadyDecided { .. }
    ));
}
