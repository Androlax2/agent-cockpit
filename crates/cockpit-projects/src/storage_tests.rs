//! Deterministic race and limit fixtures for the descriptor walker.
#[path = "../tests/common/mod.rs"]
mod common;

use super::*;
use common::{Scratch, register};
use std::fs as disk;
use std::os::unix::fs::{PermissionsExt, symlink};

async fn fixture() -> (Scratch, Database, StoredCheckout) {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    register(&database, &root).await;
    let clone = crate::create_parallel_clone(&database, &crate::Git::from_path(), "app", "first")
        .await
        .unwrap();
    let stored = crate::store::stored_checkout(&database.read().await.unwrap(), clone.id)
        .await
        .unwrap();
    disk::create_dir(clone.path.join("target")).unwrap();
    (scratch, database, stored)
}

#[tokio::test]
async fn an_entry_budget_preserves_partial_bytes_and_time_exhaustion_is_not_zero() {
    let (_scratch, _database, stored) = fixture().await;
    for n in 0..20 {
        disk::write(stored.checkout.path.join(format!("target/{n}")), b"abc").unwrap();
    }
    let size = measure(&stored, &mut Budget::new(8, SCAN_BUDGET), &mut || {});
    assert!(
        matches!(size, TargetSize::Incomplete { apparent_bytes, ref reason }
        if apparent_bytes > 0 && apparent_bytes < 60 && reason.contains("entry limit")),
        "{size:?}"
    );
    let size = measure(
        &stored,
        &mut Budget::new(MAX_ENTRIES, Duration::ZERO),
        &mut || {},
    );
    assert!(
        matches!(size, TargetSize::Unavailable { ref reason } if reason.contains("time budget")),
        "{size:?}"
    );
}

#[tokio::test]
async fn depth_limit_is_explicit() {
    let (_scratch, _database, stored) = fixture().await;
    let mut directory = stored.checkout.path.join("target");
    for _ in 0..=MAX_DEPTH {
        directory.push("d");
    }
    disk::create_dir_all(&directory).unwrap();
    disk::write(directory.join("beyond-limit"), b"abc").unwrap();
    let size = measure(
        &stored,
        &mut Budget::new(MAX_ENTRIES, SCAN_BUDGET),
        &mut || {},
    );
    assert!(
        matches!(size, TargetSize::Incomplete { apparent_bytes: 0, ref reason } if reason.contains("depth limit")),
        "{size:?}"
    );
}

#[tokio::test]
async fn replacing_a_child_after_stat_cannot_follow_a_link_outside_target() {
    let (scratch, _database, stored) = fixture().await;
    let child = stored.checkout.path.join("target/child");
    disk::create_dir(&child).unwrap();
    let outside = scratch.path().join("outside");
    disk::create_dir(&outside).unwrap();
    disk::write(outside.join("secret"), [0; 100]).unwrap();
    let mut calls = 0;
    let size = measure(
        &stored,
        &mut Budget::new(MAX_ENTRIES, SCAN_BUDGET),
        &mut || {
            calls += 1;
            if calls == 2 {
                // First hook is after pinning root; second is after stat of child.
                disk::remove_dir(&child).unwrap();
                symlink(&outside, &child).unwrap();
            }
        },
    );
    assert_eq!(calls, 2);
    assert!(
        matches!(
            size,
            TargetSize::Incomplete {
                apparent_bytes: 0,
                ..
            }
        ),
        "{size:?}"
    );
    assert_eq!(disk::read(outside.join("secret")).unwrap(), [0; 100]);
}

