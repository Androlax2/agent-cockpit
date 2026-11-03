//! Issue #702: which checkouts may have a harness's own folder trust recorded before a
//! launch: a parallel clone of a project in `projects.trusted` with its project's origin.
mod common;

use std::path::{Path, PathBuf};

use cockpit_projects::{
    Checkout, CheckoutId, CheckoutKind, CheckoutState, Git, ProjectName, harness_trust_source,
};
use common::{Scratch, git};

const ORIGIN: &str = "git@example.com:owner/app.git";

struct Fixture {
    _scratch: Scratch,
    main: PathBuf,
    clone: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let scratch = Scratch::new();
        let main = scratch.repository("app");
        let clone = scratch.repository("app-clone");
        for root in [&main, &clone] {
            git(root, &["remote", "add", "origin", ORIGIN]);
        }
        Fixture {
            _scratch: scratch,
            main,
            clone,
        }
    }

    fn checkout(&self, kind: CheckoutKind) -> Checkout {
        let path = match kind {
            CheckoutKind::Main => self.main.clone(),
            _ => self.clone.clone(),
        };
        Checkout {
            id: CheckoutId(2),
            project: ProjectName::new("app").unwrap(),
            project_root: self.main.clone(),
            kind,
            path,
            incarnation: 1,
            state: CheckoutState::Live,
            occupancy: None,
        }
    }

    fn source(&self, kind: CheckoutKind, trusted_roots: &[&Path]) -> Result<PathBuf, String> {
        let trusted_roots: Vec<PathBuf> = trusted_roots
            .iter()
            .map(|root| root.to_path_buf())
            .collect();
        harness_trust_source(&self.checkout(kind), &trusted_roots, &Git::from_path())
    }
}

fn parallel() -> CheckoutKind {
    CheckoutKind::Parallel {
        branch: "feature".to_owned(),
    }
}

#[test]
fn a_parallel_clone_of_a_trusted_project_inherits_from_its_main_checkout() {
    let fixture = Fixture::new();

    assert_eq!(
        fixture.source(parallel(), &[&fixture.main]),
        Ok(fixture.main.clone())
    );
}

#[test]
fn a_parallel_clone_of_an_untrusted_project_is_refused() {
    let fixture = Fixture::new();

    let refused = fixture.source(parallel(), &[]).unwrap_err();

    assert!(refused.contains("not in projects.trusted"), "{refused}");
}

#[test]
fn a_review_clone_is_refused_even_in_a_trusted_project() {
    let fixture = Fixture::new();

    let refused = fixture
        .source(CheckoutKind::Review { pull_request: 7 }, &[&fixture.main])
        .unwrap_err();

    assert_eq!(refused, "a review clone is never trusted");
}

#[test]
fn a_main_checkout_is_refused() {
    let fixture = Fixture::new();

    let refused = fixture
        .source(CheckoutKind::Main, &[&fixture.main])
        .unwrap_err();

    assert_eq!(refused, "it is a main checkout, not a clone");
}

#[test]
fn a_clone_whose_origin_moved_is_refused() {
    let fixture = Fixture::new();
    git(
        &fixture.clone,
        &[
            "remote",
            "set-url",
            "origin",
            "git@example.com:other/app.git",
        ],
    );

    let refused = fixture.source(parallel(), &[&fixture.main]).unwrap_err();

    assert!(refused.contains("is not its project's"), "{refused}");
}

#[test]
fn a_clone_without_an_origin_is_refused() {
    let fixture = Fixture::new();
    git(&fixture.clone, &["remote", "remove", "origin"]);

    let refused = fixture.source(parallel(), &[&fixture.main]).unwrap_err();

    assert!(refused.contains("has no origin"), "{refused}");
}
