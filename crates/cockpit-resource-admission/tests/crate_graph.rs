//! Report 02: admission is a lower capability that scheduling calls. It builds against the
//! state database and the executions it charges, never scheduling, sessions or tasks.
use std::path::Path;
use std::process::Command;

const ALLOWED_COCKPIT_DEPENDENCIES: &[&str] = &[
    "cockpit-processes",
    "cockpit-resource-admission",
    "cockpit-turso",
];

#[test]
fn cockpit_resource_admission_depends_on_processes_and_the_database_only() {
    let root_manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .arg("tree")
        .arg("--manifest-path")
        .arg(&root_manifest)
        .args(["--locked", "--target", "all"])
        .args(["--edges", "normal,build,dev", "--prefix", "none"])
        .args(["--format", "{p}", "--package", "cockpit-resource-admission"])
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
