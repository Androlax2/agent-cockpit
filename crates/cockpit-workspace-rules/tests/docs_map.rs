//! docs/README.md names every root Markdown file, every file directly in docs/ and every
//! directory under docs/, so a new page cannot appear without an owner and a reader.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate lives in crates/<name>/")
        .to_path_buf()
}

fn entries_of(directory: &Path) -> Vec<(String, bool)> {
    fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", directory.display()))
        .map(|entry| {
            let entry = entry.expect("entry is readable");
            let is_directory = entry.file_type().expect("file type is readable").is_dir();
            (
                entry.file_name().to_string_lossy().into_owned(),
                is_directory,
            )
        })
        .collect()
}

/// Table rows a page must start with, for example `| `README.md` |`, `| `docs/design/` |`.
fn expected_code_spans(root: &Path) -> Vec<String> {
    let root_files = entries_of(root)
        .into_iter()
        .filter(|(name, is_directory)| !is_directory && name.ends_with(".md"))
        .map(|(name, _)| format!("| `{name}` |"));
    let docs_entries = entries_of(&root.join("docs"))
        .into_iter()
        .filter(|(name, is_directory)| {
            *is_directory || (name.ends_with(".md") && name != "README.md")
        })
        .map(|(name, is_directory)| {
            let suffix = if is_directory { "/" } else { "" };
            format!("| `docs/{name}{suffix}` |")
        });
    root_files.chain(docs_entries).collect()
}

fn missing_from(map: &str, expected: &[String]) -> Vec<String> {
    expected
        .iter()
        .filter(|code_span| !map.contains(code_span.as_str()))
        .cloned()
        .collect()
}

#[test]
fn every_root_page_and_docs_entry_is_named_in_the_docs_map() {
    let root = repo_root();
    let map = fs::read_to_string(root.join("docs/README.md")).expect("docs/README.md exists");
    let missing = missing_from(&map, &expected_code_spans(&root));
    assert!(
        missing.is_empty(),
        "docs/README.md must name {missing:?}; add each to the table for its reader"
    );
}

#[test]
fn a_page_missing_from_the_map_is_reported() {
    let map = "| `README.md` | front door |\n| `docs/QUICKSTART.md` | first session |";
    let expected = vec![
        "| `README.md` |".to_owned(),
        "| `docs/NEW-PAGE.md` |".to_owned(),
        "| `docs/new-records/` |".to_owned(),
    ];
    assert_eq!(
        missing_from(map, &expected),
        ["| `docs/NEW-PAGE.md` |", "| `docs/new-records/` |"]
    );
}
