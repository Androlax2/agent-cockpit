//! #406: a finished command run ended before the cutoff loses its output text; its decision,
//! command, digest and exit code stay, and a recent, open or start request is untouched.

mod common;

use cockpit_approvals::{
    Approved, Output, RunId, RunOutputs, RunResult, approve_run, finish_run, retirable_run_outputs,
    retire_run_outputs, run_request, start_run,
};
use cockpit_processes::ExecutionId;
use common::{Fixture, NOW, session};

const AFTER_GRACE: i64 = NOW + cockpit_approvals::APPROVAL_GRACE_SECONDS;
const ENDED: i64 = AFTER_GRACE + 10;
const LATER: i64 = ENDED + 1_000;

async fn approved(fixture: &Fixture, command: &str) -> Approved {
    let run = fixture.asked(&session("s1"), command).await;
    let write = fixture.database.write().await.unwrap();
    let (approved, _) = approve_run(
        &write,
        None,
        run.id,
        run.revision,
        &run.digest(),
        AFTER_GRACE,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    approved
}

/// A command run finished at `ended_at` with `shown` as its output.
async fn finished(fixture: &Fixture, command: &str, shown: &str, ended_at: i64) -> RunId {
    let approved = approved(fixture, command).await;
    let write = fixture.database.write().await.unwrap();
    start_run(&write, &approved, ExecutionId(approved.id.0))
        .await
        .unwrap();
    let result = RunResult {
        exit_code: 3,
        output: Output {
            shown: shown.to_owned(),
            omitted_lines: 2,
            truncated_bytes: 1,
            redacted: 0,
        },
        output_retired: false,
        stopped_by_owner: false,
        sudo_ran_unasked: 0,
        typed_secret_redacted: 0,
    };
    finish_run(&write, &approved, &result, ended_at)
        .await
        .unwrap();
    write.commit().await.unwrap();
    approved.id
}

async fn retire(fixture: &Fixture, cutoff: i64, limit: u32) -> RunOutputs {
    let write = fixture.database.write().await.unwrap();
    let retired = retire_run_outputs(&write, cutoff, LATER, limit)
        .await
        .unwrap();
    write.commit().await.unwrap();
    retired
}

async fn retirable(fixture: &Fixture, cutoff: i64) -> RunOutputs {
    let read = fixture.database.read().await.unwrap();
    retirable_run_outputs(&read, cutoff).await.unwrap()
}

#[tokio::test]
async fn only_finished_commands_ended_by_the_cutoff_are_retirable() {
    let fixture = Fixture::new().await;
    finished(&fixture, "ls", "twelve bytes", ENDED).await;
    finished(&fixture, "pwd", "late", LATER + 1).await;
    approved(&fixture, "make").await;
    fixture.asked(&session("s1"), "true").await;

    assert_eq!(
        retirable(&fixture, LATER).await,
        RunOutputs { runs: 1, bytes: 12 }
    );
}

#[tokio::test]
async fn a_retired_run_keeps_its_decision_command_and_exit_code() {
    let fixture = Fixture::new().await;
    let id = finished(&fixture, "ls -l", "the old output", ENDED).await;
    let before = {
        let read = fixture.database.read().await.unwrap();
        run_request(&read, id, LATER).await.unwrap().unwrap()
    };

    let retired = retire(&fixture, LATER, 10).await;

    assert_eq!(retired, RunOutputs { runs: 1, bytes: 14 });
    let read = fixture.database.read().await.unwrap();
    let after = run_request(&read, id, LATER).await.unwrap().unwrap();
    let result = after.result.clone().unwrap();
    assert!(result.output_retired);
    assert_eq!(result.output.shown, "");
    assert_eq!(result.exit_code, 3);
    assert_eq!(
        (
            &after.command,
            &after.asked_command,
            after.decision,
            after.revision
        ),
        (
            &before.command,
            &before.asked_command,
            before.decision,
            before.revision
        )
    );
    assert_eq!(after.digest(), before.digest());
    assert_eq!(retirable(&fixture, LATER).await, RunOutputs::default());
}

#[tokio::test]
async fn retirement_goes_in_bounded_batches_oldest_first() {
    let fixture = Fixture::new().await;
    let first = finished(&fixture, "one", "a", ENDED).await;
    let second = finished(&fixture, "two", "bb", ENDED).await;
    let third = finished(&fixture, "three", "ccc", ENDED).await;

    assert_eq!(
        retire(&fixture, LATER, 2).await,
        RunOutputs { runs: 2, bytes: 3 }
    );
    assert_eq!(
        retire(&fixture, LATER, 2).await,
        RunOutputs { runs: 1, bytes: 3 }
    );
    assert_eq!(retire(&fixture, LATER, 2).await, RunOutputs::default());
    let read = fixture.database.read().await.unwrap();
    for id in [first, second, third] {
        let run = run_request(&read, id, LATER).await.unwrap().unwrap();
        assert!(run.result.unwrap().output_retired, "{id:?}");
    }
}

#[tokio::test]
async fn a_run_ended_after_the_cutoff_keeps_its_output() {
    let fixture = Fixture::new().await;
    let id = finished(&fixture, "ls", "fresh", LATER).await;

    assert_eq!(retire(&fixture, ENDED, 10).await, RunOutputs::default());

    let read = fixture.database.read().await.unwrap();
    let result = run_request(&read, id, LATER)
        .await
        .unwrap()
        .unwrap()
        .result
        .unwrap();
    assert_eq!(
        (result.output.shown.as_str(), result.output_retired),
        ("fresh", false)
    );
}

#[tokio::test]
async fn the_schema_refuses_a_retirement_mark_on_a_run_that_kept_its_output() {
    let fixture = Fixture::new().await;
    let id = finished(&fixture, "ls", "kept", ENDED).await;
    let write = fixture.database.write().await.unwrap();

    let marked = write
        .execute(
            "UPDATE approvals_runs SET output_retired_at = 1 WHERE id = ?1",
            [id.0],
        )
        .await;

    assert!(marked.is_err());
}
