//! Report 02: approvals is a capability the daemon composes. It builds against the state
//! database, authorization and processes (approved runs are executions, #163) only: never
//! the wire types, the agent channel (the daemon identifies the sender) or projects (the
//! daemon finds the asking agent's project).
use std::path::Path;
use std::process::Command;

const ALLOWED_COCKPIT_DEPENDENCIES: &[&str] = &[
    "cockpit-approvals",
    "cockpit-authorization",
    "cockpit-processes",
    "cockpit-turso",
];

#[test]
fn cockpit_approvals_depends_on_authorization_processes_and_the_database_only() {
    let root_manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .arg("tree")
        .arg("--manifest-path")
        .arg(&root_manifest)
        .args(["--locked", "--target", "all"])
        .args(["--edges", "normal,build,dev", "--prefix", "none"])
        .args(["--format", "{p}", "--package", "cockpit-approvals"])
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut forbidden: Vec<String> = String::from_utf8(output.stdout)
        .expect("cargo tree prints UTF-8")
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|package| package.starts_with("cockpit-"))
        .filter(|package| !ALLOWED_COCKPIT_DEPENDENCIES.contains(package))
        .map(str::to_string)
        .collect();
    forbidden.sort();
    forbidden.dedup();
    assert_eq!(forbidden, Vec::<String>::new());
}
