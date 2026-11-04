//! #739: how many sessions a coordinator runs at once, the count its `start-sessions` grant's
//! `max_active` caps.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use cockpit_authorization::SessionId as Holder;
use cockpit_turso::sql::Connection;

use crate::approvals::Failed;

/// The sessions `coordinator` started that are not ended (asleep and failed ones included:
/// the owner or a wake brings them back), with its start requests still open in the `ask run`
/// queue, read in `connection`'s transaction. A session it started is one whose
/// `started_by` names it (#710), from the moment its row exists, or one a `started` audit
/// entry of its names: the sessions started before #710 have no `started_by`. Between an
/// unconfirmed start's audit and its session's row, a second start the same coordinator asks
/// is not counted against it: [`Launching`] counts those.
pub(crate) async fn active_workers(
    connection: &Connection,
    coordinator: &Holder,
    now: i64,
) -> Result<u32, Failed> {
    let started: HashSet<String> = cockpit_authorization::started_sessions(connection, coordinator)
        .await?
        .into_iter()
        .map(|session| session.as_str().to_owned())
        .collect();
    let running = cockpit_sessions::sessions(connection, false)
        .await?
        .into_iter()
        .filter(|session| {
            session.started_by.as_ref().map(|by| by.as_str()) == Some(coordinator.as_str())
                || started.contains(session.id.as_str())
        })
        .count();
    let waiting = cockpit_approvals::open_runs(connection, now)
        .await?
        .into_iter()
        .filter(|run| run.start.is_some() && run.session == *coordinator)
        .count();
    Ok(u32::try_from(running + waiting).unwrap_or(u32::MAX))
}

/// The starts made without confirmation whose session has no row yet, per coordinator: their
/// `start` is committed before `Sessions::start` creates the session, so [`active_workers`]
/// does not see them. Kept in memory: a start under way does not outlive the daemon.
#[derive(Default)]
pub(crate) struct Launching(Mutex<HashMap<String, u32>>);

impl Launching {
    pub(crate) fn count(&self, coordinator: &Holder) -> u32 {
        let launching = self.0.lock().expect("no thread panics holding it");
        launching.get(coordinator.as_str()).copied().unwrap_or(0)
    }

    /// Counts one start of `coordinator` until the guard drops. Begin it before committing the
    /// start's audit, so the next start reads it, and keep it until `Sessions::start` returns.
    pub(crate) fn begin(self: &Arc<Self>, coordinator: &Holder) -> LaunchingGuard {
        let mut launching = self.0.lock().expect("no thread panics holding it");
        *launching
            .entry(coordinator.as_str().to_owned())
            .or_insert(0) += 1;
        LaunchingGuard {
            launching: Arc::clone(self),
            coordinator: coordinator.as_str().to_owned(),
        }
    }
}

pub(crate) struct LaunchingGuard {
    launching: Arc<Launching>,
    coordinator: String,
}

impl Drop for LaunchingGuard {
    fn drop(&mut self) {
        let mut launching = self
            .launching
            .0
            .lock()
            .expect("no thread panics holding it");
        if let Some(count) = launching.get_mut(&self.coordinator) {
            *count -= 1;
            if *count == 0 {
                launching.remove(&self.coordinator);
            }
        }
    }
}
