//! The client is what every frontend links (report 02): it must stay free of persistence,
//! scheduling and every other capability, so it reaches the daemon only through the
//! protocol. The workspace-wide `clients ->` rules are in cockpit-workspace-rules; this
//! guards the client's own direct edges.

use std::process::Command;

use serde_json::Value;

const ALLOWED_DEPENDENCIES: [&str; 4] = ["cockpit-protocol", "serde", "serde_json", "tokio"];

/// Package names (not renames) of this crate's normal and build dependencies, every target.
fn runtime_dependency_names() -> Vec<String> {
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ])
        .arg("--manifest-path")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON");
    let client = metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|package| package["name"] == env!("CARGO_PKG_NAME"))
        .expect("this package is in the workspace");
    client["dependencies"]
        .as_array()
        .expect("dependencies")
        .iter()
        .filter(|dependency| dependency["kind"] != "dev")
        .map(|dependency| dependency["name"].as_str().expect("name").to_owned())
        .collect()
}

#[test]
fn the_client_depends_only_on_the_protocol_and_its_transport_crates() {
    let unexpected: Vec<String> = runtime_dependency_names()
        .into_iter()
        .filter(|name| !ALLOWED_DEPENDENCIES.contains(&name.as_str()))
        .collect();

    assert_eq!(unexpected, Vec::<String>::new());
}
