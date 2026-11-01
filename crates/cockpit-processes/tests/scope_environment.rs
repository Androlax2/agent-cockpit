//! A scoped execution's program gets exactly its spec's environment: the session bus
//! `systemd-run` reaches the manager through stays with `systemd-run` when the spec leaves it
//! out (#305). The supervisor reads that bus from its own process, so the test runs the
//! launches in a child of this test binary that has one.

mod common;

use std::path::Path;
use std::process::{Command, Stdio};

use common::*;

/// What the supervisor's process has and the spec leaves out.
const SUPERVISOR_BUS: &str = "unix:path=/run/user/7/bus";
/// Set only in the child: where the launched programs write what they saw.
const SEEN_DIR: &str = "COCKPIT_TEST_SCOPE_SEEN_DIR";

#[test]
fn a_scoped_program_does_not_get_the_bus_its_spec_leaves_out() {
    let seen = tempfile::tempdir().unwrap();

    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "launches_with_a_bus_in_the_supervisor",
            "--nocapture",
        ])
        .env("DBUS_SESSION_BUS_ADDRESS", SUPERVISOR_BUS)
        .env(SEEN_DIR, seen.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    for launch in ["direct", "hosted"] {
        assert_eq!(
            std::fs::read_to_string(seen.path().join(launch))
                .ok()
                .as_deref(),
            Some("unset"),
            "the {launch} launch"
        );
    }
}

/// The child of the test above: one direct and one hosted launch in a fake scope, each
/// writing the bus it got. Outside that test it has nowhere to write and does nothing.
#[tokio::test(flavor = "multi_thread")]
async fn launches_with_a_bus_in_the_supervisor() {
    let Some(seen) = std::env::var_os(SEEN_DIR) else {
        return;
    };
    let seen = Path::new(&seen);
    let home = Home::new();
    let fakes = FakeSystemd::new();
    let database = home.open_database().await;
    let supervisor = home.scope_supervisor(&fakes);

    let execution = supervisor
        .launch(&database, &bus_probe(&home, &seen.join("direct")))
        .await
        .unwrap();
    wait_until_ended(&supervisor, &database, execution.id).await;
    let hosted = supervisor
        .prepare_hosted(&database, &bus_probe(&home, &seen.join("hosted")), &[])
        .await
        .unwrap();
    let status = Command::new(&hosted.command[0])
        .args(&hosted.command[1..])
        .current_dir(home.path())
        .status()
        .unwrap();

    assert!(status.success());
}

/// `shell` writing its `DBUS_SESSION_BUS_ADDRESS`, or `unset`, to `seen`.
fn bus_probe(home: &Home, seen: &Path) -> cockpit_processes::LaunchSpec {
    shell(
        home,
        &format!(
            "printf '%s' \"${{DBUS_SESSION_BUS_ADDRESS-unset}}\" > '{}'",
            seen.display()
        ),
    )
}
