//! `Sandbox::prepare_check` (#862): a project's check as the launcher's program, with no
//! provider, agent socket, launch token or session identity, following `agents.sandbox` as a
//! session does.

mod common;

use std::ffi::OsString;
use std::path::PathBuf;

use cockpit_sandbox::{
    AgentSandbox, AllowEntry, AllowSource, CheckRequest, EgressDecisions, Enforcement, HostPaths,
    NeedsSource, Platform, Prepared, Sandbox, Subject, Target, ToolNeeds,
};
use common::{
    HOMES, MOUNT_SOURCES, NO_BUBBLEWRAP, missing_backend, native_backend, no_host_paths, wsl2,
};

const CHECKOUT: &str = "/home/owner/app";
const EXECUTION: &str = "/home/owner/.local/state/torlek/executions/r7";

fn check_request() -> CheckRequest {
    CheckRequest {
        run: "r7".to_owned(),
        check: "unit".to_owned(),
        shell: PathBuf::from("/bin/sh"),
        command: "cargo test".to_owned(),
        checkout: PathBuf::from(CHECKOUT),
        start_dir: PathBuf::from(CHECKOUT),
        execution_dir: PathBuf::from(EXECUTION),
        temp_dir: PathBuf::from(format!("{EXECUTION}/tmp")),
        environment: vec![
            ("HOME".into(), "/home/owner".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("TMPDIR".into(), format!("{EXECUTION}/tmp").into()),
        ],
        tools: ToolNeeds::none(),
        egress: Vec::new(),
        decisions: EgressDecisions::default(),
    }
}

/// Host paths whose agent socket folder and bridge programs every session reaches.
fn agent_host_paths() -> HostPaths {
    HostPaths {
        read_only: vec![PathBuf::from("/opt/torlek/bridges")],
        sockets: vec![PathBuf::from("/run/user/1000/torlek")],
        denied: vec![PathBuf::from("/home/owner/.local/state/torlek/state.db")],
        ..no_host_paths()
    }
}

fn required_check() -> Prepared {
    Sandbox::new(Platform::Linux, agent_host_paths(), native_backend())
        .prepare_check(AgentSandbox::Required, &check_request())
        .unwrap()
}

#[test]
fn a_required_check_runs_its_command_in_the_shell_through_the_launcher() {
    let prepared = required_check();

    assert_eq!(
        prepared.arguments,
        [
            OsString::from("run"),
            OsString::from(format!("{MOUNT_SOURCES}/check-r7/launch-spec.json"))
        ]
    );
    let spec = prepared.staged.unwrap().spec;
    assert_eq!(
        (spec.program, spec.arguments, spec.working_dir),
        (
            PathBuf::from("/bin/sh"),
            vec!["-c".to_owned(), "cargo test".to_owned()],
            PathBuf::from(CHECKOUT)
        )
    );
}

#[test]
fn a_checks_spec_names_the_check_run_as_its_subject() {
    let spec = required_check().staged.unwrap().spec;

    assert_eq!(
        spec.subject,
        Subject::Check {
            run: "r7".to_owned(),
            check: "unit".to_owned()
        }
    );
}

#[test]
fn a_check_reaches_no_agent_socket_or_bridge_program() {
    let spec = required_check().staged.unwrap().spec;

    assert_eq!(spec.sockets, Vec::<PathBuf>::new());
    assert!(
        !spec
            .read_only
            .contains(&PathBuf::from("/opt/torlek/bridges")),
        "{:?}",
        spec.read_only
    );
}

#[test]
fn a_check_writes_its_checkout_and_its_own_folders_only() {
    let spec = required_check().staged.unwrap().spec;

    assert_eq!(
        spec.read_write,
        [
            PathBuf::from(CHECKOUT),
            PathBuf::from(EXECUTION),
            PathBuf::from(format!("{EXECUTION}/tmp"))
        ]
    );
    assert_eq!(spec.home_source, PathBuf::from(format!("{HOMES}/check-r7")));
}

#[test]
fn a_check_never_reads_the_hosts_denied_paths_and_gets_the_floor() {
    let spec = required_check().staged.unwrap().spec;

    assert!(
        spec.hidden
            .contains(&PathBuf::from("/home/owner/.local/state/torlek/state.db")),
        "{:?}",
        spec.hidden
    );
    assert_eq!(spec.needs_source, NeedsSource::Floor);
    assert_eq!(spec.read_only_declared, Vec::<PathBuf>::new());
}

#[test]
fn a_checks_environment_is_the_one_torlek_builds() {
    let prepared = required_check();

    let names: Vec<_> = prepared
        .environment
        .iter()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    for name in ["HOME", "PATH", "TMPDIR", "GIT_CONFIG_GLOBAL"] {
        assert!(names.contains(&name.to_owned()), "{name}: {names:?}");
    }
    assert!(
        !names.iter().any(|name| name.starts_with("TORLEK_")),
        "{names:?}"
    );
}

#[test]
fn a_checks_evidence_names_the_run_the_backend_and_the_landlock_abi() {
    let evidence = required_check().evidence;

    assert!(
        matches!(&evidence.enforcement, Enforcement::Sandboxed { backend, .. }
            if backend == "torlek-sandbox"),
        "{evidence:?}"
    );
    assert!(
        evidence.observed.contains(&"landlock abi: 6".to_owned())
            && evidence
                .observed
                .contains(&"subject: check run r7".to_owned()),
        "{evidence:?}"
    );
    assert!(
        evidence.rules.iter().any(|rule| rule.rule
            == "subject: check run r7 of check unit; no provider, agent socket, launch token \
                or session identity"),
        "{evidence:?}"
    );
}

#[test]
fn a_check_run_id_that_is_a_path_is_refused_naming_the_run() {
    let mut request = check_request();
    request.run = "a/b".to_owned();

    let refused = Sandbox::new(Platform::Linux, agent_host_paths(), native_backend())
        .prepare_check(AgentSandbox::Required, &request)
        .unwrap_err();

    assert!(refused.to_string().contains("check run a/b"), "{refused}");
}

#[test]
fn a_checks_launch_identity_is_named_for_its_run() {
    let evidence = required_check().evidence;

    assert!(
        matches!(&evidence.enforcement, Enforcement::Sandboxed { client_identity, .. }
            if client_identity.starts_with("check-r7-")),
        "{evidence:?}"
    );
}

#[test]
fn a_check_on_wsl2_does_not_get_the_windows_interop_socket_variable() {
    let mut request = check_request();
    request
        .environment
        .push(("WSL_INTEROP".into(), "/run/WSL/1_interop".into()));

    let prepared = Sandbox::new(wsl2(), agent_host_paths(), native_backend())
        .prepare_check(AgentSandbox::Required, &request)
        .unwrap();

    assert!(
        !prepared
            .environment
            .iter()
            .any(|(name, _)| name == "WSL_INTEROP"),
        "{:?}",
        prepared.environment
    );
}

#[test]
fn a_required_check_without_a_backend_is_refused_naming_why() {
    let refused = Sandbox::new(Platform::Linux, agent_host_paths(), missing_backend())
        .prepare_check(AgentSandbox::Required, &check_request())
        .unwrap_err();

    assert!(refused.to_string().contains(NO_BUBBLEWRAP), "{refused}");
}

#[test]
fn the_off_setting_runs_the_check_on_the_host_and_the_evidence_says_no_sandbox() {
    let prepared = Sandbox::new(Platform::Linux, agent_host_paths(), native_backend())
        .prepare_check(AgentSandbox::Off, &check_request())
        .unwrap();

    assert_eq!(
        (prepared.program, prepared.arguments),
        (
            PathBuf::from("/bin/sh"),
            vec![OsString::from("-c"), OsString::from("cargo test")]
        )
    );
    assert!(
        matches!(
            prepared.evidence.enforcement,
            Enforcement::Unsandboxed { .. }
        ),
        "{:?}",
        prepared.evidence
    );
    assert!(prepared.staged.is_none());
}

#[test]
fn a_checks_proxy_names_the_check_and_allows_no_host() {
    let spec = required_check().staged.unwrap().spec;

    assert_eq!(
        (spec.egress.program.as_str(), spec.egress.entries.len()),
        ("check unit", 0)
    );
}

#[test]
fn a_check_starts_in_its_working_directory_inside_the_checkout() {
    let request = CheckRequest {
        start_dir: PathBuf::from(format!("{CHECKOUT}/tools")),
        ..check_request()
    };

    let spec = Sandbox::new(Platform::Linux, agent_host_paths(), native_backend())
        .prepare_check(AgentSandbox::Required, &request)
        .unwrap()
        .staged
        .unwrap()
        .spec;

    assert_eq!(spec.working_dir, PathBuf::from(format!("{CHECKOUT}/tools")));
}

/// #862, Q2: the registries of the checkout's ecosystems, and the project's grants as the
/// owner's decisions, the proxy applies from the first connection.
#[test]
fn a_checks_proxy_allows_its_registries_and_its_projects_grants() {
    let registry = AllowEntry {
        target: Target::Name("index.crates.io".to_owned()),
        port: 443,
        source: AllowSource::Ecosystem {
            ecosystem: "rust".to_owned(),
        },
    };
    let grant = AllowEntry {
        target: Target::Name("git.example.org".to_owned()),
        port: 443,
        source: AllowSource::Grant {
            grant: "7".to_owned(),
        },
    };
    let request = CheckRequest {
        egress: vec![registry.clone()],
        decisions: EgressDecisions {
            allowed: vec![grant.clone()],
            ..EgressDecisions::default()
        },
        ..check_request()
    };

    let staged = Sandbox::new(Platform::Linux, agent_host_paths(), native_backend())
        .prepare_check(AgentSandbox::Required, &request)
        .unwrap()
        .staged
        .unwrap();

    assert_eq!(staged.spec.egress.entries, [registry]);
    assert_eq!(staged.decisions.allowed, [grant]);
}

/// #862, Q2: a check run holds no bound credential, so its proxy intercepts nothing and its
/// environment names no session CA.
#[test]
fn a_check_gets_no_interception_and_no_trust_variable() {
    let prepared = required_check();

    assert_eq!(prepared.staged.unwrap().spec.interception, None);
    let trust_variables: Vec<_> = prepared
        .environment
        .iter()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .filter(|name| {
            cockpit_sandbox::BUNDLE_VARIABLES.contains(&name.as_str())
                || name == cockpit_sandbox::EXTRA_CA_VARIABLE
        })
        .collect();
    assert_eq!(trust_variables, Vec::<String>::new());
}
