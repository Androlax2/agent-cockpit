//! Decision records are one file each under docs/decisions/ (see its README), so parallel
//! pull requests never edit the same file; docs/DECISIONS.md is frozen history.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate lives in crates/<name>/")
        .to_path_buf()
}

/// `YYYY-MM-DD-<issue>-<slug>.md`: date, issue number, lower-case slug.
fn is_decision_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".md") else {
        return false;
    };
    let mut parts = stem.splitn(5, '-');
    let date_ok = [4, 2, 2].iter().all(|&width| {
        parts
            .next()
            .is_some_and(|part| part.len() == width && part.bytes().all(|b| b.is_ascii_digit()))
    });
    let issue_ok = parts
        .next()
        .is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    let slug_ok = parts.next().is_some_and(|slug| {
        !slug.is_empty()
            && !slug.starts_with('-')
            && !slug.ends_with('-')
            && slug
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    });
    date_ok && issue_ok && slug_ok
}

#[test]
fn every_decision_record_is_named_date_issue_slug() {
    let offenders: Vec<String> = fs::read_dir(repo_root().join("docs/decisions"))
        .expect("docs/decisions/ exists")
        .map(|entry| {
            entry
                .expect("entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name != "README.md" && !is_decision_file_name(name))
        .collect();
    assert!(
        offenders.is_empty(),
        "docs/decisions/ files must be named YYYY-MM-DD-<issue>-<slug>.md, not {offenders:?}"
    );
}

#[test]
fn the_frozen_decisions_file_gains_no_entries() {
    let frozen =
        fs::read_to_string(repo_root().join("docs/DECISIONS.md")).expect("DECISIONS.md exists");
    let entries = frozen
        .lines()
        .filter(|line| line.starts_with("## "))
        .count();
    assert_eq!(
        entries, 18,
        "docs/DECISIONS.md is frozen at 18 entries; add a file under docs/decisions/ instead"
    );
}

#[test]
fn the_name_rule_accepts_the_documented_example_and_rejects_the_old_style() {
    assert!(is_decision_file_name(
        "2026-10-31-29-process-containment.md"
    ));
    assert!(!is_decision_file_name("process-containment.md"));
    assert!(!is_decision_file_name("2026-10-31-process-containment.md"));
    assert!(!is_decision_file_name(
        "2026-10-31-29-Process-Containment.md"
    ));
    assert!(!is_decision_file_name("2026-10-31-29-.md"));
}
