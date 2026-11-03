use super::*;

async fn fixture() -> (tempfile::TempDir, StoredCheckout, Snapshot) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    for args in [
        vec!["init", "--quiet", "--initial-branch=main"],
        vec!["commit", "--quiet", "--allow-empty", "-m", "fixture"],
    ] {
        assert!(
            std::process::Command::new("git")
                .current_dir(&project)
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.com",
                    "-c",
                    "core.hooksPath=/dev/null"
                ])
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let db = cockpit_turso::Database::open(&temp.path().join("state.db"), &[crate::MIGRATIONS])
        .await
        .unwrap();
    let git = crate::Git::from_path();
    let (registered, _) = crate::register_project(&db, &git, &project, None)
        .await
        .unwrap();
    let clone = crate::create_parallel_clone(&db, &git, registered.name.as_str(), "build")
        .await
        .unwrap();
    std::fs::create_dir_all(clone.path.join("target/sub")).unwrap();
    std::fs::write(clone.path.join("target/a"), b"123").unwrap();
    std::fs::write(clone.path.join("target/sub/b"), b"12345").unwrap();
    std::fs::write(clone.path.join("source"), b"keep").unwrap();
    let stored = crate::store::stored_checkout(&db.read().await.unwrap(), clone.id)
        .await
        .unwrap();
    let snapshot = filesystem::observe(&stored, b"target").unwrap().unwrap();
    (temp, stored, snapshot)
}

const NAME: &str = ".torlek-target-prune-fixture-1";

#[tokio::test]
async fn quarantine_removes_exact_manifest_and_keeps_replacement_target() {
    let (_temp, stored, snapshot) = fixture().await;
    let proof = create(&stored, &snapshot, NAME).unwrap();
    assert!(stored.checkout.path.join("target/a").exists());
    detach(&stored, &snapshot, NAME, &proof).unwrap();
    std::fs::create_dir(stored.checkout.path.join("target")).unwrap();
    std::fs::write(stored.checkout.path.join("target/new"), b"keep").unwrap();
    let mut cursor = 0;
    let mut removed = 0;
    while cursor < snapshot.entries.len() {
        let slice = remove_slice(&stored, &snapshot, NAME, &proof, cursor);
        assert!(slice.reason.is_none(), "{:?}", slice.reason);
        assert!(slice.cursor > cursor);
        assert_eq!(slice.absent, 0);
        removed += slice.removed;
        cursor = slice.cursor;
    }
    assert_eq!(removed, 8);
    finish(&stored, &snapshot, NAME, &proof).unwrap();
    assert!(!stored.checkout.path.join(NAME).exists());
    assert_eq!(
        std::fs::read(stored.checkout.path.join("target/new")).unwrap(),
        b"keep"
    );
    assert_eq!(
        std::fs::read(stored.checkout.path.join("source")).unwrap(),
        b"keep"
    );
    assert!(stored.checkout.path.join(".git").is_dir());
}

#[tokio::test]
async fn quarantine_refuses_replaced_quarantine_and_symlink_without_outside_effects() {
    let (temp, stored, snapshot) = fixture().await;
    let proof = create(&stored, &snapshot, NAME).unwrap();
    detach(&stored, &snapshot, NAME, &proof).unwrap();
    let outside = temp.path().join("outside");
    std::fs::write(&outside, b"outside").unwrap();
    let entry = stored.checkout.path.join(NAME).join("target/sub/b");
    std::fs::remove_file(&entry).unwrap();
    std::os::unix::fs::symlink(&outside, &entry).unwrap();
    let slice = remove_slice(&stored, &snapshot, NAME, &proof, 0);
    assert!(slice.reason.is_some());
    assert_eq!((slice.cursor, slice.removed), (0, 0));
    assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
    std::fs::rename(
        stored.checkout.path.join(NAME),
        stored.checkout.path.join("saved"),
    )
    .unwrap();
    std::fs::create_dir(stored.checkout.path.join(NAME)).unwrap();
    assert!(
        remove_slice(&stored, &snapshot, NAME, &proof, 0)
            .reason
            .is_some()
    );
    assert!(stored.checkout.path.join("saved/target/a").is_file());
}

#[tokio::test]
async fn quarantine_existing_name_is_not_reused_and_detachment_receipt_is_recoverable() {
    let (_temp, stored, snapshot) = fixture().await;
    let proof = create(&stored, &snapshot, NAME).unwrap();
    assert!(create(&stored, &snapshot, NAME).is_err());
    detach(&stored, &snapshot, NAME, &proof).unwrap();
    // Re-open only persisted identities, as detachment-before-receipt recovery does.
    verify_detached(&stored, &snapshot, NAME, &proof).unwrap();
    assert_eq!(
        std::fs::read(stored.checkout.path.join(NAME).join("target/a")).unwrap(),
        b"123"
    );
    assert!(detach(&stored, &snapshot, NAME, &proof).is_err());
}

#[tokio::test]
async fn quarantine_unlink_before_receipt_reports_absence_and_finish_is_recoverable() {
    let (_temp, stored, snapshot) = fixture().await;
    let proof = create(&stored, &snapshot, NAME).unwrap();
    detach(&stored, &snapshot, NAME, &proof).unwrap();
    let first = remove_slice(&stored, &snapshot, NAME, &proof, 0);
    assert!(first.reason.is_none(), "{:?}", first.reason);
    assert_eq!(first.removed, 8);
    // Lose the receipt, not the effect. Recovery must never call these bytes confirmed.
    let recovered = remove_slice(&stored, &snapshot, NAME, &proof, 0);
    assert!(recovered.reason.is_none(), "{:?}", recovered.reason);
    assert_eq!(recovered.cursor, snapshot.entries.len());
    assert_eq!((recovered.removed, recovered.absent), (0, 8));
    // Model interruption after metadata unlink, before rmdir/checkpoint.
    std::fs::remove_file(stored.checkout.path.join(NAME).join("operation")).unwrap();
    finish(&stored, &snapshot, NAME, &proof).unwrap();
    // Model rmdir before its receipt. A new target remains outside this operation.
    std::fs::create_dir(stored.checkout.path.join("target")).unwrap();
    std::fs::write(stored.checkout.path.join("target/new"), b"keep").unwrap();
    finish(&stored, &snapshot, NAME, &proof).unwrap();
    assert_eq!(
        std::fs::read(stored.checkout.path.join("target/new")).unwrap(),
        b"keep"
    );
}
