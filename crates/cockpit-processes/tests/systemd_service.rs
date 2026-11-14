//! Confined executions (#1387): a transient systemd user service per execution, started with
//! `systemd-run --pipe --wait`, whose sandboxing hides paths, keeps every folder but the
//! writable ones read-only and gives it no network. The fake `systemd-run` records the
//! properties and runs the command in place; the tests marked ignored need this user's real
//! systemd manager, which enforces them.
#![cfg(target_os = "linux")]

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use cockpit_processes::{Confinement, Containment, EndReason, Error, Phase, Supervisor};
use cockpit_turso::Database;
use common::*;

fn folder(home: &Home, name: &str) -> PathBuf {
    let path = home.path().join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn confinement(home: &Home) -> Confinement {
    Confinement {
        writable: vec![folder(home, "clone dir")],
        hidden: vec![folder(home, "secrets")],
        emptied: vec![folder(home, "state")],
    }
}

async fn reserve_confined(
    supervisor: &Supervisor,
    database: &Database,
) -> cockpit_processes::ExecutionId {
    let write = database.write().await.unwrap();
    let id = supervisor
        .reserve_confined(&write, database.epoch())
        .await
        .unwrap();
    write.commit().await.unwrap();
    id
}

fn property(arguments: &[String], value: &str) -> bool {
    arguments
        .windows(2)
        .any(|pair| pair[0] == "-p" && pair[1] == value)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_execution_runs_in_its_own_service_unit() {
    let home = Home::new();
    let database = home.open_database().await;
    let fakes = FakeSystemd::new();
    let supervisor = home.scope_supervisor(&fakes);
    let id = reserve_confined(&supervisor, &database).await;

    let launched = supervisor
        .launch_confined(&database, id, &shell(&home, "exit 3"), &confinement(&home))
        .await
        .unwrap();

    assert_eq!(
        launched.execution.containment,
        Containment::SystemdService {
            unit: format!("cockpit-{}-{}.service", database.epoch(), id.0)
        }
    );
    assert_eq!(launched.exit.await.unwrap().unwrap().code(), Some(3));
    let arguments = fakes.last_arguments();
    for switch in [
        "--user",
        "--pipe",
        "--wait",
        "--collect",
        "--expand-environment=no",
    ] {
        assert!(
            arguments.iter().any(|argument| argument == switch),
            "{switch}: {arguments:?}"
        );
    }
    assert!(
        !arguments.iter().any(|argument| argument == "--scope"),
        "{arguments:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_execution_gets_its_limits_as_service_properties() {
    let home = Home::new();
    let database = home.open_database().await;
    let fakes = FakeSystemd::new();
    let supervisor = home.scope_supervisor(&fakes);
    let id = reserve_confined(&supervisor, &database).await;
    let limits = confinement(&home);

    let launched = supervisor
        .launch_confined(&database, id, &shell(&home, "exit 0"), &limits)
        .await
        .unwrap();
    launched.exit.await.unwrap().unwrap();

    let arguments = fakes.last_arguments();
    let path = |path: &Path| path.display().to_string();
    let execution_dir = supervisor.execution_dir(&database, id);
    for expected in [
        "PrivateUsers=yes".to_owned(),
        "PrivateNetwork=yes".to_owned(),
        "PrivateTmp=yes".to_owned(),
        "ReadOnlyPaths=/".to_owned(),
        format!("InaccessiblePaths=-\"{}\"", path(&limits.hidden[0])),
        format!("TemporaryFileSystem=\"{}:ro\"", path(&limits.emptied[0])),
        format!("BindPaths=\"{}\"", path(&limits.writable[0])),
        format!("BindPaths=\"{}\"", path(&execution_dir)),
    ] {
        assert!(property(&arguments, &expected), "{expected}: {arguments:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_program_gets_exactly_its_specs_environment() {
    let home = Home::new();
    let database = home.open_database().await;
    let fakes = FakeSystemd::new();
    let supervisor = home.scope_supervisor(&fakes);
    let id = reserve_confined(&supervisor, &database).await;
    let seen = home.path().join("seen");
    let mut spec = shell(
        &home,
        &format!(
            "printf '%s|%s|%s' \"$GREETING\" \"${{XDG_RUNTIME_DIR-unset}}\" \"${{HOME-unset}}\" > {}",
            seen.display()
        ),
    );
    spec.environment
        .push((OsString::from("GREETING"), OsString::from("it's $HOME")));

    let launched = supervisor
        .launch_confined(&database, id, &spec, &confinement(&home))
        .await
        .unwrap();
    launched.exit.await.unwrap().unwrap();

    assert_eq!(
        std::fs::read_to_string(&seen).unwrap(),
        "it's $HOME|unset|unset"
    );
    assert!(
        !supervisor
            .execution_dir(&database, id)
            .join("environment")
            .exists(),
        "the environment file is removed before the program runs"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_execution_ends_observed_like_a_scope() {
    let home = Home::new();
    let database = home.open_database().await;
    let fakes = FakeSystemd::new();
    let supervisor = home.scope_supervisor(&fakes);
    let id = reserve_confined(&supervisor, &database).await;

    let launched = supervisor
        .launch_confined(&database, id, &shell(&home, "exit 0"), &confinement(&home))
        .await
        .unwrap();
    assert_eq!(launched.execution.phase, Phase::Running);
    launched.exit.await.unwrap().unwrap();

    let ended = wait_until_ended(&supervisor, &database, id).await;
    assert_eq!(ended.phase, Phase::Ended(EndReason::Exited));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_execution_is_terminated_through_its_unit() {
    let home = Home::new();
    let database = home.open_database().await;
    let fakes = FakeSystemd::new();
    let supervisor = home.scope_supervisor(&fakes);
    let id = reserve_confined(&supervisor, &database).await;
    let launched = supervisor
        .launch_confined(
            &database,
            id,
            &shell(&home, "exec sleep 30"),
            &confinement(&home),
        )
        .await
        .unwrap();
    let unit = format!("cockpit-{}-{}.service", database.epoch(), id.0);

    let termination = supervisor
        .terminate(&database, id, QUICK_ESCALATION)
        .await
        .unwrap();

    assert!(
        matches!(
            termination,
            cockpit_processes::Termination::Confirmed { .. }
        ),
        "{termination:?}"
    );
    assert!(
        fakes
            .calls()
            .iter()
            .any(|call| call.contains("kill") && call.ends_with(&unit)),
        "{:?}",
        fakes.calls()
    );
    drop(launched);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_reservation_is_refused_without_a_systemd_manager() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let write = database.write().await.unwrap();

    let refused = supervisor.reserve_confined(&write, database.epoch()).await;

    assert!(
        matches!(refused, Err(Error::ConfinementUnavailable { .. })),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_reservation_is_launched_confined_only() {
    let home = Home::new();
    let database = home.open_database().await;
    let fakes = FakeSystemd::new();
    let supervisor = home.scope_supervisor(&fakes);
    let id = reserve_confined(&supervisor, &database).await;

    let refused = supervisor
        .launch_reserved(&database, id, &shell(&home, "exit 0"))
        .await;

    assert!(
        matches!(refused, Err(Error::NotReserved { .. })),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_launch_refuses_systemd_before_254() {
    let home = Home::new();
    let database = home.open_database().await;
    let fakes = FakeSystemd::new();
    fakes.report_version(253);
    let supervisor = home.scope_supervisor(&fakes);
    let id = reserve_confined(&supervisor, &database).await;

    let refused = supervisor
        .launch_confined(&database, id, &shell(&home, "exit 0"), &confinement(&home))
        .await;

    assert!(
        matches!(refused, Err(Error::ConfinementUnavailable { .. })),
        "{refused:?}"
    );
    let ended = cockpit_processes::execution(&database, id).await.unwrap();
    assert_eq!(ended.phase, Phase::Ended(EndReason::NeverStarted));
}

/// The real manager enforces what the properties say: a hidden file cannot be read, a
/// folder outside the writable ones cannot be written, and there is no network.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_execution_is_held_to_its_limits() {
    let tools = cockpit_processes::SystemdTools::from_path();
    assert!(
        tools.manager_reachable().await,
        "no reachable systemd user manager"
    );
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = Supervisor::new(
        home.executions_dir(),
        cockpit_processes::LaunchContainment::SystemdScope,
        tools,
    );
    let limits = confinement(&home);
    std::fs::write(limits.hidden[0].join("id_test"), "secret").unwrap();
    let report = limits.writable[0].join("report");
    let id = reserve_confined(&supervisor, &database).await;
    let script = format!(
        "{{ cat '{hidden}' && echo read; echo x > '{outside}' && echo wrote; \
         echo y > /tmp/scratch && echo temp; \
         exec 3<>/dev/tcp/1.1.1.1/53 && echo connected; }} > '{report}' 2>&1",
        hidden = limits.hidden[0].join("id_test").display(),
        outside = home.path().join("outside").display(),
        report = report.display(),
    );
    let mut spec = shell(&home, &script);
    spec.program = PathBuf::from("/bin/bash");

    let launched = supervisor
        .launch_confined(&database, id, &spec, &limits)
        .await
        .unwrap();
    let status = launched.exit.await.unwrap().unwrap();

    let seen = std::fs::read_to_string(&report).unwrap_or_else(|error| {
        let stderr = supervisor.output_path(&database, id, cockpit_processes::OutputKind::Stderr);
        panic!(
            "{error}; {status}; the launcher wrote: {}",
            std::fs::read_to_string(stderr).unwrap_or_default()
        )
    });
    assert!(seen.contains("Permission denied"), "{seen}");
    assert!(!seen.contains("\nread"), "{seen}");
    assert!(seen.contains("Read-only file system"), "{seen}");
    assert!(seen.contains("temp"), "{seen}");
    assert!(!seen.contains("connected"), "{seen}");
}

/// The migration that admits services rebuilds the executions table: a scope execution
/// recorded before it keeps its row, unit included.
#[tokio::test(flavor = "multi_thread")]
async fn an_execution_recorded_before_services_keeps_its_scope() {
    const BEFORE_SERVICES: cockpit_turso::MigrationSet = cockpit_turso::MigrationSet {
        owner: "processes",
        migrations: cockpit_processes::MIGRATIONS.migrations.split_at(3).0,
    };
    let home = Home::new();
    let path = home.path().join("state.db");
    {
        let database = Database::open(&path, &[BEFORE_SERVICES]).await.unwrap();
        let write = database.write().await.unwrap();
        write
            .execute(
                "INSERT INTO process_executions (id, containment, unit, boot_id, phase,
                    intended_at, ended_at, ended_reason)
                 VALUES (7, 'systemd-scope', 'cockpit-e-7.scope', 'boot', 'ended', 1, 2,
                    'exited')",
                (),
            )
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    let database = home.open_database().await;

    let kept = cockpit_processes::execution(&database, cockpit_processes::ExecutionId(7))
        .await
        .unwrap();
    assert_eq!(
        kept.containment,
        Containment::SystemdScope {
            unit: "cockpit-e-7.scope".to_owned()
        }
    );
    assert_eq!(kept.phase, Phase::Ended(EndReason::Exited));
}
