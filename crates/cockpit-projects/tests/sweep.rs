//! The sweep's states on a real database and real Git: only `deleting` skips the checks.
mod common;

use std::path::{Path, PathBuf};

use cockpit_projects::{
    Checkout, CheckoutState, Error, Git, Kept, checkouts, create_parallel_clone, sweep, sweep_clone,
};
use common::{Scratch, git, register};

#[tokio::test]
async fn a_clone_left_removing_by_an_interrupted_sweep_is_checked_again_and_kept() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    register(&database, &root).await;
    let clone = create_parallel_clone(&database, &Git::from_path(), "app", "feature")
        .await
        .unwrap();
    std::fs::write(clone.path.join("draft"), "work\n").unwrap();
    git(&clone.path, &["add", "draft"]);
    git(&clone.path, &["commit", "--quiet", "-m", "local work"]);
    let write = database.write().await.unwrap();
    write
        .execute(
            "UPDATE project_checkouts SET state = 'removing' WHERE id = ?1",
            [clone.id.0],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    let report = sweep(&database, &Git::from_path(), false).await.unwrap();

    assert_eq!(report.kept.len(), 1, "{report:?}");
    assert_eq!(report.kept[0].1, Kept::UnpushedCommits);
    assert!(clone.path.join("draft").exists());
    let read = database.read().await.unwrap();
    let state = checkouts(&read, None)
        .await
        .unwrap()
        .into_iter()
        .find(|checkout| checkout.id == clone.id)
        .unwrap()
        .state;
    assert_eq!(state, CheckoutState::Live);
}

/// A clone whose branch `feature` holds one commit adding `draft`, on no remote branch.
async fn clone_with_local_commit(
    scratch: &Scratch,
) -> (cockpit_turso::Database, PathBuf, Checkout) {
    let database = scratch.open().await;
    let root = scratch.repository("app");
    register(&database, &root).await;
    let clone = create_parallel_clone(&database, &Git::from_path(), "app", "feature")
        .await
        .unwrap();
    std::fs::write(clone.path.join("draft"), "work\n").unwrap();
    git(&clone.path, &["add", "draft"]);
    git(&clone.path, &["commit", "--quiet", "-m", "local work"]);
    (database, root, clone)
}

/// The pull request flow: `feature` pushed, squash-merged into `main` as a new commit, its
/// remote branch deleted, and a fetch with `--prune` in the clone.
fn squash_merge(root: &Path, clone: &Path) {
    git(clone, &["push", "--quiet", "origin", "feature"]);
    std::fs::write(root.join("draft"), "work\n").unwrap();
    git(root, &["add", "draft"]);
    git(root, &["commit", "--quiet", "-m", "local work (#1)"]);
    git(
        root,
        &["branch", "--quiet", "--delete", "--force", "feature"],
    );
    git(clone, &["fetch", "--quiet", "--prune", "origin"]);
}

#[tokio::test]
async fn a_squash_merged_branch_whose_remote_branch_was_deleted_is_removed() {
    let scratch = Scratch::new();
    let (database, root, clone) = clone_with_local_commit(&scratch).await;
    squash_merge(&root, &clone.path);

    let (swept, kept) = sweep_clone(&database, &Git::from_path(), clone.id)
        .await
        .unwrap();

    assert_eq!(kept, None);
    assert_eq!(swept.id, clone.id);
    assert!(!clone.path.exists());
}

#[tokio::test]
async fn a_squash_merged_branch_whose_file_the_default_branch_changed_since_is_removed() {
    let scratch = Scratch::new();
    let (database, root, clone) = clone_with_local_commit(&scratch).await;
    squash_merge(&root, &clone.path);
    std::fs::write(root.join("draft"), "work, then more\n").unwrap();
    git(
        &root,
        &["commit", "--quiet", "--all", "-m", "a later pull request"],
    );
    git(&clone.path, &["fetch", "--quiet", "origin"]);

    let (_, kept) = sweep_clone(&database, &Git::from_path(), clone.id)
        .await
        .unwrap();

    assert_eq!(kept, None);
    assert!(!clone.path.exists());
}

#[tokio::test]
async fn a_squash_merged_branch_with_a_later_commit_is_kept_as_unpushed() {
    let scratch = Scratch::new();
    let (database, root, clone) = clone_with_local_commit(&scratch).await;
    squash_merge(&root, &clone.path);
    std::fs::write(clone.path.join("follow-up"), "more\n").unwrap();
    git(&clone.path, &["add", "follow-up"]);
    git(&clone.path, &["commit", "--quiet", "-m", "after the merge"]);

    let (_, kept) = sweep_clone(&database, &Git::from_path(), clone.id)
        .await
        .unwrap();

    assert_eq!(kept, Some(Kept::UnpushedCommits));
    assert!(clone.path.join("follow-up").exists());
}

#[tokio::test]
async fn a_squash_merged_branch_that_then_undid_part_of_its_change_is_kept_as_unpushed() {
    let scratch = Scratch::new();
    let (database, root, clone) = clone_with_local_commit(&scratch).await;
    std::fs::write(clone.path.join("notes"), "more\n").unwrap();
    git(&clone.path, &["add", "notes"]);
    git(&clone.path, &["commit", "--quiet", "-m", "notes"]);
    std::fs::write(root.join("notes"), "more\n").unwrap();
    git(&root, &["add", "notes"]);
    squash_merge(&root, &clone.path);
    git(&clone.path, &["rm", "--quiet", "notes"]);
    git(
        &clone.path,
        &["commit", "--quiet", "-m", "drop the notes again"],
    );

    let (_, kept) = sweep_clone(&database, &Git::from_path(), clone.id)
        .await
        .unwrap();

    assert_eq!(kept, Some(Kept::UnpushedCommits));
    assert!(clone.path.join("draft").exists());
}

#[tokio::test]
async fn a_branch_whose_changes_the_default_branch_changed_again_is_kept_as_unpushed() {
    let scratch = Scratch::new();
    let (database, root, clone) = clone_with_local_commit(&scratch).await;
    std::fs::write(root.join("draft"), "other\n").unwrap();
    git(&root, &["add", "draft"]);
    git(&root, &["commit", "--quiet", "-m", "a different draft"]);
    git(&clone.path, &["fetch", "--quiet", "origin"]);

    let (_, kept) = sweep_clone(&database, &Git::from_path(), clone.id)
        .await
        .unwrap();

    assert_eq!(kept, Some(Kept::UnpushedCommits));
    assert!(clone.path.join("draft").exists());
}

#[tokio::test]
async fn sweeping_a_main_checkout_as_a_clone_is_refused() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    let (_, main) = register(&database, &root).await;

    let refused = sweep_clone(&database, &Git::from_path(), main.id).await;

    assert!(matches!(refused, Err(Error::Refused { .. })), "{refused:?}");
    assert!(root.join("README").exists());
}

#[tokio::test]
async fn an_empty_unpushed_commit_is_kept_as_unpushed() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    register(&database, &root).await;
    let clone = create_parallel_clone(&database, &Git::from_path(), "app", "feature")
        .await
        .unwrap();
    git(
        &clone.path,
        &["commit", "--quiet", "--allow-empty", "-m", "a note"],
    );

    let (_, kept) = sweep_clone(&database, &Git::from_path(), clone.id)
        .await
        .unwrap();

    assert_eq!(kept, Some(Kept::UnpushedCommits));
    assert!(clone.path.exists());
}
