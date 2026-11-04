//! The low-disk guard (#406): reads the free space of the state directory's filesystem and
//! tells admission, so new launches and heavy jobs wait with a reason (`queue explain`, the
//! TUI queue, `doctor`). Low space also wakes the target-pruning pass at once. That pass
//! deletes only what the owner allowed in `[storage]`; this guard deletes nothing itself.
//!
//! The owner is told through the daemon log at warn level and through `doctor`, which stays
//! red while space is low. No push notifier exists yet (#62, #63).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_resource_admission::{Backend, DiskSpace};
use tokio::sync::Notify;

const CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// While space stays low, the pruning pass runs again this often.
const SWEEP_INTERVAL_WHILE_LOW: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
pub(crate) struct DiskGuard {
    pub(crate) backend: Arc<Backend>,
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
    pub(crate) async fn check(&self, last_sweep: &mut Option<Instant>) {
        if self.floor_mib == 0 {
            return;
        }
        let path = self.watched_path.clone();
        let read = tokio::task::spawn_blocking(move || cockpit_projects::free_space_mib(&path))
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
        if !space.is_low() {
            *last_sweep = None;
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
            && last_sweep.is_none_or(|at| at.elapsed() >= SWEEP_INTERVAL_WHILE_LOW)
        {
            *last_sweep = Some(Instant::now());
            self.sweep_now.notify_one();
        }
    }
}

pub(crate) async fn guard_forever(guard: DiskGuard) {
    let mut ticks = tokio::time::interval(CHECK_INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_sweep = None;
    loop {
        ticks.tick().await;
        guard.check(&mut last_sweep).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn guard(floor_mib: u64) -> (DiskGuard, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let backend = Backend::detect(
            cockpit_resource_admission::HostTools::from_host(
                &cockpit_processes::SystemdTools::from_path(),
            ),
            cockpit_processes::LaunchContainment::ProcessGroup,
        )
        .await
        .unwrap();
        let guard = DiskGuard {
            backend: Arc::new(backend),
            watched_path: directory.path().to_owned(),
            floor_mib,
            queue_changed: Arc::new(Notify::new()),
            sweep_now: Arc::new(Notify::new()),
            prune_automatic: true,
        };
        (guard, directory)
    }

    #[tokio::test]
    async fn a_floor_above_the_free_space_holds_work_and_wakes_the_sweep() {
        let (guard, _directory) = guard(u64::MAX / (1024 * 1024)).await;
        guard.check(&mut None).await;
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
        guard.check(&mut None).await;
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
        guard.check(&mut None).await;
        assert!(guard.backend.disk_space().is_none());
    }
}
