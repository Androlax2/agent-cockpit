use std::fs;
use std::path::{Path, PathBuf};

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("this crate lives in crates/")
        .to_path_buf()
}

/// The `members = ["crates/*"]` glob: every non-hidden directory under crates/.
fn member_dirs() -> Vec<PathBuf> {
    let mut member_dirs: Vec<PathBuf> = fs::read_dir(crates_dir())
        .expect("crates/ is readable")
        .map(|entry| entry.expect("crates/ entry is readable").path())
        .filter(|path| path.is_dir())
        .filter(|path| !path.file_name().unwrap().to_string_lossy().starts_with('.'))
        .collect();
    member_dirs.sort();
    member_dirs
}

/// Each `key = value` line of a manifest as `table.key=value`, without comments or spaces.
///
/// Not a TOML parser: enough for the flat inheritance keys these rules check, so the
/// workspace needs no parsing dependency. A `#` inside a quoted value ends the line early,
/// which only affects values these rules never read.
fn flattened_entries(manifest: &str) -> Vec<String> {
    let mut table = String::new();
    let mut entries = Vec::new();
    for line in manifest.lines() {
        let line: String = line
            .split('#')
            .next()
            .unwrap()
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            table = header.to_string();
            continue;
        }
        entries.push(if table.is_empty() {
            line
        } else {
            format!("{table}.{line}")
        });
    }
    entries
}

/// Whether `key` (a dotted path such as `package.edition`) is set to `{ workspace = true }`.
fn inherits_from_workspace(manifest: &str, key: &str) -> bool {
    let dotted = format!("{key}.workspace=true");
    let inline = format!("{key}={{workspace=true}}");
    flattened_entries(manifest)
        .iter()
        .any(|entry| *entry == dotted || *entry == inline)
}

/// Names of the members whose `Cargo.toml` does not inherit `key` from the workspace.
fn members_not_inheriting(key: &str) -> Vec<String> {
    member_dirs()
        .into_iter()
        .filter(|member_dir| {
            let manifest_path = member_dir.join("Cargo.toml");
            let manifest = fs::read_to_string(&manifest_path)
                .unwrap_or_else(|error| panic!("{}: {error}", manifest_path.display()));
            !inherits_from_workspace(&manifest, key)
        })
        .map(|member_dir| {
            member_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn every_member_inherits_the_workspace_lints() {
    assert_eq!(members_not_inheriting("lints"), Vec::<String>::new());
}

#[test]
fn every_member_inherits_the_workspace_edition() {
    assert_eq!(
        members_not_inheriting("package.edition"),
        Vec::<String>::new()
    );
}

#[test]
fn every_member_inherits_the_workspace_rust_version() {
    assert_eq!(
        members_not_inheriting("package.rust-version"),
        Vec::<String>::new()
    );
}

/// `.cargo/deny.toml` skips the license check only for unpublished workspace crates.
#[test]
fn every_member_inherits_the_workspace_publish_setting() {
    assert_eq!(
        members_not_inheriting("package.publish"),
        Vec::<String>::new()
    );
}

#[test]
fn a_commented_out_inheritance_line_does_not_count() {
    let manifest = "[package]\nedition = \"2021\"\n# edition.workspace = true\n";
    assert!(!inherits_from_workspace(manifest, "package.edition"));
}

#[test]
fn every_valid_spelling_of_lint_inheritance_counts() {
    for manifest in [
        "[lints]\nworkspace = true\n",
        "[lints]\r\nworkspace=true # shared policy\r\n",
        "lints.workspace = true\n",
        "lints = { workspace = true }\n",
    ] {
        assert!(inherits_from_workspace(manifest, "lints"), "{manifest:?}");
    }
}
