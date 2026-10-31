//! Report 02 (amended by #77): frontends import `cockpit-client` and `cockpit-protocol`, plus
//! only the named capability crates whose verbs must work without the daemon.

use std::fs;
use std::path::Path;

const ALLOWED_PRODUCT_DEPENDENCIES: [&str; 3] =
    ["cockpit-client", "cockpit-protocol", "cockpit-config"];

/// Names under `[dependencies]` that are workspace crates (`cockpit-*`).
fn product_dependencies(manifest: &str) -> Vec<String> {
    let mut in_dependencies = false;
    let mut names = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_dependencies = line == "[dependencies]";
            continue;
        }
        let Some((name, _)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if in_dependencies && name.starts_with("cockpit-") {
            names.push(name.to_string());
        }
    }
    names
}

#[test]
fn the_cli_depends_only_on_allowed_product_crates() {
    let manifest =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
    let forbidden: Vec<String> = product_dependencies(&manifest)
        .into_iter()
        .filter(|name| !ALLOWED_PRODUCT_DEPENDENCIES.contains(&name.as_str()))
        .collect();
    assert_eq!(forbidden, Vec::<String>::new());
}

#[test]
fn a_capability_crate_outside_the_list_is_caught() {
    let manifest = "[package]\nname = \"cockpit-cli\"\n\n[dependencies]\ncockpit-config = { path = \"../cockpit-config\" }\ncockpit-turso = { path = \"../cockpit-turso\" }\n\n[dev-dependencies]\ncockpit-sessions = { path = \"x\" }\n";
    assert_eq!(
        product_dependencies(manifest),
        ["cockpit-config", "cockpit-turso"]
    );
}
