//! `Git::current_branch` (#60): the branch a checkout has out, `None` only for a detached
//! HEAD, and an error, never a detached HEAD, when Git cannot read the folder.

mod common;

use cockpit_projects::Git;
use common::{Scratch, git};

#[test]
fn a_checkout_on_a_branch_names_it() {
    let scratch = Scratch::new();
    let root = scratch.repository("app");
    git(&root, &["switch", "--quiet", "--create", "feature/x"]);

    assert_eq!(
        Git::from_path().current_branch(&root).unwrap().as_deref(),
        Some("feature/x")
    );
}

#[test]
fn a_detached_head_has_no_branch() {
    let scratch = Scratch::new();
    let root = scratch.repository("app");
    git(&root, &["switch", "--quiet", "--detach"]);

    assert_eq!(Git::from_path().current_branch(&root).unwrap(), None);
}

#[test]
fn a_folder_git_cannot_read_is_an_error_not_a_detached_head() {
    let scratch = Scratch::new();
    let folder = scratch.path().join("not-a-repository");
    std::fs::create_dir(&folder).unwrap();

    assert!(Git::from_path().current_branch(&folder).is_err());
}