#[tokio::test]
async fn replacing_the_pinned_root_does_not_measure_its_replacement() {
    let (scratch, _database, stored) = fixture().await;
    disk::write(stored.checkout.path.join("target/original"), b"abc").unwrap();
    let moved = scratch.path().join("moved");
    let size = measure(
        &stored,
        &mut Budget::new(MAX_ENTRIES, SCAN_BUDGET),
        &mut || {
            disk::rename(&stored.checkout.path, &moved).unwrap();
            disk::create_dir_all(stored.checkout.path.join("target")).unwrap();
            disk::write(stored.checkout.path.join("target/replacement"), [0; 100]).unwrap();
        },
    );
    assert!(matches!(size, TargetSize::Unavailable { .. }), "{size:?}");
    assert_eq!(disk::read(moved.join("target/original")).unwrap(), b"abc");
}

#[tokio::test]
async fn an_unreadable_target_is_unavailable_not_missing() {
    let (_scratch, _database, stored) = fixture().await;
    let target = stored.checkout.path.join("target");
    disk::set_permissions(&target, disk::Permissions::from_mode(0o000)).unwrap();
    let can_read = disk::read_dir(&target).is_ok();
    let size = measure(
        &stored,
        &mut Budget::new(MAX_ENTRIES, SCAN_BUDGET),
        &mut || {},
    );
    disk::set_permissions(&target, disk::Permissions::from_mode(0o700)).unwrap();
    // A privileged runner may read mode000; its successful result is not evidence of denial.
    if can_read {
        assert_eq!(size, TargetSize::Complete { apparent_bytes: 0 });
    } else {
        assert!(matches!(size, TargetSize::Unavailable { .. }), "{size:?}");
    }
}

