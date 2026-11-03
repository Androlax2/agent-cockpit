//! Durable output ownership starts with the same reservation, including unknown callers.
mod common;
use common::*;

#[tokio::test]
async fn every_reservation_has_an_epoch_bound_unknown_output_record() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();
    let exists = read
        .prepare("SELECT count(*) FROM sqlite_master WHERE name = 'process_output_sets'")
        .await
        .unwrap()
        .query_row(())
        .await
        .unwrap()
        .get::<i64>(0)
        .unwrap();
    assert_eq!(
        exists, 1,
        "output identity must be committed with every execution reservation"
    );
    let row = read
        .prepare("SELECT origin_epoch, mode, project FROM process_output_sets WHERE execution = ?1")
        .await
        .unwrap()
        .query_row([id.0])
        .await
        .unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), database.epoch());
    assert_eq!(row.get::<String>(1).unwrap(), "unknown");
    assert_eq!(row.get::<Option<String>>(2).unwrap(), None);
}

#[tokio::test]
async fn attribution_is_one_time_transactional_and_unknown_legacy_is_protected() {
    use cockpit_processes::{OutputMode, attribute_output, output_facts};
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    attribute_output(
        &write,
        id,
        database.epoch(),
        Some("app"),
        Some("session"),
        OutputMode::Files,
        true,
    )
    .await
    .unwrap();
    assert!(
        attribute_output(
            &write,
            id,
            database.epoch(),
            Some("other"),
            None,
            OutputMode::Files,
            false
        )
        .await
        .is_err()
    );
    drop(write);
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();
    let fact = output_facts(&read, id).await.unwrap();
    assert_eq!(fact.mode, OutputMode::Unknown);
    assert_eq!(
        fact.project, None,
        "rolled-back attribution must not survive"
    );
    assert!(fact.protected());
}

