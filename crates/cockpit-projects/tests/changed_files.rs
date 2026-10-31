//! `Git::changed_files`: what work in a checkout changed outside the default branch, read with
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

fn cloned(scratch: &Scratch) -> std::path::PathBuf {
    let upstream = scratch.repository("upstream");
    git(
        &scratch.path(),
        &["clone", "--quiet", upstream.to_str().unwrap(), "work"],
    );
    scratch.path().join("work")
}

#[test]
fn a_commit_pushed_to_a_feature_branch_still_counts() {
    let scratch = Scratch::new();
    let work = cloned(&scratch);
    git(&work, &["switch", "--quiet", "-c", "feature"]);
    std::fs::write(work.join("Makefile"), "all:\n").unwrap();
    git(&work, &["add", "Makefile"]);
    git(&work, &["commit", "--quiet", "-m", "feature"]);
    git(&work, &["push", "--quiet", "origin", "feature"]);

    assert_eq!(Git::from_path().changed_files(&work).unwrap(), ["Makefile"]);
}

#[test]
fn an_ignored_file_counts() {
    let scratch = Scratch::new();
    let work = cloned(&scratch);
    std::fs::write(work.join(".git/info/exclude"), "deploy.sh\nbuild/\n").unwrap();
    std::fs::write(work.join("deploy.sh"), "echo hi\n").unwrap();
    std::fs::create_dir_all(work.join("build")).unwrap();
    std::fs::write(work.join("build/run.sh"), "").unwrap();

    assert_eq!(
        Git::from_path().changed_files(&work).unwrap(),
        ["build/", "deploy.sh"]
    );
}

#[test]
fn a_file_hidden_from_status_by_skip_worktree_or_assume_unchanged_counts() {
    let scratch = Scratch::new();
    let upstream = scratch.repository("upstream");
    std::fs::write(upstream.join("a.sh"), "").unwrap();
    std::fs::write(upstream.join("b.sh"), "").unwrap();
    git(&upstream, &["add", "a.sh", "b.sh"]);
    git(&upstream, &["commit", "--quiet", "-m", "scripts"]);
    git(
        &scratch.path(),
        &["clone", "--quiet", upstream.to_str().unwrap(), "work"],
    );
    let work = scratch.path().join("work");
    git(&work, &["update-index", "--skip-worktree", "a.sh"]);
    git(&work, &["update-index", "--assume-unchanged", "b.sh"]);
    std::fs::write(work.join("a.sh"), "rm -rf ~\n").unwrap();
    std::fs::write(work.join("b.sh"), "rm -rf ~\n").unwrap();

    assert_eq!(
        Git::from_path().changed_files(&work).unwrap(),
        ["a.sh", "b.sh"]
    );
}