#[tokio::test]
async fn a_target_replaced_during_traversal_is_incomplete() {
    let (_scratch, _database, stored) = fixture().await;
    let target = stored.checkout.path.join("target");
    disk::create_dir(target.join("child")).unwrap();
    disk::write(target.join("child/original"), b"abc").unwrap();
    let mut calls = 0;
    let size = measure(
        &stored,
        &mut Budget::new(MAX_ENTRIES, SCAN_BUDGET),
        &mut || {
            calls += 1;
            if calls == 2 {
                disk::rename(&target, stored.checkout.path.join("saved-target")).unwrap();
                disk::create_dir(&target).unwrap();
                disk::write(target.join("replacement"), [0; 100]).unwrap();
            }
        },
    );
    assert_eq!(calls, 2);
    assert!(
        matches!(size, TargetSize::Incomplete { apparent_bytes: 3, ref reason } if reason.contains("target directory changed")),
        "{size:?}"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn clone_ancestors_only_need_search_access_not_directory_enumeration() {
    let (_scratch, _database, stored) = fixture().await;
    let ancestor = stored.checkout.path.parent().unwrap();
    disk::set_permissions(ancestor, disk::Permissions::from_mode(0o111)).unwrap();
    let size = measure(
        &stored,
        &mut Budget::new(MAX_ENTRIES, SCAN_BUDGET),
        &mut || {},
    );
    disk::set_permissions(ancestor, disk::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(size, TargetSize::Complete { apparent_bytes: 0 });
}

#[tokio::test]
async fn the_report_caps_registry_rows_and_does_not_modify_them() {
    let (_scratch, database, stored) = fixture().await;
    let write = database.write().await.unwrap();
    write.execute(
        "WITH RECURSIVE numbers(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM numbers WHERE n<129)
         INSERT INTO project_checkouts (project_id,kind,branch,path,incarnation,state,git_dir_marker,created_at)
         SELECT c.project_id,'parallel','fixture-'||n,c.path||'-'||n,1,'live',c.git_dir_marker,c.created_at
         FROM project_checkouts c, numbers WHERE c.id=?1", [stored.checkout.id.0]
    ).await.unwrap();
    write.commit().await.unwrap();
    let report = clone_storage(&database).await.unwrap();
    assert_eq!(report.clones.len(), MAX_CLONES);
    assert!(report.truncated);
    assert!(
        report.clones[1..]
            .iter()
            .all(|clone| matches!(clone.measurement, TargetSize::Unavailable { .. }))
    );
    let read = database.read().await.unwrap();
    assert_eq!(crate::checkouts(&read, None).await.unwrap().len(), 131);
}

/// One registered clone per `(name, file count)`; the scratch keeps the fixture alive.
async fn stored_clones_with_files(
    clones: &[(&str, usize)],
) -> (Scratch, Database, Vec<StoredCheckout>) {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    register(&database, &root).await;
    let mut stored = Vec::new();
    for (name, file_count) in clones {
        let clone = crate::create_parallel_clone(&database, &crate::Git::from_path(), "app", name)
            .await
            .unwrap();
        disk::create_dir(clone.path.join("target")).unwrap();
        for number in 0..*file_count {
            disk::write(clone.path.join(format!("target/{number}")), b"abc").unwrap();
        }
        stored.push(
            crate::store::stored_checkout(&database.read().await.unwrap(), clone.id)
                .await
                .unwrap(),
        );
    }
    (scratch, database, stored)
}

#[tokio::test]
async fn every_clone_over_the_entry_limit_reports_its_own_partial_size() {
    let (_scratch, _database, stored) =
        stored_clones_with_files(&[("first", 30), ("second", 30), ("third", 30)]).await;

    let clones = measure_all(stored, 10, SCAN_BUDGET, REPORT_BUDGET);

    assert_eq!(clones.len(), 3);
    for clone in &clones {
        assert!(
            matches!(&clone.measurement, TargetSize::Incomplete { apparent_bytes, reason }
            if *apparent_bytes > 0 && reason.contains("entry limit reached after")),
            "{:?}",
            clone.measurement
        );
    }
}

#[tokio::test]
async fn an_incomplete_count_names_how_many_entries_the_walk_visited() {
    let (_scratch, _database, stored) = fixture().await;
    for number in 0..20 {
        disk::write(
            stored.checkout.path.join(format!("target/{number}")),
            b"abc",
        )
        .unwrap();
    }

    let size = measure(&stored, &mut Budget::new(8, SCAN_BUDGET), &mut || {});

    assert!(
        matches!(&size, TargetSize::Incomplete { reason, .. }
        if reason == "clone directory-entry limit reached after 8 entries"),
        "{size:?}"
    );
}

#[tokio::test]
async fn a_target_ending_exactly_at_the_entry_limit_is_complete() {
    let (_scratch, _database, stored) = fixture().await;
    for number in 0..3 {
        disk::write(
            stored.checkout.path.join(format!("target/{number}")),
            b"abc",
        )
        .unwrap();
    }

    // Three files plus the `.` and `..` entries.
    let size = measure(&stored, &mut Budget::new(5, SCAN_BUDGET), &mut || {});

    assert_eq!(size, TargetSize::Complete { apparent_bytes: 9 });
}

#[tokio::test]
async fn a_clone_under_the_limits_stays_complete_next_to_one_over_them() {
    let (_scratch, _database, stored) =
        stored_clones_with_files(&[("large", 30), ("small", 3)]).await;

    let clones = measure_all(stored, 10, SCAN_BUDGET, REPORT_BUDGET);

    assert!(matches!(
        clones[0].measurement,
        TargetSize::Incomplete { .. }
    ));
    assert_eq!(
        clones[1].measurement,
        TargetSize::Complete { apparent_bytes: 9 }
    );
}

#[test]
fn unused_report_time_flows_to_the_clones_still_waiting() {
    let deadline = Instant::now() + Duration::from_secs(60);

    let first_of_four = fair_time_share(deadline, 4);
    let last_of_four = fair_time_share(deadline, 1);

    assert!(
        first_of_four <= Duration::from_secs(15),
        "{first_of_four:?}"
    );
    assert!(first_of_four > Duration::from_secs(14), "{first_of_four:?}");
    assert!(last_of_four > Duration::from_secs(59), "{last_of_four:?}");
    assert_eq!(
        fair_time_share(Instant::now() - Duration::from_secs(1), 3),
        Duration::ZERO
    );
}
