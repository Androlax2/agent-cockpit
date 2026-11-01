//! `probe_docker` against stand-in `docker` programs (#50): an engine identified by its own
//! answer, a client whose engine is down, and one that never answers.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cockpit_readiness::probe_docker;
use tempfile::TempDir;

fn fake_docker(dir: &Path, script: &str) -> PathBuf {
    let program = dir.join("docker");
    std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

#[test]
fn an_engine_is_identified_by_what_docker_info_answers() {
    let dir = TempDir::new().unwrap();
    let docker = fake_docker(
        dir.path(),
        r#"[ "$1 $2 $3" = 'info --format {{json .}}' ] || exit 9
printf '%s\n' '{"ID":"7c2b-engine","Name":"box","ServerVersion":"28.1.1","SecurityOptions":["name=seccomp,profile=builtin","name=rootless"]}'"#,
    );

    let engine = probe_docker(&docker, Duration::from_secs(10)).unwrap();

    assert_eq!(engine.id, "7c2b-engine");
    assert_eq!(engine.name, "box");
    assert_eq!(engine.server_version, "28.1.1");
    assert!(engine.rootless);
}

#[test]
fn a_docker_program_without_an_engine_names_why() {
    let dir = TempDir::new().unwrap();
    let docker = fake_docker(
        dir.path(),
        "echo 'Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the docker daemon running?' >&2; exit 1",
    );

    let error = probe_docker(&docker, Duration::from_secs(10)).unwrap_err();

    assert!(error.contains("no engine answered"), "{error}");
    assert!(
        error.contains("Cannot connect to the Docker daemon"),
        "{error}"
    );
}

#[test]
fn an_engine_that_does_not_answer_in_time_is_unreachable() {
    let dir = TempDir::new().unwrap();
    let docker = fake_docker(dir.path(), "exec sleep 30");
    let started = Instant::now();

    let error = probe_docker(&docker, Duration::from_millis(300)).unwrap_err();

    assert!(error.contains("did not answer within"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn no_docker_program_is_no_engine() {
    let dir = TempDir::new().unwrap();

    let error = probe_docker(&dir.path().join("docker"), Duration::from_secs(1)).unwrap_err();

    assert!(error.contains("does not run"), "{error}");
}
