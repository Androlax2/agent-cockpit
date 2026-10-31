//! The protocol owns published wire types only (issue #20): no database, UI or policy, and no
//! other Cockpit crate, so every frontend and plugin can depend on it without pulling those in.
//! The workspace-wide forbidden-edge rule is issue #19; this guards this crate's own edges.

use std::process::Command;

use serde_json::Value;

const ALLOWED_DEPENDENCIES: [&str; 2] = ["serde", "serde_json"];

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
    let protocol = metadata["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|package| package["name"] == env!("CARGO_PKG_NAME"))
        .expect("this package is in the workspace");
    protocol["dependencies"]
        .as_array()
        .expect("dependencies")
        .iter()
        .filter(|dependency| dependency["kind"] != "dev")
        .map(|dependency| dependency["name"].as_str().expect("name").to_owned())
        .collect()
}

#[test]
fn the_protocol_depends_only_on_its_serialization_crates() {
    let unexpected: Vec<String> = runtime_dependency_names()
        .into_iter()
        .filter(|name| !ALLOWED_DEPENDENCIES.contains(&name.as_str()))
        .collect();

    assert_eq!(unexpected, Vec::<String>::new());
}
