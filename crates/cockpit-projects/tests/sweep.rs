//! The sweep's states on a real database and real Git: only `deleting` skips the checks.
mod common;

use cockpit_projects::{CheckoutState, Git, Kept, checkouts, create_parallel_clone, sweep};
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
