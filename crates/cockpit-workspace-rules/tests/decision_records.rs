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

/// The token after "superseded by" on a line, without surrounding backticks or punctuation;
/// empty when nothing follows. `None` when the line does not say "superseded by".
fn successor_named_on(line: &str) -> Option<&str> {
    const MARKER: &str = "superseded by";
    let marker_start = line.to_ascii_lowercase().find(MARKER)?;
    let after_marker = &line[marker_start + MARKER.len()..];
    let first_word = after_marker.split_whitespace().next().unwrap_or("");
    Some(first_word.trim_matches(|c| "`:.,;()".contains(c)))
}

/// Why a `Status:` line is not one of the forms in docs/decisions/README.md, if it is not.
fn status_line_problem(line: &str) -> Option<&'static str> {
    let status = line.strip_prefix("Status: ")?.trim();
    if status == "accepted" {
        return None;
    }
    if let Some(file) = status.strip_prefix("superseded by ") {
        let names_only_a_file = is_decision_file_name(file.trim_matches('`'));
        return (!names_only_a_file).then_some("'superseded by' takes only a file name");
    }
    if let Some(successor_and_remainder) = status.strip_prefix("partly superseded by ") {
        let Some((file, still_holds)) = successor_and_remainder.split_once(": ") else {
            return Some("'partly superseded by <file>' needs ': <what still holds>'");
        };
        let names_a_file = is_decision_file_name(file.trim_matches('`'));
        let says_what_holds = !still_holds.trim().is_empty();
        return (!(names_a_file && says_what_holds))
            .then_some("'partly superseded by' needs a file name and what still holds");
    }
    Some(
        "expected 'accepted', 'superseded by <file>' or 'partly superseded by <file>: <what still holds>'",
    )
}

fn is_existing_record(decisions_directory: &Path, file_name: &str) -> bool {
    is_decision_file_name(file_name) && decisions_directory.join(file_name).is_file()
}

/// (file name, contents) of every decision record, README excluded.
fn decision_records() -> Vec<(String, String)> {
    let directory = repo_root().join("docs/decisions");
    fs::read_dir(&directory)
        .expect("docs/decisions/ exists")
        .map(|entry| entry.expect("entry is readable").path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "md")
                && path.file_name().is_some_and(|name| name != "README.md")
        })
        .map(|path| {
            let name = path
                .file_name()
                .expect("a file has a name")
                .to_string_lossy()
                .into_owned();
            let contents = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
            (name, contents)
        })
        .collect()
}

#[test]
fn every_superseded_by_line_names_an_existing_decision_record() {
    let decisions_directory = repo_root().join("docs/decisions");
    let mut offenders = Vec::new();
    for (record, contents) in decision_records() {
        for line in contents.lines() {
            let Some(successor) = successor_named_on(line) else {
                continue;
            };
            if successor == record || !is_existing_record(&decisions_directory, successor) {
                offenders.push(format!("{record}: {line:?}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "'superseded by' must be followed by the file name of another record in docs/decisions/: {offenders:#?}"
    );
}

#[test]
fn every_status_line_is_accepted_or_names_its_successor() {
    let mut offenders = Vec::new();
    for (record, contents) in decision_records() {
        for line in contents.lines() {
            if let Some(problem) = status_line_problem(line) {
                offenders.push(format!("{record}: {line:?}: {problem}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Status lines must follow docs/decisions/README.md: {offenders:#?}"
    );
}

#[test]
fn the_superseded_by_rule_finds_the_file_on_every_documented_wording() {
    let successor = "2026-11-05-828-remove-sandbox-engines.md";
    let wordings = [
        format!("Status: superseded by {successor}"),
        format!("Status: superseded by `{successor}`"),
        format!("Status: partly superseded by {successor}: the file convention holds"),
        format!("Superseded by `{successor}`: the pins are removed."),
        format!("  Superseded by `{successor}`."),
    ];
    for wording in &wordings {
        assert_eq!(successor_named_on(wording), Some(successor), "{wording}");
    }
}

#[test]
fn a_successor_that_is_not_a_file_in_the_log_is_not_an_existing_record() {
    let decisions_directory = repo_root().join("docs/decisions");
    assert!(is_existing_record(
        &decisions_directory,
        "2026-10-31-33-provider-contract.md"
    ));
    assert!(!is_existing_record(
        &decisions_directory,
        "2099-01-01-1-no-such-record.md"
    ));
    assert!(!is_existing_record(&decisions_directory, "README.md"));
    assert!(!is_existing_record(&decisions_directory, "../DECISIONS.md"));
}

#[test]
fn the_superseded_by_rule_reports_a_missing_or_malformed_file_name() {
    assert_eq!(successor_named_on("Superseded by"), Some(""));
    assert_eq!(successor_named_on("Superseded by #828."), Some("#828"));
    assert_eq!(
        successor_named_on("Status: superseded by the sandbox rewrite"),
        Some("the")
    );
    assert_eq!(
        successor_named_on("Supersedes the engine parts of 61"),
        None
    );
}

#[test]
fn the_status_rule_accepts_the_documented_forms_and_rejects_the_rest() {
    let successor = "2026-11-05-828-remove-sandbox-engines.md";
    let accepted = [
        "Status: accepted".to_string(),
        format!("Status: superseded by {successor}"),
        format!("Status: superseded by `{successor}`"),
        format!("Status: partly superseded by {successor}: refuse, never weaken"),
        "A line that is not a status line".to_string(),
    ];
    for line in &accepted {
        assert_eq!(status_line_problem(line), None, "{line}");
    }
    let rejected = [
        "Status: Accepted".to_string(),
        "Status: superseded".to_string(),
        "Status: superseded by #828".to_string(),
        format!("Status: superseded by {successor}: the pins are removed"),
        format!("Status: partly superseded by {successor}"),
        format!("Status: partly superseded by {successor}: "),
    ];
    for line in &rejected {
        assert!(status_line_problem(line).is_some(), "{line}");
    }
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
