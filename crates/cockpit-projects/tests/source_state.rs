//! `Git::source_state`: the head a checkout is on and a digest of its uncommitted changes,
//! read with the host Git safety rules (#313: a verification receipt names the exact source).

mod common;

use std::path::PathBuf;

use cockpit_projects::{Git, SourceState};
use common::{Scratch, git};

fn work(scratch: &Scratch) -> PathBuf {
    let upstream = scratch.repository("upstream");
    git(
        &scratch.path(),
        &["clone", "--quiet", upstream.to_str().unwrap(), "work"],
    );
    scratch.path().join("work")
}

fn state(work: &std::path::Path) -> SourceState {
    Git::from_path().source_state(work).unwrap()
}

#[test]
fn a_clean_checkout_is_its_head_with_no_uncommitted_changes() {
    let scratch = Scratch::new();
    let work = work(&scratch);

    let state = state(&work);

    assert_eq!(state.head, git(&work, &["rev-parse", "HEAD"]).trim());
    assert_eq!(state.dirty, None);
}

#[test]
fn every_kind_of_uncommitted_change_has_its_own_digest() {
    let scratch = Scratch::new();
    let work = work(&scratch);
    let mut digests = Vec::new();

    std::fs::write(work.join("new.rs"), "one\n").unwrap();
    digests.push(state(&work).dirty);
    std::fs::write(work.join("new.rs"), "two\n").unwrap();
    digests.push(state(&work).dirty);
    git(&work, &["add", "new.rs"]);
    std::fs::write(work.join("new.rs"), "three\n").unwrap();
    digests.push(state(&work).dirty);

    assert!(digests.iter().all(Option::is_some), "{digests:?}");
    digests.dedup();
    assert_eq!(digests.len(), 3, "{digests:?}");
}

#[test]
fn the_same_changes_have_the_same_digest_and_leave_the_index_as_it_was() {
    let scratch = Scratch::new();
    let work = work(&scratch);
    std::fs::write(work.join("new.rs"), "one\n").unwrap();
    let status = git(&work, &["status", "--porcelain"]);

    let first = state(&work);
    let second = state(&work);

    assert_eq!(first, second);
    assert_eq!(git(&work, &["status", "--porcelain"]), status);
}

#[test]
fn ignored_files_are_not_uncommitted_changes() {
    let scratch = Scratch::new();
    let work = work(&scratch);
    std::fs::write(work.join(".gitignore"), "target/\n").unwrap();
    git(&work, &["add", ".gitignore"]);
    git(&work, &["commit", "--quiet", "-m", "ignore"]);
    std::fs::create_dir(work.join("target")).unwrap();
    std::fs::write(work.join("target/out"), "built\n").unwrap();

    assert_eq!(state(&work).dirty, None);
}

#[test]
fn a_repository_with_no_commit_is_refused() {
    let scratch = Scratch::new();
    let root = scratch.path().join("fresh");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--quiet"]);

    assert!(Git::from_path().source_state(&root).is_err());
}
