//! `Sandbox::prepare`, the one entry every launch goes through, and the evidence it records:
//! sandboxed by default, refused without an engine, unsandboxed only by the owner's setting.

mod common;

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_sandbox::{AgentSandbox, Enforcement, HostPaths, MIGRATIONS, Platform, Sandbox};
use common::{OWNER_UID, launch_request, linux_engine, no_host_paths};

fn agent_words() -> (PathBuf, Vec<OsString>) {
    (PathBuf::from("/usr/bin/claude"), vec!["--resume".into()])
}

#[test]
fn a_required_sandbox_wraps_the_agent_in_greywall() {
    let settings = tempfile::tempdir().unwrap();
    let sandbox = Sandbox::new(
        Platform::Linux,
        Ok(linux_engine(settings.path())),
        no_host_paths(),
    );
    let (program, arguments) = agent_words();

    let prepared = sandbox
        .prepare(
            AgentSandbox::Required,
            &launch_request("s1", "/home/owner/app"),
            program,
            arguments,
        )
        .unwrap();

    assert_eq!(prepared.program, PathBuf::from("/opt/sandbox/greywall"));
    let tail: Vec<&OsString> = prepared.arguments.iter().rev().take(3).collect();
    assert_eq!(
        tail,
        [
            &OsString::from("--resume"),
            &"/usr/bin/claude".into(),
            &"--".into()
        ]
    );
    let Enforcement::Sandboxed {
        backend,
        client_identity,
        ..
    } = &prepared.evidence.enforcement
    else {
        panic!("expected sandboxed evidence: {:?}", prepared.evidence);
    };
    assert_eq!(backend, "greywall");
    assert!(client_identity.starts_with("cockpit-s1-"));
    assert_eq!(prepared.evidence.platform, "linux");
}

#[test]
fn without_an_engine_a_required_sandbox_refuses_the_launch_with_the_reason() {
    let sandbox = Sandbox::unavailable("no `greywall` on PATH");
    let (program, arguments) = agent_words();

    let refused = sandbox
        .prepare(
            AgentSandbox::Required,
            &launch_request("s1", "/home/owner/app"),
            program,
            arguments,
        )
        .unwrap_err();

    assert_eq!(refused.rule, "sandbox");
    assert!(
        refused.reason.contains("no `greywall` on PATH"),
        "{refused}"
    );
}

#[test]
fn the_owners_off_setting_runs_the_agent_unchanged_and_the_evidence_says_so() {
    let sandbox = Sandbox::unavailable("no `greywall` on PATH");
    let (program, arguments) = agent_words();

    let prepared = sandbox
        .prepare(
            AgentSandbox::Off,
            &launch_request("s1", "/home/owner/app"),
            program.clone(),
            arguments.clone(),
        )
        .unwrap();

    assert_eq!((prepared.program, prepared.arguments), (program, arguments));
    assert!(prepared.settings.is_none());
    assert_eq!(
        prepared.evidence.enforcement,
        Enforcement::Unsandboxed {
            reason: "the owner set agents.sandbox = \"off\"".to_owned()
        }
    );
}

#[test]
fn a_hosts_denied_path_reaches_the_profile() {
    let settings = tempfile::tempdir().unwrap();
    let sandbox = Sandbox::new(
        Platform::Linux,
        Ok(linux_engine(settings.path())),
        HostPaths {
            read_only: vec![PathBuf::from("/run/user/1000/cockpit/agent")],
            denied: vec![PathBuf::from("/run/user/1000/cockpit/daemon.sock")],
            owner_uid: OWNER_UID,
        },
    );

    let profile = sandbox
        .profile(&launch_request("s1", "/home/owner/app"))
        .unwrap();

    assert!(
        profile
            .rules()
            .contains(&"deny /run/user/1000/cockpit/daemon.sock".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_evidence_is_read_back_as_recorded_for_the_sessions_latest_launch() {
    let dir = tempfile::tempdir().unwrap();
    let database = cockpit_turso::Database::open(&dir.path().join("state.db"), &[MIGRATIONS])
        .await
        .unwrap();
    let settings = tempfile::tempdir().unwrap();
    let sandbox = Sandbox::new(
        Platform::Linux,
        Ok(linux_engine(settings.path())),
        no_host_paths(),
    );
    let unsandboxed = sandbox
        .prepare(
            AgentSandbox::Off,
            &launch_request("s1", "/home/owner/app"),
            "/usr/bin/claude".into(),
            Vec::new(),
        )
        .unwrap()
        .evidence;
    let sandboxed = sandbox
        .prepare(
            AgentSandbox::Required,
            &launch_request("s1", "/home/owner/app"),
            "/usr/bin/claude".into(),
            Vec::new(),
        )
        .unwrap()
        .evidence;
    let write = database.write().await.unwrap();
    cockpit_sandbox::record(&write, 1, "s1", &unsandboxed)
        .await
        .unwrap();
    cockpit_sandbox::record(&write, 2, "s1", &sandboxed)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let read = database.read().await.unwrap();
    let latest = cockpit_sandbox::latest_for_session(&read, "s1")
        .await
        .unwrap();
    let none = cockpit_sandbox::latest_for_session(&read, "s2")
        .await
        .unwrap();

    assert_eq!(latest, Some(sandboxed));
    assert_eq!(none, None);
}
