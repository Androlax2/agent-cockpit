//! The folder a project engine's proxy shares with the daemon (#390 part 6c), beside the
//! sessions' folders of mount sources: the owner's decisions in, the refused hosts out.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cockpit_sandbox::{
    AskedDestination, EgressDecisions, EgressRequests, HostPaths, Platform, Sandbox, Subject,
};
use common::{native_backend, no_host_paths};

fn sandbox_in(mount_sources: &Path) -> Sandbox {
    Sandbox::new(
        Platform::Linux,
        HostPaths {
            mount_sources: mount_sources.to_path_buf(),
            ..no_host_paths()
        },
        native_backend(),
    )
}

fn containers_of(project: &str) -> Subject {
    Subject::Containers {
        project: project.to_owned(),
    }
}

fn asked(host: &str) -> EgressRequests {
    EgressRequests {
        subject: containers_of("app"),
        asked: vec![AskedDestination {
            host: host.to_owned(),
            port: 443,
        }],
        revision: 1,
        services: Vec::new(),
    }
}

#[test]
fn an_engines_proxy_folder_is_made_for_the_owner_alone() {
    let mounts = tempfile::tempdir().unwrap();

    let folder = sandbox_in(mounts.path())
        .prepare_engine_proxy_folder("app")
        .unwrap()
        .unwrap();

    assert_eq!(folder, mounts.path().join("engine-app"));
    let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

/// A restarted proxy asks afresh: what the last one refused is filed already.
#[test]
fn a_prepared_folder_holds_no_request_or_counters_of_the_last_proxy() {
    let mounts = tempfile::tempdir().unwrap();
    let sandbox = sandbox_in(mounts.path());
    let folder = sandbox.prepare_engine_proxy_folder("app").unwrap().unwrap();
    std::fs::write(
        folder.join(cockpit_sandbox::REQUESTS_FILE),
        serde_json::to_vec(&asked("crates.io")).unwrap(),
    )
    .unwrap();
    std::fs::write(folder.join(cockpit_sandbox::EGRESS_FILE), b"{}").unwrap();

    sandbox.prepare_engine_proxy_folder("app").unwrap();

    assert_eq!(sandbox.engine_egress_requests("app"), Ok(None));
    assert!(!folder.join(cockpit_sandbox::EGRESS_FILE).exists());
}

#[test]
fn the_hosts_an_engines_proxy_refused_are_read_from_its_folder() {
    let mounts = tempfile::tempdir().unwrap();
    let sandbox = sandbox_in(mounts.path());
    let folder = sandbox.prepare_engine_proxy_folder("app").unwrap().unwrap();
    std::fs::write(
        folder.join(cockpit_sandbox::REQUESTS_FILE),
        serde_json::to_vec(&asked("crates.io")).unwrap(),
    )
    .unwrap();

    assert_eq!(
        sandbox.engine_egress_requests("app"),
        Ok(Some(asked("crates.io")))
    );
}

#[test]
fn decisions_reach_an_engines_proxy_folder_only_once_it_is_prepared() {
    let mounts = tempfile::tempdir().unwrap();
    let sandbox = sandbox_in(mounts.path());

    assert_eq!(
        sandbox.write_engine_egress_decisions("app", &EgressDecisions::default()),
        Ok(false)
    );
    let folder = sandbox.prepare_engine_proxy_folder("app").unwrap().unwrap();
    assert_eq!(
        sandbox.write_engine_egress_decisions("app", &EgressDecisions::default()),
        Ok(true)
    );
    let written = std::fs::read(folder.join(cockpit_sandbox::DECISIONS_FILE)).unwrap();
    assert_eq!(
        serde_json::from_slice::<EgressDecisions>(&written).unwrap(),
        EgressDecisions::default()
    );
}

#[test]
fn a_project_name_that_is_not_one_plain_name_names_no_folder() {
    let mounts = tempfile::tempdir().unwrap();

    let refused = sandbox_in(mounts.path()).prepare_engine_proxy_folder("../app");

    assert!(refused.is_err(), "{refused:?}");
}
