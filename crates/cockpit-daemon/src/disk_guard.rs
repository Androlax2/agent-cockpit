//! The low-disk guard (#406): reads the free space of the state directory's filesystem and
//! tells admission, so new launches and heavy jobs wait with a reason (`queue explain`, the
//! TUI queue, `doctor`). Low space also wakes the target-pruning pass at once. That pass
//! deletes only what the owner allowed in `[storage]`; this guard deletes nothing itself.
//!
//! The owner is told through the daemon log at warn level and through `doctor`, which stays
//! red while space is low. `torlek-notify` is told through the change feed (#1143): the guard
//! publishes one `disk` entity on entering low disk and retracts it on leaving, so staying low
//! publishes nothing and the notifier raises one notice per entry.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_protocol::DiskView;
use cockpit_resource_admission::{Backend, DiskSpace};
use cockpit_turso::Database;
use tokio::sync::Notify;

use crate::session_feed::state_of;

const DISK: &str = "disk";
/// The only `disk` entity: the filesystem of the state directory.
const STATE_DIR_DISK: &str = "state-dir";
const CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// While space stays low, the pruning pass runs again this often.
const SWEEP_INTERVAL_WHILE_LOW: Duration = Duration::from_secs(10 * 60);

/// What the guard remembers between checks.
#[derive(Default)]
pub(crate) struct Memory {
    pub(crate) last_sweep: Option<Instant>,
    /// Whether the `disk` entity is on the change feed; unknown until a write succeeded once,
    /// so a failed write is tried again at the next check.
    on_feed: OnFeed,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum OnFeed {
    #[default]
    Unknown,
    Absent,
    Present,
}

#[derive(Clone)]
pub(crate) struct DiskGuard {
    pub(crate) backend: Arc<Backend>,
    pub(crate) database: Arc<Database>,
    /// Reads the free space of the filesystem holding a path; a field so a test can script
    /// the readings instead of reading a disk.
    pub(crate) read_free_mib: fn(&std::path::Path) -> std::io::Result<u64>,
    pub(crate) watched_path: PathBuf,
    /// 0 turns the guard off.
    pub(crate) floor_mib: u64,
    pub(crate) queue_changed: Arc<Notify>,
    pub(crate) sweep_now: Arc<Notify>,
    /// Whether the owner enabled automatic target pruning; if not, nothing listens to `sweep_now`.
    pub(crate) prune_automatic: bool,
}

impl DiskGuard {
    /// Reads the disk once and records it. A disk that cannot be read is logged and leaves the
    /// previous reading, so a transient error neither holds work nor hides a low disk.
    pub(crate) async fn check(&self, memory: &mut Memory) {
        if self.floor_mib == 0 {
            self.publish(memory, None).await;
            return;
        }
        let path = self.watched_path.clone();
        let read_free_mib = self.read_free_mib;
        let read = tokio::task::spawn_blocking(move || read_free_mib(&path))
            .await
            .map_err(std::io::Error::other)
            .and_then(|result| result);
        let free_mib = match read {
            Ok(free_mib) => free_mib,
            Err(error) => {
                tracing::error!(path = %self.watched_path.display(), %error, "could not read the free disk space");
                return;
            }
        };
        let space = DiskSpace {
            free_mib,
            floor_mib: self.floor_mib,
        };
        let was_low = self.backend.disk_space().is_some_and(DiskSpace::is_low);
        self.backend.set_disk_space(Some(space));
        if space.is_low() != was_low {
            self.queue_changed.notify_one();
        }
        self.publish(memory, Some(space)).await;
        if !space.is_low() {
            memory.last_sweep = None;
            if was_low {
                tracing::info!(
                    free_mib,
                    "free disk space is back above the floor: waiting work resumes"
                );
            }
            return;
        }
        if !was_low {
            tracing::warn!(
                free_mib,
                floor_mib = self.floor_mib,
                path = %self.watched_path.display(),
                prune_automatic = self.prune_automatic,
                "free disk space is below the floor: new launches and heavy jobs wait; nothing is deleted unless the owner allowed it in [storage] (target_prune_automatic)"
            );
        }
        if self.prune_automatic
            && memory
                .last_sweep
                .is_none_or(|at| at.elapsed() >= SWEEP_INTERVAL_WHILE_LOW)
        {
            memory.last_sweep = Some(Instant::now());
            self.sweep_now.notify_one();
        }
    }

