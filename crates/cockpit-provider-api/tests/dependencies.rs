//! The provider contract is words and types on the standard library alone; adapters bring
//! whatever their provider needs.

use std::process::Command;

#[test]
fn the_contract_has_no_dependency() {
    let output = Command::new(env!("CARGO"))
        .args(["tree", "--locked", "--offline", "--target", "all"])
        .args([
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .arg("--manifest-path")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .args(["--package", env!("CARGO_PKG_NAME")])
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let packages: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect();

    assert_eq!(packages, [env!("CARGO_PKG_NAME")]);
}
