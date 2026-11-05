//! `Sandbox::prepare`, the one entry every launch goes through, and the evidence it records:
//! with no backend in this release (#828), `agents.sandbox = "off"` runs the agent unchanged
//! and says so, and `"required"` refuses the launch naming the missing backend.

mod common;

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_sandbox::{
    AgentSandbox, AppliedRule, Enforcement, Evidence, HostPaths, MIGRATIONS, Platform, Sandbox,
};
use common::{MOUNT_SOURCES, OWNER_UID, launch_request, no_host_paths};

fn agent_words() -> (PathBuf, Vec<OsString>) {
    (PathBuf::from("/usr/bin/claude"), vec!["--resume".into()])
}

fn sandboxes() -> [(&'static str, Sandbox); 3] {
    [
        ("linux", Sandbox::new(Platform::Linux, no_host_paths())),
        ("macos", Sandbox::new(Platform::MacOs, no_host_paths())),
        ("host paths unread", Sandbox::unavailable()),
    ]
}

/// The security rule of #828: an explicit `required` never falls back to running the agent
/// unsandboxed, whatever the host.
#[test]
fn a_required_sandbox_refuses_every_launch_naming_the_missing_backend() {
    for (case, sandbox) in sandboxes() {
        let (program, arguments) = agent_words();

        let refused = sandbox
            .prepare(
                AgentSandbox::Required,
                &launch_request("s1", "/home/owner/app"),
                program,
                arguments,
            )
            .unwrap_err();

        assert_eq!(refused.rule, "agents.sandbox = \"required\"", "{case}");
        assert!(
            refused.reason.contains(cockpit_config::NO_SANDBOX_BACKEND),
            "{case}: {refused}"
        );
        assert!(
            refused
                .reason
                .contains("refused rather than run unsandboxed"),
            "{case}: {refused}"
        );
    }
}

#[test]
fn the_off_setting_runs_the_agent_unchanged_and_the_evidence_says_no_sandbox() {
    let sandbox = Sandbox::new(Platform::Linux, no_host_paths());
    let (program, arguments) = agent_words();
    let request = launch_request("s1", "/home/owner/app");

    let prepared = sandbox
        .prepare(
            AgentSandbox::Off,
            &request,
            program.clone(),
            arguments.clone(),
        )
        .unwrap();

    assert_eq!((prepared.program, prepared.arguments), (program, arguments));
    assert_eq!(prepared.environment, request.environment);
    assert_eq!(
        prepared.evidence,
        Evidence {
            enforcement: Enforcement::Unsandboxed {
                reason: "no sandbox: agents.sandbox = \"off\"".to_owned()
            },
            platform: "linux".to_owned(),
            rules: Vec::new(),
            observed: Vec::new(),
        }
    );
}

#[test]
fn a_hosts_denied_path_reaches_the_profile() {
    let sandbox = Sandbox::new(
        Platform::Linux,
        HostPaths {
            read_only: vec![PathBuf::from("/run/user/1000/cockpit/agent")],
            denied: vec![PathBuf::from("/run/user/1000/cockpit/daemon.sock")],
            denied_write: Vec::new(),
            owner_uid: OWNER_UID,
            mount_sources: PathBuf::from(MOUNT_SOURCES),
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

#[test]
fn a_launch_requests_own_denied_path_reaches_its_profile_beside_the_hosts() {
    let sandbox = Sandbox::new(
        Platform::Linux,
        HostPaths {
            read_only: vec![PathBuf::from("/run/user/1000/cockpit/agent")],
            denied: vec![PathBuf::from("/run/user/1000/cockpit/daemon.sock")],
            denied_write: Vec::new(),
            owner_uid: OWNER_UID,
            mount_sources: PathBuf::from(MOUNT_SOURCES),
        },
    );
    let mut request = launch_request("s1", "/home/owner/app");
    request.denied = vec![PathBuf::from("/home/owner/.config/gh")];

    let rules = sandbox.profile(&request).unwrap().rules();

    for rule in [
        "deny /run/user/1000/cockpit/daemon.sock",
        "deny /home/owner/.config/gh",
    ] {
        assert!(rules.contains(&rule.to_owned()), "{rule} in {rules:?}");
    }
}

#[test]
fn a_sandbox_that_read_no_host_paths_refuses_a_profile() {
    let refused = Sandbox::unavailable()
        .profile(&launch_request("s1", "/home/owner/app"))
        .unwrap_err();

    assert_eq!(refused.rule, "sandbox");
}

/// A launch an earlier release sandboxed keeps its evidence readable after the upgrade.
#[tokio::test(flavor = "multi_thread")]
async fn the_evidence_is_read_back_as_recorded_for_the_sessions_latest_launch() {
    let dir = tempfile::tempdir().unwrap();
    let database = cockpit_turso::Database::open(&dir.path().join("state.db"), &[MIGRATIONS])
        .await
        .unwrap();
    let unsandboxed = Sandbox::new(Platform::Linux, no_host_paths())
        .prepare(
            AgentSandbox::Off,
            &launch_request("s1", "/home/owner/app"),
            "/usr/bin/claude".into(),
            Vec::new(),
        )
        .unwrap()
        .evidence;
    let earlier_release = Evidence {
        enforcement: Enforcement::Sandboxed {
            backend: "an earlier backend".to_owned(),
            engine: "engine 1.0".to_owned(),
            client_identity: "cockpit-s1-0001".to_owned(),
        },
        platform: "linux".to_owned(),
        rules: vec![AppliedRule {
            rule: "read-write /home/owner/app".to_owned(),
            enforced_by: "the earlier backend".to_owned(),
        }],
        observed: vec!["landlock".to_owned()],
    };
    let write = database.write().await.unwrap();
    cockpit_sandbox::record(&write, 1, "s1", &earlier_release)
        .await
        .unwrap();
    cockpit_sandbox::record(&write, 2, "s1", &unsandboxed)
        .await
        .unwrap();
    cockpit_sandbox::record(&write, 3, "s2", &earlier_release)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let read = database.read().await.unwrap();
    let latest = cockpit_sandbox::latest_for_session(&read, "s1")
        .await
        .unwrap();
    let earlier = cockpit_sandbox::latest_for_session(&read, "s2")
        .await
        .unwrap();
    let none = cockpit_sandbox::latest_for_session(&read, "s3")
        .await
        .unwrap();

    assert_eq!(latest, Some(unsandboxed));
    assert_eq!(earlier, Some(earlier_release));
    assert_eq!(none, None);
}
