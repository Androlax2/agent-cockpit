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

/// The `description` of a manifest's `[package]` table, when it is a single-line string
/// without escapes.
fn package_description(manifest: &str) -> Option<String> {
    let mut table = "";
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            table = line.split('#').next().unwrap().trim();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if table != "[package]" || key.trim() != "description" {
            continue;
        }
        let value = value.trim();
        let quote = value
            .chars()
            .next()
            .filter(|first| *first == '"' || *first == '\'')?;
        let (description, after) = value[1..].split_once(quote)?;
        if description.contains('\\') {
            return None;
        }
        let after = after.trim();
        return (after.is_empty() || after.starts_with('#')).then(|| description.to_string());
    }
    None
}

/// One sentence: ends with a period and has no sentence break inside.
fn is_one_sentence(text: &str) -> bool {
    let Some(body) = text.strip_suffix('.') else {
        return false;
    };
    !body.trim().is_empty() && !body.contains(". ") && !body.ends_with('.')
}

/// Markdown is hard-wrapped, so a sentence may span README lines.
fn words_of(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `name: problem` for each member whose ownership sentence is missing or does not match.
fn members_without_an_ownership_sentence() -> Vec<String> {
    member_dirs()
        .into_iter()
        .filter_map(|member_dir| {
            let name = member_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let manifest_path = member_dir.join("Cargo.toml");
            let manifest = fs::read_to_string(&manifest_path)
                .unwrap_or_else(|error| panic!("{}: {error}", manifest_path.display()));
            let Some(description) = package_description(&manifest) else {
                return Some(format!(
                    "{name}: no single-line `description` without escapes in [package]"
                ));
            };
            if !is_one_sentence(&description) {
                return Some(format!(
                    "{name}: description is not one sentence: {description:?}"
                ));
            }
            let Ok(readme) = fs::read_to_string(member_dir.join("README.md")) else {
                return Some(format!("{name}: no readable README.md"));
            };
            if !words_of(&readme).contains(&words_of(&description)) {
                return Some(format!("{name}: README.md does not state the description"));
            }
            None
        })
        .collect()
}

/// Report 02: every crate owns one named capability, stated the same way in both places.
#[test]
fn every_member_states_its_ownership_in_one_sentence_in_its_manifest_and_readme() {
    assert_eq!(
        members_without_an_ownership_sentence(),
        Vec::<String>::new()
    );
}

#[test]
fn a_description_is_read_from_the_package_table_only() {
    for (manifest, expected) in [
        (
            "[package]\ndescription = \"Owns tasks.\"\n",
            Some("Owns tasks."),
        ),
        (
            "[package]\ndescription=\"Owns tasks.\" # why\r\n",
            Some("Owns tasks."),
        ),
        (
            "[package]\nname = \"x\"\n[features]\ndescription = \"No.\"\n",
            None,
        ),
        (
            "[package] # metadata\ndescription = 'Owns tasks.'\n",
            Some("Owns tasks."),
        ),
        (
            "[package]\ndescription = \"Owns the \\\"may\\\" answer.\"\n",
            None,
        ),
        ("[package]\ndescription.workspace = true\n", None),
        (
            "[package]\ndescription = \"\"\"\nOwns tasks.\n\"\"\"\n",
            None,
        ),
    ] {
        assert_eq!(
            package_description(manifest).as_deref(),
            expected,
            "{manifest:?}"
        );
    }
}

#[test]
fn only_a_single_sentence_counts_as_ownership() {
    for (text, expected) in [
        ("Owns tasks, their scope and attempts.", true),
        ("Owns tasks", false),
        ("Owns tasks. Also runs them.", false),
        (".", false),
        ("Owns tasks..", false),
    ] {
        assert_eq!(is_one_sentence(text), expected, "{text:?}");
    }
}
