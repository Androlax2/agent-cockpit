//! `Git::changed_files`: what work in a checkout changed and no remote has yet, read with
//! the host Git safety rules (#156: the approval names changed build files a command runs).

mod common;

use cockpit_projects::Git;
use common::{Scratch, git};

#[test]
fn changed_files_are_the_uncommitted_ones_and_those_of_unpushed_commits() {
    let scratch = Scratch::new();
    let upstream = scratch.repository("upstream");
    let work = scratch.path().join("work");
    git(
        &scratch.path(),
        &["clone", "--quiet", upstream.to_str().unwrap(), "work"],
    );
    std::fs::write(work.join("Makefile"), "all:\n").unwrap();
    git(&work, &["add", "Makefile"]);
    git(&work, &["commit", "--quiet", "-m", "local"]);
    std::fs::write(work.join("package.json"), "{}\n").unwrap();
    git(&work, &["add", "package.json"]);
    std::fs::create_dir_all(work.join("crates/x")).unwrap();
    std::fs::write(work.join("crates/x/build.rs"), "fn main() {}\n").unwrap();
    std::fs::write(work.join("README"), "edited\n").unwrap();

    let changed = Git::from_path().changed_files(&work).unwrap();

    assert_eq!(
        changed,
        ["Makefile", "README", "crates/x/build.rs", "package.json"]
    );
}

#[test]
fn a_checkout_without_changes_has_none() {
    let scratch = Scratch::new();
    let upstream = scratch.repository("upstream");
    let work = scratch.path().join("work");
    git(
        &scratch.path(),
        &["clone", "--quiet", upstream.to_str().unwrap(), "work"],
    );

    assert!(Git::from_path().changed_files(&work).unwrap().is_empty());
}

#[test]
fn a_repository_with_no_commit_yet_lists_its_new_files() {
    let scratch = Scratch::new();
    let root = scratch.path().join("fresh");
    std::fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--quiet"]);
    std::fs::write(root.join("build.rs"), "").unwrap();

    assert_eq!(Git::from_path().changed_files(&root).unwrap(), ["build.rs"]);
}
