//! #739: how many sessions a coordinator runs at once, the count its `start-sessions` grant's
//! `max_active` caps.

use std::collections::HashSet;

use cockpit_authorization::SessionId as Holder;
use cockpit_turso::sql::Connection;

use crate::approvals::Failed;

/// The sessions `coordinator` started that are not ended (asleep and failed ones included:
/// the owner or a wake brings them back), with its start requests still open in the `ask run`
/// queue, read in `connection`'s transaction. A session it started is one whose
/// `started_by` names it (#710), from the moment its row exists, or one a `started` audit
/// entry of its names: the sessions started before #710 have no `started_by`. Between an
/// unconfirmed start's audit and its session's row, a second start the same coordinator asks
/// is not counted against it.
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
