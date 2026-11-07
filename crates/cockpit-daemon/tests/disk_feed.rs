//! #1143 at the daemon's socket: the low-disk guard puts the disk on the change feed that
//! `torlek-notify` follows, and a guard that is off puts nothing there. The floor is one no disk
//! meets or zero, so what the fixture's own filesystem has free decides nothing.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use cockpit_client::Subscription;
use cockpit_protocol::{Change, DiskView, EntityKind};
use common::{GENEROUS, TestHome};

/// A home whose owner config holds `storage_settings`.
fn home_with(storage_settings: &str) -> TestHome {
    let home = TestHome::new();
    let config = home.home().join("config").join(common::BUILD_PROFILE_DIR);
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.toml"), storage_settings).unwrap();
    home
}

/// The disk entities in the feed's first snapshot.
async fn disks_in_the_snapshot(home: &TestHome) -> Vec<DiskView> {
    let mut feed = Subscription::new(home.socket_path(), None, GENEROUS);
    let pending = tokio::time::timeout(GENEROUS, feed.next_batch())
        .await
        .expect("the feed sends a snapshot")
        .expect("the feed continues");
    pending
        .batch()
        .changes
        .iter()
        .filter(|event| event.entity.kind == EntityKind::Disk)
        .map(|event| {
            let Change::Upsert { state } = &event.change else {
                panic!("a snapshot holds no tombstone: {event:?}");
            };
            assert_eq!(event.entity.id, "state-dir");
            serde_json::from_value(state.clone().into()).unwrap()
        })
        .collect()
}

#[tokio::test]
async fn a_low_disk_is_on_the_change_feed_from_the_first_snapshot() {
    let home = home_with("[storage]\nlow_disk_floor_mib = 1000000000000\n");
    let _daemon = home.start();

    let disks = disks_in_the_snapshot(&home).await;

    assert_eq!(disks.len(), 1, "{disks:?}");
    assert_eq!(disks[0].floor_mib, 1_000_000_000_000);
}

#[tokio::test]
async fn a_guard_that_is_off_puts_no_disk_on_the_change_feed() {
    let home = home_with("[storage]\nlow_disk_floor_mib = 0\n");
    let _daemon = home.start();

    let disks = disks_in_the_snapshot(&home).await;

    assert_eq!(disks, []);
}
