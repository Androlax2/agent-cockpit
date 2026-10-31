//! Report 02 (amended by #77): frontends import `cockpit-client` and `cockpit-protocol`, plus
//! only the named capability crates whose verbs must work without the daemon.
//!
//! Reads the resolved graph in `Cargo.lock`, so target-specific, renamed and transitive
//! dependencies count the same as a plain `[dependencies]` line.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use toml::{Table, Value};

const ALLOWED_PRODUCT_DEPENDENCIES: [&str; 3] =
    ["cockpit-client", "cockpit-protocol", "cockpit-config"];

/// Every `cockpit-*` package reachable from `root` in a Cargo.lock, `root` excluded.
fn reachable_product_crates(lock: &str, root: &str) -> BTreeSet<String> {
    let lock: Table = toml::from_str(lock).expect("Cargo.lock is TOML");
    let packages = lock["package"]
        .as_array()
        .expect("Cargo.lock lists packages");
    let dependencies_of = |name: &str| -> Vec<String> {
        packages
            .iter()
            .filter(|package| package["name"].as_str() == Some(name))
            .flat_map(|package| package.get("dependencies").and_then(Value::as_array))
            .flatten()
            // An entry is "name" or, when versions are ambiguous, "name version".
            .filter_map(|entry| entry.as_str()?.split(' ').next().map(str::to_string))
            .collect()
    };
    let mut reached = BTreeSet::new();
    let mut pending = vec![root.to_string()];
    while let Some(name) = pending.pop() {
        for dependency in dependencies_of(&name) {
            if reached.insert(dependency.clone()) {
                pending.push(dependency);
            }
        }
    }
    reached
        .into_iter()
        .filter(|name| name.starts_with("cockpit-") && name != root)
        .collect()
}

#[test]
fn the_cli_reaches_only_allowed_product_crates() {
    let lock_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let lock = fs::read_to_string(&lock_path).unwrap();
    let forbidden: Vec<String> = reachable_product_crates(&lock, "cockpit-cli")
        .into_iter()
        .filter(|name| !ALLOWED_PRODUCT_DEPENDENCIES.contains(&name.as_str()))
        .collect();
    assert_eq!(forbidden, Vec::<String>::new());
}

#[test]
fn a_crate_reached_through_an_allowed_one_is_caught() {
    let lock = r#"
[[package]]
name = "cockpit-cli"
version = "0.1.0"
dependencies = ["cockpit-config", "tempfile"]

[[package]]
name = "cockpit-config"
version = "0.1.0"
dependencies = ["cockpit-turso 0.1.0", "serde"]

[[package]]
name = "cockpit-turso"
version = "0.1.0"
"#;
    assert_eq!(
        reachable_product_crates(lock, "cockpit-cli"),
        BTreeSet::from(["cockpit-config".to_string(), "cockpit-turso".to_string()])
    );
}
