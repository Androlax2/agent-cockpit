//! Registered identities and filesystem boundaries in isolated Git/database fixtures.
mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::net::UnixListener;

use cockpit_projects::{Git, TargetSize, claim, clone_storage, create_parallel_clone};
use common::{Scratch, holder, register};

#[tokio::test]
async fn report_counts_apparent_bytes_and_preserves_occupied_and_missing_clones() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    register(&database, &root).await;
    let first = create_parallel_clone(&database, &Git::from_path(), "app", "first")
        .await
        .unwrap();
    let second = create_parallel_clone(&database, &Git::from_path(), "app", "second")
        .await
        .unwrap();
    let write = database.write().await.unwrap();
    claim(&write, first.id, &holder("live-session"))
        .await
        .unwrap();
    write.commit().await.unwrap();
    fs::create_dir_all(first.path.join("target/debug")).unwrap();
    fs::write(first.path.join("target/a"), b"abc").unwrap();
    fs::hard_link(
        first.path.join("target/a"),
        first.path.join("target/debug/link"),
    )
    .unwrap();
    fs::write(first.path.join("target/debug/b"), b"12345").unwrap();
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join("target/excluded-main"), [0; 100]).unwrap();

    let report = clone_storage(&database).await.unwrap();

    assert!(!report.truncated);
    assert_eq!(report.clones.len(), 2);
    assert_eq!(report.clones[0].checkout.id, first.id);
    assert_eq!(report.clones[0].checkout.incarnation, 1);
    assert_eq!(
        report.clones[0].measurement,
        TargetSize::Complete { apparent_bytes: 11 }
    );
    assert_eq!(
        report.clones[0]
            .checkout
            .occupancy
            .as_ref()
            .unwrap()
            .holder
            .as_str(),
        "live-session"
    );
    assert_eq!(report.clones[1].checkout.id, second.id);
    assert_eq!(report.clones[1].measurement, TargetSize::Missing);
    assert_eq!(fs::read(first.path.join("target/a")).unwrap(), b"abc");
    assert!(!second.path.join("target").exists());
}

#[tokio::test]
async fn links_and_special_files_never_count_external_data_as_complete() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    register(&database, &root).await;
    let clone = create_parallel_clone(&database, &Git::from_path(), "app", "first")
        .await
        .unwrap();
    let outside = scratch.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret"), [0; 100]).unwrap();
    fs::create_dir(clone.path.join("target")).unwrap();
    fs::write(clone.path.join("target/a"), b"abc").unwrap();
    symlink(&outside, clone.path.join("target/linked")).unwrap();
    // Bind below the socket path limit even with macOS's long canonical temporary root.
    let socket_path = scratch.path().join("socket");
    let _socket = UnixListener::bind(&socket_path).unwrap();
    fs::rename(socket_path, clone.path.join("target/socket")).unwrap();

    let report = clone_storage(&database).await.unwrap();
    assert!(
        matches!(&report.clones[0].measurement, TargetSize::Incomplete { apparent_bytes: 3, reason } if !reason.is_empty()),
        "{report:?}"
    );

    fs::rename(clone.path.join("target"), clone.path.join("saved-target")).unwrap();
    symlink(&outside, clone.path.join("target")).unwrap();
    let report = clone_storage(&database).await.unwrap();
    assert!(matches!(
        report.clones[0].measurement,
        TargetSize::Unavailable { .. }
    ));
    assert_eq!(fs::read(outside.join("secret")).unwrap(), [0; 100]);
}

#[tokio::test]
async fn replaced_clone_and_symlinked_ancestor_are_unavailable_even_with_an_old_marker() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    register(&database, &root).await;
    let clone = create_parallel_clone(&database, &Git::from_path(), "app", "first")
        .await
        .unwrap();
    fs::create_dir(clone.path.join("target")).unwrap();
    fs::write(clone.path.join("target/data"), b"abc").unwrap();
    let moved = scratch.path().join("moved-clone");
    fs::rename(&clone.path, &moved).unwrap();
    fs::create_dir_all(clone.path.join(".git")).unwrap();
    fs::create_dir(clone.path.join("target")).unwrap();
    let report = clone_storage(&database).await.unwrap();
    assert!(matches!(
        report.clones[0].measurement,
        TargetSize::Unavailable { .. }
    ));

    fs::remove_dir_all(&clone.path).unwrap();
    symlink(&moved, &clone.path).unwrap();
    let report = clone_storage(&database).await.unwrap();
    assert!(matches!(
        report.clones[0].measurement,
        TargetSize::Unavailable { .. }
    ));

    fs::remove_file(&clone.path).unwrap();
    fs::rename(&moved, &clone.path).unwrap();
    let parent = clone.path.parent().unwrap();
    let moved_parent = scratch.path().join("moved-parent");
    fs::rename(parent, &moved_parent).unwrap();
    symlink(&moved_parent, parent).unwrap();
    let report = clone_storage(&database).await.unwrap();
    assert!(matches!(
        report.clones[0].measurement,
        TargetSize::Unavailable { .. }
    ));
}