#[tokio::test]
async fn a_guard_keeps_database_open_until_its_last_holder_drops() {
    use cockpit_processes::{OutputMode, attribute_output, guard_output, output_users};
    use std::sync::Arc;
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    attribute_output(
        &write,
        id,
        database.epoch(),
        Some("app"),
        None,
        OutputMode::Files,
        true,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    let guard = guard_output(Arc::clone(&database), id).await.unwrap();
    let epoch = database.epoch().to_owned();
    assert_eq!(output_users(&epoch, id), 1);
    let last = guard.clone();
    drop(guard);
    drop(database);
    assert!(
        cockpit_turso::Database::open(
            &home.path().join("state.db"),
            &[cockpit_processes::MIGRATIONS]
        )
        .await
        .is_err()
    );
    drop(last);
    assert_eq!(output_users(&epoch, id), 0);
    let reopened = home.open_database().await;
    assert_eq!(reopened.epoch(), epoch);
}

#[tokio::test]
async fn restored_database_does_not_rebind_a_historical_output_directory() {
    use std::sync::Arc;
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    write.commit().await.unwrap();
    let epoch = database.epoch().to_owned();
    let backup = home.path().join("backup.db");
    database.backup_to(&backup).await.unwrap();
    drop(database);
    cockpit_turso::restore(&backup, &home.path().join("state.db"))
        .await
        .unwrap();
    let restored = Arc::new(home.open_database().await);
    assert_ne!(restored.epoch(), epoch);
    assert!(cockpit_processes::guard_output(restored, id).await.is_err());
}

#[tokio::test]
async fn a_finished_stream_keeps_its_guard_until_the_last_viewer_drops() {
    use cockpit_processes::{OutputKind, OutputRead, output_users};
    use std::sync::Arc;
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let execution = supervisor
        .launch(&database, &shell(&home, "printf kept"))
        .await
        .unwrap();
    let stream = supervisor
        .follow_output(Arc::clone(&database), &execution, OutputKind::Stdout)
        .await
        .unwrap();
    let clone = stream.clone();
    assert_eq!(stream.read(4, 10).await, OutputRead::Finished);
    drop(stream);
    assert_eq!(output_users(database.epoch(), execution.id), 1);
    drop(clone);
    assert!(eventually(|| output_users(database.epoch(), execution.id) == 0).await);
}

#[tokio::test]
async fn recovery_of_an_unclaimed_launch_prevents_a_stale_producer_from_spawning() {
    use std::sync::Arc;
    let home = Home::new();
    let database = Arc::new(home.open_database().await);
    let supervisor = home.process_group_supervisor();
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    cockpit_processes::attribute_output(
        &write,
        id,
        database.epoch(),
        Some("app"),
        None,
        cockpit_processes::OutputMode::Files,
        true,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    let write = database.write().await.unwrap();
    assert!(
        cockpit_processes::end_unstarted_output(&write, id, database.epoch())
            .await
            .unwrap()
    );
    write.commit().await.unwrap();
    let guard = cockpit_processes::guard_output(Arc::clone(&database), id)
        .await
        .unwrap();
    assert!(
        supervisor
            .launch_reserved_with_output(id, &shell(&home, "touch should-not-exist"), guard)
            .await
            .is_err()
    );
    assert!(!home.path().join("should-not-exist").exists());
    assert!(!supervisor.execution_dir(&database, id).exists());
}

#[tokio::test]
async fn attribution_cannot_change_after_hosted_preparation_claimed_launch() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let prepared = supervisor
        .prepare_hosted(&database, &shell(&home, "true"), &[])
        .await
        .unwrap();
    let write = database.write().await.unwrap();
    assert!(
        cockpit_processes::attribute_output(
            &write,
            prepared.execution,
            database.epoch(),
            Some("late"),
            None,
            cockpit_processes::OutputMode::Files,
            false
        )
        .await
        .is_err(),
        "attribution must close before external preparation"
    );
}

#[tokio::test]
async fn launch_entrypoints_refuse_a_different_capture_mode_before_filesystem_effects() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    for mode in [
        cockpit_processes::OutputMode::Pty,
        cockpit_processes::OutputMode::Hosted,
    ] {
        let write = database.write().await.unwrap();
        let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
        cockpit_processes::attribute_output(
            &write,
            id,
            database.epoch(),
            Some("app"),
            None,
            mode,
            false,
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        assert!(
            supervisor
                .launch_reserved(&database, id, &shell(&home, "true"))
                .await
                .is_err(),
            "{mode:?} is not file output"
        );
        assert!(!supervisor.execution_dir(&database, id).exists());
    }
    let write = database.write().await.unwrap();
    let id = supervisor.reserve(&write, database.epoch()).await.unwrap();
    cockpit_processes::attribute_output(
        &write,
        id,
        database.epoch(),
        Some("app"),
        None,
        cockpit_processes::OutputMode::Files,
        true,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    assert!(
        supervisor
            .prepare_reserved_hosted(&database, id, &shell(&home, "true"), &[])
            .await
            .is_err()
    );
    assert!(!supervisor.execution_dir(&database, id).exists());
}

#[tokio::test]
async fn inactive_scope_without_a_launcher_identity_remains_unconfirmed() {
    let home = Home::new();
    let fakes = FakeSystemd::new();
    let database = home.open_database().await;
    let supervisor = home.scope_supervisor(&fakes);
    // Real preparation claims launch; a crash after this point cannot prove whether the
    // host submitted its command. The fake manager reports no registered unit.
    let prepared = supervisor
        .prepare_hosted(&database, &shell(&home, "true"), &[])
        .await
        .unwrap();
    let observed = supervisor
        .observe(&database, prepared.execution)
        .await
        .unwrap();
    assert_eq!(observed.phase, cockpit_processes::Phase::Launching);
}

#[tokio::test]
async fn upgrading_existing_executions_preserves_unknown_identity_and_monotonic_ids() {
    const OLD: cockpit_turso::MigrationSet = cockpit_turso::MigrationSet {
        owner: "processes",
        migrations: &[cockpit_processes::MIGRATIONS.migrations[0]],
    };
    let home = Home::new();
    let database = cockpit_turso::Database::open(&home.path().join("state.db"), &[OLD])
        .await
        .unwrap();
    let epoch = database.epoch().to_owned();
    let write = database.write().await.unwrap();
    write.execute("INSERT INTO process_executions (id,containment,boot_id,phase,intended_at,ended_at,ended_reason) VALUES (42,'process-group','old-boot','ended',1,2,'exited')",()).await.unwrap();
    write.commit().await.unwrap();
    drop(database);
    let database = home.open_database().await;
    assert_eq!(database.epoch(), epoch);
    let write = database.write().await.unwrap();
    let facts = cockpit_processes::output_facts(&write, cockpit_processes::ExecutionId(42))
        .await
        .unwrap();
    assert_eq!(facts.origin_epoch, None);
    assert_eq!(facts.mode, cockpit_processes::OutputMode::Unknown);
    assert!(facts.protected());
    assert!(!facts.active);
    let next = home
        .process_group_supervisor()
        .reserve(&write, database.epoch())
        .await
        .unwrap();
    assert_eq!(next.0, 43);
    write.commit().await.unwrap();
}
