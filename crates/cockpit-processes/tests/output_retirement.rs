//! Retiring a run's captured output files: fenced against readers, rechecked under the writer,
//! and finished by recovery after an interruption. Fixture homes only.
mod common;
use common::*;

use cockpit_processes::{
    ExecutionId, OutputKind, OutputMode, RetiringOutput, attribute_output, begin_output_retirement,
    finish_output_retirement, guard_output, guard_output_in, recover_output_retirements,
    retirable_outputs, retiring_outputs,
};
use cockpit_turso::Database;
use std::sync::Arc;

/// An ended run of `project` whose stdout holds `output`, with no reader left.
async fn ended_file_run(
    home: &Home,
    database: &Arc<Database>,
    supervisor: &cockpit_processes::Supervisor,
    output: &str,
) -> ExecutionId {
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    attribute_output(
        &write,
        id,
        database.epoch(),
        Some("app"),
        None,
        OutputMode::Files,
        false,
    )
    .await
    .unwrap();
    let guard = guard_output_in(&write, Arc::clone(database), id)
        .await
        .unwrap();
    write.commit().await.unwrap();
    let launched = supervisor
        .launch_reserved_with_output(id, &shell(home, &format!("printf {output}")), guard)
        .await
        .unwrap();
    launched.exit.await.unwrap().unwrap();
    wait_until_ended(supervisor, database, id).await;
    assert!(
        eventually(|| cockpit_processes::output_users(database.epoch(), id) == 0).await,
        "the run's own collector never let go of its output"
    );
    id
}

async fn revision(database: &Database, id: ExecutionId) -> i64 {
    cockpit_processes::output_facts(&database.read().await.unwrap(), id)
        .await
        .unwrap()
        .revision
}

async fn retire_in_database(database: &Database, id: ExecutionId) -> RetiringOutput {
    let revision = revision(database, id).await;
    let write = database.write().await.unwrap();
    let retiring = begin_output_retirement(&write, id, database.epoch(), revision)
        .await
        .unwrap();
    write.commit().await.unwrap();
    retiring
}

#[tokio::test]
async fn an_ended_attributed_file_run_is_listed_as_retirable() {
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let id = ended_file_run(&home, &database, &supervisor, "done").await;

    let listed = retirable_outputs(
        &database.read().await.unwrap(),
        database.epoch(),
        ExecutionId(0),
        16,
    )
    .await
    .unwrap();

    assert_eq!(
        listed
            .iter()
            .map(|output| output.facts.execution)
            .collect::<Vec<_>>(),
        vec![id]
    );
    assert!(listed[0].ended_at > 0);
}

#[tokio::test]
async fn a_pending_result_or_unattributed_run_is_not_retirable() {
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let write = database.write().await.unwrap();
    let pending = supervisor.reserve(&write, database.epoch()).await.unwrap();
    attribute_output(
        &write,
        pending,
        database.epoch(),
        Some("app"),
        None,
        OutputMode::Files,
        true,
    )
    .await
    .unwrap();
    supervisor.reserve(&write, database.epoch()).await.unwrap();
    write.commit().await.unwrap();

    let listed = retirable_outputs(
        &database.read().await.unwrap(),
        database.epoch(),
        ExecutionId(0),
        16,
    )
    .await
    .unwrap();

    assert!(listed.is_empty(), "{listed:?}");
}

#[tokio::test]
async fn retirement_refuses_an_output_someone_still_reads() {
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let id = ended_file_run(&home, &database, &supervisor, "done").await;
    let reader = guard_output(Arc::clone(&database), id).await.unwrap();
    let revision = revision(&database, id).await;

    let write = database.write().await.unwrap();
    let refused = begin_output_retirement(&write, id, database.epoch(), revision).await;

    assert!(refused.is_err(), "retired output a reader holds");
    drop(reader);
}

#[tokio::test]
async fn retirement_refuses_a_stale_revision() {
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let id = ended_file_run(&home, &database, &supervisor, "done").await;
    let stale = revision(&database, id).await - 1;

    let write = database.write().await.unwrap();
    let refused = begin_output_retirement(&write, id, database.epoch(), stale).await;

    assert!(refused.is_err(), "accepted a stale revision");
}

#[tokio::test]
async fn a_retiring_output_refuses_new_readers() {
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let id = ended_file_run(&home, &database, &supervisor, "done").await;

    retire_in_database(&database, id).await;

    assert!(guard_output(Arc::clone(&database), id).await.is_err());
}

#[tokio::test]
async fn removing_a_run_output_keeps_files_that_are_not_its_stdout_or_stderr() {
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let id = ended_file_run(&home, &database, &supervisor, "12345").await;
    let directory = supervisor.execution_dir(&database, id);
    std::fs::write(directory.join("not-ours"), b"keep").unwrap();
    let retiring = retire_in_database(&database, id).await;

    let removed = supervisor.remove_output_files(&retiring).unwrap();

    assert_eq!(removed, 5);
    assert!(
        !supervisor
            .output_path(&database, id, OutputKind::Stdout)
            .exists()
    );
    assert!(
        !supervisor
            .output_path(&database, id, OutputKind::Stderr)
            .exists()
    );
    assert_eq!(std::fs::read(directory.join("not-ours")).unwrap(), b"keep");
}

#[tokio::test]
async fn recovery_finishes_a_retirement_interrupted_before_its_files_went() {
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let id = ended_file_run(&home, &database, &supervisor, "done").await;
    retire_in_database(&database, id).await;

    recover_output_retirements(&database, &supervisor)
        .await
        .unwrap();

    assert!(
        !supervisor
            .output_path(&database, id, OutputKind::Stdout)
            .exists()
    );
    assert!(
        retiring_outputs(&database.read().await.unwrap(), database.epoch())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_finished_retirement_is_not_listed_again() {
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let id = ended_file_run(&home, &database, &supervisor, "done").await;
    let retiring = retire_in_database(&database, id).await;
    supervisor.remove_output_files(&retiring).unwrap();
    let write = database.write().await.unwrap();
    finish_output_retirement(&write, retiring).await.unwrap();
    write.commit().await.unwrap();

    let listed = retirable_outputs(
        &database.read().await.unwrap(),
        database.epoch(),
        ExecutionId(0),
        16,
    )
    .await
    .unwrap();

    assert!(listed.is_empty(), "{listed:?}");
}
