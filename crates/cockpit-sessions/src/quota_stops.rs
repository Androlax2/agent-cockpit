//! Quota stops (#158): one row per turn a plan's usage limit stopped, recorded in the
//! stopping report's own transaction. The daemon resumes each stop at most once, with one
//! continue message: once its account's plan window has refilled, or right after a quota-stop
//! relaunch moved the session to another account (#59). A stop ends resumed or dropped, never
//! both; it is dropped when the agent works again before that, when it stops again, when its
//! session ends, or when nothing can tell the daemon its reset and the owner is asked instead.

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::store::now;
use crate::{Error, SessionId};

/// A recorded quota stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaStop {
    pub id: i64,
    pub session: SessionId,
    /// The launch whose turn the limit stopped.
    pub launch: i64,
    pub provider: String,
    /// The launch's account; `None` for an agent on the owner's own sign-in.
    pub account: Option<String>,
    /// Seconds since 1970.
    pub stopped_at: i64,
    /// When the plan-window reading that showed the limit spent, taken after the stop, was
    /// taken: the reset counts only once a later reading shows room.
    pub confirmed_at: Option<i64>,
    /// When that reading said the spent windows start over, when it said.
    pub resets_at: Option<i64>,
    /// When the quota-stop relaunch moved the session to another account (#59).
    pub switched_at: Option<i64>,
    pub resolution: Option<QuotaStopResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaStopResolution {
    /// The daemon claimed the stop's one continue message at `at`.
    Resumed { at: i64 },
    /// Nothing will resume the stop, for `reason`.
    Dropped { at: i64, reason: String },
}

impl QuotaStop {
    pub fn is_open(&self) -> bool {
        self.resolution.is_none()
    }
}

const SELECT_STOP: &str = "SELECT id, session_id, launch_id, provider, account, stopped_at,
    confirmed_at, resets_at, switched_at, resumed_at, dropped_at, dropped_reason
    FROM session_quota_stops AS q";

const IS_OPEN: &str = "resumed_at IS NULL AND dropped_at IS NULL";

/// Records that `launch`'s turn quota-stopped now, dropping an earlier stop of the session
/// still open: only the latest stop is resumed.
pub(crate) async fn record(
    write: &WriteTransaction<'_>,
    session: &str,
    launch: i64,
) -> Result<(), Error> {
    let at = now();
    drop_open(write, session, "the agent quota-stopped again", at).await?;
    write
        .execute(
            "INSERT INTO session_quota_stops (session_id, launch_id, provider, account, stopped_at)
             SELECT s.id, l.id, s.provider, l.account, ?3 FROM session_launches l
             JOIN sessions s ON s.id = l.session_id
             WHERE s.id = ?1 AND l.id = ?2",
            (session, launch, at),
        )
        .await?;
    Ok(())
}

/// Drops `session`'s open stop, if any, for `reason`.
pub(crate) async fn drop_open(
    write: &WriteTransaction<'_>,
    session: &str,
    reason: &str,
    at: i64,
) -> Result<(), Error> {
    write
        .execute(
            &format!(
                "UPDATE session_quota_stops SET dropped_at = ?2, dropped_reason = ?3
                 WHERE session_id = ?1 AND {IS_OPEN}"
            ),
            (session, at, reason),
        )
        .await?;
    Ok(())
}

/// Notes that the quota-stop relaunch moved `session` to another account now, so its open
/// stop is due its continue message without waiting for a reset.
pub(crate) async fn mark_switched(
    write: &WriteTransaction<'_>,
    session: &SessionId,
) -> Result<(), Error> {
    write
        .execute(
            &format!(
                "UPDATE session_quota_stops SET switched_at = ?2
                 WHERE session_id = ?1 AND {IS_OPEN}"
            ),
            (session.as_str(), now()),
        )
        .await?;
    Ok(())
}

/// Every open stop, oldest first.
pub async fn open_quota_stops(connection: &Connection) -> Result<Vec<QuotaStop>, Error> {
    let mut rows = connection
        .query(&format!("{SELECT_STOP} WHERE {IS_OPEN} ORDER BY id"), ())
        .await?;
    let mut stops = Vec::new();
    while let Some(row) = rows.next().await? {
        stops.push(stop_from(&row)?);
    }
    Ok(stops)
}

/// Each session's latest stop, open or resolved, for its card.
pub async fn latest_quota_stops(connection: &Connection) -> Result<Vec<QuotaStop>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "{SELECT_STOP} WHERE id = (SELECT max(latest.id) FROM session_quota_stops AS latest
                 WHERE latest.session_id = q.session_id) ORDER BY id"
            ),
            (),
        )
        .await?;
    let mut stops = Vec::new();
    while let Some(row) = rows.next().await? {
        stops.push(stop_from(&row)?);
    }
    Ok(stops)
}

