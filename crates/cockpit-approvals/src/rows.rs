//! What run requests and guides share in storage: an asking state that expires unless the
//! agent keeps polling, the asking project, and how a stored row is refused.

use std::path::PathBuf;

use cockpit_authorization::SessionId;
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::Row;

use crate::runs::{EXPIRES_AFTER_SECONDS, Project};
use crate::{Error, Refusal};

/// A poll moves the expiry forward only once it is this much older than a fresh one, so
/// polls inside that window write nothing: an asking request outlives the last poll by at
/// least `EXPIRES_AFTER_SECONDS - REFRESH_AFTER_SECONDS`.
const REFRESH_AFTER_SECONDS: i64 = 60;

/// A request state that starts as asking and reads as expired once nobody polls for it.
pub(crate) trait AskState: Copy + PartialEq {
    const ASKING: Self;
    /// Never stored: an asking row past its `expires_at`.
    const EXPIRED: Self;

    fn as_str(self) -> &'static str;
}

/// The stored state as it reads at `now`.
pub(crate) fn state_at<S: AskState>(stored: S, expires_at: i64, now: i64) -> S {
    if stored == S::ASKING && now >= expires_at {
        S::EXPIRED
    } else {
        stored
    }
}

/// Refuses a decision on a request that no longer asks.
pub(crate) fn still_asking<S: AskState>(state: S) -> Result<(), Refusal> {
    if state == S::ASKING {
        Ok(())
    } else if state == S::EXPIRED {
        Err(Refusal::Expired)
    } else {
        Err(Refusal::AlreadyDecided {
            state: state.as_str(),
        })
    }
}

/// The expiry a poll at `now` should store, or `None` while the stored one is still fresh.
pub(crate) fn refreshed_expiry(expires_at: i64, now: i64) -> Option<i64> {
    let fresh = now + EXPIRES_AFTER_SECONDS;
    (fresh - expires_at >= REFRESH_AFTER_SECONDS).then_some(fresh)
}

/// The next free id of `table`, one of this crate's tables.
pub(crate) async fn next_id(write: &WriteTransaction<'_>, table: &str) -> Result<i64, Error> {
    Ok(write
        .prepare(&format!("SELECT coalesce(max(id), 0) + 1 FROM {table}"))
        .await?
        .query_row(())
        .await?
        .get::<i64>(0)?)
}

pub(crate) fn invalid_row(table: &'static str, reason: String) -> Error {
    Error::InvalidStoredRow { table, reason }
}

pub(crate) fn session_from(
    row: &Row,
    column: usize,
    table: &'static str,
) -> Result<SessionId, Error> {
    SessionId::new(row.get::<String>(column)?)
        .map_err(|error| invalid_row(table, error.to_string()))
}

/// The project stored in the `name` and `root` columns, which are null together.
pub(crate) fn project_from(row: &Row, name: usize, root: usize) -> Result<Option<Project>, Error> {
    Ok(
        match (
            row.get::<Option<String>>(name)?,
            row.get::<Option<String>>(root)?,
        ) {
            (Some(name), Some(root)) => Some(Project {
                name,
                root: PathBuf::from(root),
            }),
            _ => None,
        },
    )
}
