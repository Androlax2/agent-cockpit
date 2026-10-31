//! Report 02's forbidden edges around the state database, checked on the manifests in `crates/`.
use std::fs;
use std::path::{Path, PathBuf};

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("this crate lives in crates/")
        .to_path_buf()
}

/// Every workspace member as (directory name, manifest text), as the `crates/*` glob sees them.
fn member_manifests() -> Vec<(String, String)> {
    let mut members: Vec<(String, String)> = fs::read_dir(crates_dir())
        .expect("crates/ is readable")
        .map(|entry| entry.expect("crates/ entry is readable").path())
        .filter(|path| path.is_dir())
        .filter(|path| !path.file_name().unwrap().to_string_lossy().starts_with('.'))
        .map(|member_dir| {
            let manifest_path = member_dir.join("Cargo.toml");
            let manifest = fs::read_to_string(&manifest_path)
                .unwrap_or_else(|error| panic!("{}: {error}", manifest_path.display()));
            let name = member_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            (name, manifest)
        })
        .collect();
    members.sort();
    members
}

/// Names of every dependency a manifest declares, normal, dev, build or target-specific.
///
/// Not a TOML parser: it reads the `name = ...` keys of `*dependencies` tables and the
/// `[*dependencies.name]` headers, which is how this workspace spells dependencies.
fn dependency_names(manifest: &str) -> Vec<String> {
    let mut is_dependency_table = false;
    let mut names = Vec::new();
    for line in manifest.lines() {
        let line = line.split('#').next().unwrap().trim();
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let header = header.trim();
            is_dependency_table = header.ends_with("dependencies");
            if let Some((table, name)) = header.rsplit_once('.')
                && table.ends_with("dependencies")
            {
                names.push(name.trim().to_string());
            }
            continue;
        }
        if is_dependency_table && let Some((name, _)) = line.split_once('=') {
            names.push(name.trim().to_string());
        }
    }
    names
}

#[test]
fn cockpit_turso_depends_on_no_cockpit_crate() {
    let (_, manifest) = member_manifests()
        .into_iter()
        .find(|(name, _)| name == "cockpit-turso")
        .expect("cockpit-turso is a member");
    let cockpit_dependencies: Vec<String> = dependency_names(&manifest)
        .into_iter()
        .filter(|name| name.starts_with("cockpit-"))
        .collect();
    assert_eq!(cockpit_dependencies, Vec::<String>::new());
}

/// Only cockpit-turso opens the database, so every connection has change capture on and the
/// file keeps one opener.
#[test]
fn only_cockpit_turso_depends_on_turso() {
    let members_using_turso: Vec<String> = member_manifests()
        .into_iter()
        .filter(|(name, _)| name != "cockpit-turso")
        .filter(|(_, manifest)| {
            dependency_names(manifest)
                .iter()
                .any(|dependency| dependency.starts_with("turso"))
        })
        .map(|(name, _)| name)
        .collect();
    assert_eq!(members_using_turso, Vec::<String>::new());
}

#[test]
fn every_spelling_of_a_dependency_is_found() {
    let manifest = "[package]\nname = \"x\"\n\n[dependencies]\nturso = \"1\"\n\
                    cockpit-tasks = { path = \"../cockpit-tasks\" } # owner\n\n\
                    [dev-dependencies.cockpit-tmux]\npath = \"../cockpit-tmux\"\n\n\
                    [target.'cfg(unix)'.build-dependencies]\ncockpit-audit = \"1\"\n\n\
                    [lints]\nworkspace = true\n";
    assert_eq!(
        dependency_names(manifest),
        ["turso", "cockpit-tasks", "cockpit-tmux", "cockpit-audit"]
    );
}