    /// Puts the `disk` entity on the change feed while `space` is low and takes it off
    /// otherwise, `None` being a guard that is off, so a floor of 0 after a low disk does not
    /// leave a low disk on the feed. A write that fails is logged and tried again at the next
    /// check.
    async fn publish(&self, memory: &mut Memory, space: Option<DiskSpace>) {
        let low_space = space.filter(|space| space.is_low());
        let wanted = if low_space.is_some() {
            OnFeed::Present
        } else {
            OnFeed::Absent
        };
        if memory.on_feed == wanted {
            return;
        }
        match self.write_feed(low_space).await {
            Ok(()) => memory.on_feed = wanted,
            Err(error) => {
                tracing::warn!(%error, "cannot publish the disk state on the change feed")
            }
        }
    }

    async fn write_feed(
        &self,
        low_space: Option<DiskSpace>,
    ) -> Result<(), cockpit_messaging::Error> {
        let write = self.database.write().await?;
        let has_changed = match low_space {
            Some(space) => {
                let view = DiskView {
                    free_mib: space.free_mib,
                    floor_mib: space.floor_mib,
                };
                let published =
                    cockpit_messaging::publish(&write, DISK, STATE_DIR_DISK, &state_of(&view))
                        .await?;
                matches!(published, cockpit_messaging::Published::Changed { .. })
            }
            None => cockpit_messaging::retract(&write, DISK, STATE_DIR_DISK)
                .await?
                .is_some(),
        };
        if has_changed {
            write.commit().await?;
        } else {
            write.rollback().await?;
        }
        Ok(())
    }
}

pub(crate) async fn guard_forever(guard: DiskGuard) {
    let mut ticks = tokio::time::interval(CHECK_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut memory = Memory::default();
    loop {
        ticks.tick().await;
        guard.check(&mut memory).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use cockpit_messaging::{EntityChange, Feed, FeedPosition};

    use super::*;

    async fn guard_reading(
        floor_mib: u64,
        read_free_mib: fn(&std::path::Path) -> std::io::Result<u64>,
    ) -> (DiskGuard, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let backend = Backend::detect(
            cockpit_resource_admission::HostTools::from_host(
                &cockpit_processes::SystemdTools::from_path(),
            ),
            cockpit_processes::LaunchContainment::ProcessGroup,
        )
        .await
        .unwrap();
        let database = Database::open(&directory.path().join("state.db"), &crate::MIGRATION_SETS)
            .await
            .unwrap();
        let guard = DiskGuard {
            backend: Arc::new(backend),
            database: Arc::new(database),
            read_free_mib,
            watched_path: directory.path().to_owned(),
            floor_mib,
            queue_changed: Arc::new(Notify::new()),
            sweep_now: Arc::new(Notify::new()),
            prune_automatic: true,
        };
        (guard, directory)
    }

    async fn guard(floor_mib: u64) -> (DiskGuard, tempfile::TempDir) {
        guard_reading(floor_mib, cockpit_projects::free_space_mib).await
    }

    /// The `disk` entities the feed carries after the last call, as their `free_mib`; `None`
    /// for a tombstone.
    async fn next_disk_states(feed: &mut Feed, guard: &DiskGuard) -> Vec<Option<u64>> {
        let Some(batch) = feed.next(&guard.database).await.unwrap() else {
            return Vec::new();
        };
        batch
            .changes
            .into_iter()
            .map(
                |EntityChange {
                     kind, id, state, ..
                 }| {
                    assert_eq!((kind.as_str(), id.as_str()), (DISK, STATE_DIR_DISK));
                    state.map(|state| {
                        let view: DiskView = serde_json::from_value(state.into()).unwrap();
                        view.free_mib
                    })
                },
            )
            .collect()
    }

    #[tokio::test]
    async fn a_floor_above_the_free_space_holds_work_and_wakes_the_sweep() {
        let (guard, _directory) = guard(u64::MAX / (1024 * 1024)).await;
        guard.check(&mut Memory::default()).await;
        assert!(guard.backend.policy().low_disk.is_some());
        tokio::time::timeout(Duration::from_secs(1), guard.sweep_now.notified())
            .await
            .expect("the low disk woke the pruning pass");
        tokio::time::timeout(Duration::from_secs(1), guard.queue_changed.notified())
            .await
            .expect("the low disk woke a scheduling pass");
    }

    #[tokio::test]
    async fn free_space_above_the_floor_holds_nothing_and_wakes_nothing() {
        let (guard, _directory) = guard(1).await;
        guard.check(&mut Memory::default()).await;
        assert!(guard.backend.policy().low_disk.is_none());
        assert!(guard.backend.disk_space().is_some());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), guard.sweep_now.notified())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_floor_of_zero_turns_the_guard_off() {
        let (guard, _directory) = guard(0).await;
        guard.check(&mut Memory::default()).await;
        assert!(guard.backend.disk_space().is_none());
    }

    static SCRIPTED_FREE_MIB: AtomicU64 = AtomicU64::new(0);

    fn scripted_free_mib(_path: &std::path::Path) -> std::io::Result<u64> {
        Ok(SCRIPTED_FREE_MIB.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn each_entry_into_low_disk_is_one_change_on_the_feed_and_leaving_it_retracts_it() {
        let (guard, _directory) = guard_reading(1000, scripted_free_mib).await;
        let (mut feed, snapshot) = Feed::open(&guard.database, FeedPosition::Snapshot)
            .await
            .unwrap();
        assert!(snapshot.unwrap().changes.is_empty());
        let mut memory = Memory::default();

        SCRIPTED_FREE_MIB.store(5000, Ordering::SeqCst);
        guard.check(&mut memory).await;
        assert_eq!(next_disk_states(&mut feed, &guard).await, []);

        SCRIPTED_FREE_MIB.store(900, Ordering::SeqCst);
        guard.check(&mut memory).await;
        assert_eq!(next_disk_states(&mut feed, &guard).await, [Some(900)]);

        SCRIPTED_FREE_MIB.store(400, Ordering::SeqCst);
        guard.check(&mut memory).await;
        guard.check(&mut memory).await;
        assert_eq!(next_disk_states(&mut feed, &guard).await, []);

        SCRIPTED_FREE_MIB.store(2000, Ordering::SeqCst);
        guard.check(&mut memory).await;
        assert_eq!(next_disk_states(&mut feed, &guard).await, [None]);

        SCRIPTED_FREE_MIB.store(100, Ordering::SeqCst);
        guard.check(&mut memory).await;
        assert_eq!(next_disk_states(&mut feed, &guard).await, [Some(100)]);
    }

    #[tokio::test]
    async fn a_guard_turned_off_takes_a_low_disk_off_the_feed() {
        let (low, directory) = guard(u64::MAX / (1024 * 1024)).await;
        low.check(&mut Memory::default()).await;
        let (mut feed, snapshot) = Feed::open(&low.database, FeedPosition::Snapshot)
            .await
            .unwrap();
        assert_eq!(snapshot.unwrap().changes.len(), 1);
        let off = DiskGuard {
            floor_mib: 0,
            watched_path: directory.path().to_owned(),
            ..low.clone()
        };

        off.check(&mut Memory::default()).await;

        assert_eq!(next_disk_states(&mut feed, &off).await, [None]);
    }
}
