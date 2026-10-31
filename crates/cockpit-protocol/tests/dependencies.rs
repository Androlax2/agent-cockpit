//! The protocol owns published wire types only (issue #20): no database, UI or policy, and no
//! other Cockpit crate, so every frontend and plugin can depend on it without pulling those in.
//! The workspace-wide forbidden-edge rule is issue #19; this guards this crate's own edges.

use std::fs;
use std::path::Path;

const ALLOWED_DEPENDENCIES: [&str; 2] = ["serde", "serde_json"];

/// The names under `[dependencies]`, `[build-dependencies]` and target-specific variants.
/// Not a TOML parser: enough for `name = ...` lines and `[dependencies.name]` tables.
fn dependency_names(manifest: &str) -> Vec<String> {
    let mut in_dependency_table = false;
    let mut names = Vec::new();
    for line in manifest.lines() {
        let line = line.split('#').next().unwrap().trim();
        if line.starts_with('[') {
            let table = line.trim_matches(['[', ']']);
            in_dependency_table = table.ends_with("dependencies") && table != "dev-dependencies";
            if let Some((owner, name)) = table.rsplit_once('.')
                && owner.ends_with("dependencies")
                && owner != "dev-dependencies"
            {
                names.push(name.to_owned());
            }
            continue;
        }
        if in_dependency_table && let Some((name, _)) = line.split_once('=') {
            names.push(name.trim().to_owned());
        }
    }
    names
}

#[test]
fn the_protocol_depends_only_on_its_serialization_crates() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();

    let unexpected: Vec<String> = dependency_names(&manifest)
        .into_iter()
        .filter(|name| !ALLOWED_DEPENDENCIES.contains(&name.as_str()))
        .collect();

    assert_eq!(
        unexpected,
        Vec::<String>::new(),
        "{}",
        manifest_path.display()
    );
}

#[test]
fn a_new_dependency_in_any_runtime_table_is_found() {
    let manifest = "[dependencies]\nserde = \"1\"\n\n[target.'cfg(unix)'.dependencies]\nturso = \"0.1\"\n\n[build-dependencies]\ncockpit-turso = { path = \"../cockpit-turso\" }\n";

    assert_eq!(
        dependency_names(manifest),
        ["serde", "turso", "cockpit-turso"]
    );
}

#[test]
fn a_dependency_declared_as_its_own_table_is_found() {
    let manifest = "[dependencies.turso]\nversion = \"0.1\"\n";

    assert_eq!(dependency_names(manifest), ["turso"]);
}

#[test]
fn dev_dependencies_are_not_runtime_edges() {
    let manifest =
        "[dev-dependencies]\ntempfile = \"3\"\n\n[dev-dependencies.insta]\nversion = \"1\"\n";

    assert_eq!(dependency_names(manifest), Vec::<String>::new());
}
