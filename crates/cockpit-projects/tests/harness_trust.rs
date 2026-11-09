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

    assert_eq!(refused, "its origin is not its project's");
}

#[test]
fn a_clone_without_an_origin_is_refused() {
    let fixture = Fixture::new();
    git(&fixture.clone, &["remote", "remove", "origin"]);

    let refused = fixture.source(parallel(), &[&fixture.main]).unwrap_err();

    assert!(refused.contains("has no origin"), "{refused}");
}

/// Issue #1279: the main checkout's fetched default branch, read from its own Git.
mod default_branch {
    use super::*;

    /// A main checkout whose `origin/HEAD` names a commit adding `.mcp.json` and a
    /// `.claude/` folder that its working tree, still on the first commit, lacks; its `main`
    /// follows `origin/main`.
    fn behind() -> (Scratch, PathBuf, String) {
        let scratch = Scratch::new();
        let main = scratch.repository("app");
        std::fs::create_dir(main.join(".claude")).unwrap();
        std::fs::write(main.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(main.join(".mcp.json"), "{\"mcpServers\": {}}").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", main.join("link")).unwrap();
        git(&main, &["add", "."]);
        git(&main, &["commit", "--quiet", "-m", "settings"]);
        let merged = git(&main, &["rev-parse", "HEAD"]).trim().to_owned();
        git(&main, &["update-ref", "refs/remotes/origin/main", &merged]);
        git(
            &main,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        git(&main, &["remote", "add", "origin", "../origin.git"]);
        git(
            &main,
            &["branch", "--quiet", "--set-upstream-to=origin/main"],
        );
        git(&main, &["reset", "--quiet", "--hard", "HEAD~1"]);
        (scratch, main, merged)
    }

    #[test]
    fn the_default_branch_commit_is_the_one_origin_head_names() {
        let (_scratch, main, merged) = behind();

        assert_eq!(
            Git::from_path().default_branch_commit(&main).unwrap(),
            Some(merged)
        );
    }

    /// A pull there would not bring the default branch in, so there is none to name.
    #[test]
    fn a_checkout_on_a_branch_not_following_the_default_branch_has_none() {
        let (_scratch, main, _merged) = behind();
        git(&main, &["switch", "--quiet", "-c", "feature"]);

        assert_eq!(Git::from_path().default_branch_commit(&main).unwrap(), None);
    }

    #[test]
    fn a_checkout_on_a_detached_head_has_none() {
        let (_scratch, main, _merged) = behind();
        git(&main, &["switch", "--quiet", "--detach"]);

        assert_eq!(Git::from_path().default_branch_commit(&main).unwrap(), None);
    }

    #[test]
    fn a_checkout_that_fetched_no_default_branch_has_none() {
        let scratch = Scratch::new();
        let main = scratch.repository("app");

        assert_eq!(Git::from_path().default_branch_commit(&main).unwrap(), None);
    }

    #[test]
    fn a_file_the_default_branch_added_is_read_from_the_commit() {
        let (_scratch, main, merged) = behind();

        let read = Git::from_path()
            .file_at_commit(&main, &merged, Path::new(".mcp.json"))
            .unwrap();

        assert_eq!(read, Some(b"{\"mcpServers\": {}}".to_vec()));
        assert!(!main.join(".mcp.json").exists());
    }

    #[test]
    fn a_folder_a_link_or_a_missing_path_is_no_file() {
        let (_scratch, main, merged) = behind();
        let git = Git::from_path();

        for path in [".claude", "link", "missing.json", ".claude/*"] {
            assert_eq!(
                git.file_at_commit(&main, &merged, Path::new(path)).unwrap(),
                None,
                "{path}"
            );
        }
    }
}