/// Records that the reading taken at `observed_at` showed `stop`'s limit spent, until
/// `resets_at` when it said. Only the first confirmation of an open stop counts.
pub async fn confirm_quota_stop(
    write: &WriteTransaction<'_>,
    stop: i64,
    observed_at: i64,
    resets_at: Option<i64>,
) -> Result<(), Error> {
    write
        .execute(
            &format!(
                "UPDATE session_quota_stops SET confirmed_at = ?2, resets_at = ?3
                 WHERE id = ?1 AND confirmed_at IS NULL AND {IS_OPEN}"
            ),
            (stop, observed_at, resets_at),
        )
        .await?;
    Ok(())
}

/// Claims `stop`'s one continue message: `false` when it is no longer open. Claimed before
/// the message is sent, so a daemon that dies after sending never sends it again.
pub async fn claim_quota_resume(write: &WriteTransaction<'_>, stop: i64) -> Result<bool, Error> {
    let claimed = write
        .execute(
            &format!("UPDATE session_quota_stops SET resumed_at = ?2 WHERE id = ?1 AND {IS_OPEN}"),
            (stop, now()),
        )
        .await?;
    Ok(claimed == 1)
}

/// Gives back a claim whose message was certainly not sent, so a later pass sends it.
pub async fn release_quota_resume(write: &WriteTransaction<'_>, stop: i64) -> Result<(), Error> {
    write
        .execute(
            "UPDATE session_quota_stops SET resumed_at = NULL
             WHERE id = ?1 AND dropped_at IS NULL",
            [stop],
        )
        .await?;
    Ok(())
}

/// Drops `stop` for `reason` while it is open.
pub async fn drop_quota_stop(
    write: &WriteTransaction<'_>,
    stop: i64,
    reason: &str,
) -> Result<(), Error> {
    write
        .execute(
            &format!(
                "UPDATE session_quota_stops SET dropped_at = ?2, dropped_reason = ?3
                 WHERE id = ?1 AND {IS_OPEN}"
            ),
            (stop, now(), reason),
        )
        .await?;
    Ok(())
}

/// Drops `stop` for `reason` and asks the owner to continue the session: `reason` becomes a
/// dialog question on the stop's launch while it is the session's latest, which the agent's
/// next report of what it does clears. A stop a relaunch moved meanwhile is left due.
pub async fn hand_quota_stop_to_owner(
    write: &WriteTransaction<'_>,
    stop: &QuotaStop,
    reason: &str,
) -> Result<(), Error> {
    let dropped = write
        .execute(
            &format!(
                "UPDATE session_quota_stops SET dropped_at = ?2, dropped_reason = ?3
                 WHERE id = ?1 AND switched_at IS NULL AND {IS_OPEN}"
            ),
            (stop.id, now(), reason),
        )
        .await?;
    if dropped != 1 {
        return Ok(());
    }
    let session =
        cockpit_authorization::SessionId::new(stop.session.as_str()).map_err(|error| {
            Error::InvalidStoredRow {
                reason: error.to_string(),
            }
        })?;
    crate::reports::flag_owner_needed(write, &session, stop.launch, reason).await?;
    Ok(())
}

fn stop_from(row: &Row) -> Result<QuotaStop, Error> {
    let session: String = row.get(1)?;
    let resumed_at: Option<i64> = row.get(9)?;
    let dropped_at: Option<i64> = row.get(10)?;
    let dropped_reason: Option<String> = row.get(11)?;
    let resolution = match (resumed_at, dropped_at, dropped_reason) {
        (None, None, None) => None,
        (Some(at), None, None) => Some(QuotaStopResolution::Resumed { at }),
        (None, Some(at), Some(reason)) => Some(QuotaStopResolution::Dropped { at, reason }),
        _ => {
            return Err(Error::InvalidStoredRow {
                reason: "a quota stop is resumed or dropped with a reason, never both".to_owned(),
            });
        }
    };
    Ok(QuotaStop {
        id: row.get(0)?,
        session: SessionId::parse(&session)?,
        launch: row.get(2)?,
        provider: row.get(3)?,
        account: row.get(4)?,
        stopped_at: row.get(5)?,
        confirmed_at: row.get(6)?,
        resets_at: row.get(7)?,
        switched_at: row.get(8)?,
        resolution,
    })
}
