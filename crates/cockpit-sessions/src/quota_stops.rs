//! Quota stops (#158): one row per turn a plan's usage limit stopped, recorded in the
//! stopping report's own transaction. The daemon resumes each stop at most once, with one
//! continue message: once its account's plan window has refilled, or right after a quota-stop
//! relaunch moved the session to another account (#59). A stop ends resumed or dropped, never
//! both; it is dropped when the agent works again before that, when it stops again, when its
//! session ends, or when nothing can tell the daemon its reset, or its move to another account
//! is refused, and the owner is asked instead.
//!
//! A turn the network ended (#718) is recorded as a stop too, with its own [`StopCause`]: the
//! same pass resumes it once its provider answers again.

use std::collections::HashMap;
use std::sync::Arc;

use cockpit_provider_api::Provider;
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::store::now;
use crate::{Error, SessionId};

/// A recorded quota stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaStop {
    pub id: i64,
    pub session: SessionId,
    pub cause: StopCause,
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
    /// What the stop was inferred from, when the agent reported failed turns rather than a
    /// quota stop, such as "3 `rate_limit` failed turns within 30 minutes".
    pub inferred_from: Option<String>,
    pub resolution: Option<QuotaStopResolution>,
}

/// What ended the stopped turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopCause {
    /// A plan's usage limit.
    Quota,
    /// The network: the harness could not reach its provider (#718).
    Network,
}

impl StopCause {
    fn as_str(self) -> &'static str {
        match self {
            StopCause::Quota => "quota",
            StopCause::Network => "network",
        }
    }

    fn parse(stored: &str) -> Result<StopCause, Error> {
        match stored {
            "quota" => Ok(StopCause::Quota),
            "network" => Ok(StopCause::Network),
            other => Err(Error::InvalidStoredRow {
                reason: format!("unknown stop cause {other:?}"),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaStopResolution {
    /// The daemon claimed the stop's one continue message at `at`.
    Resumed { at: i64 },
    /// Nothing will resume the stop, for `reason`.
    Dropped { at: i64, reason: String },
}

const SELECT_STOP: &str = "SELECT id, session_id, launch_id, provider, account, stopped_at,
    confirmed_at, resets_at, switched_at, resumed_at, dropped_at, dropped_reason, inferred_from,
    cause FROM session_quota_stops AS q";

const IS_OPEN: &str = "resumed_at IS NULL AND dropped_at IS NULL";

/// Records that `cause` stopped `launch`'s turn now, `inferred_from` failed turns when the
/// agent did not say so, dropping an earlier stop of the session still open: only the latest
/// stop is resumed.
pub(crate) async fn record(
    write: &WriteTransaction<'_>,
    session: &str,
    launch: i64,
    cause: StopCause,
    inferred_from: Option<&str>,
) -> Result<(), Error> {
    let at = now();
    drop_open(write, session, "another turn of the agent stopped", at).await?;
    write
        .execute(
            "INSERT INTO session_quota_stops
                 (session_id, launch_id, provider, account, stopped_at, inferred_from, cause)
             SELECT s.id, l.id, s.provider, l.account, ?3, ?4, ?5 FROM session_launches l
             JOIN sessions s ON s.id = l.session_id
             WHERE s.id = ?1 AND l.id = ?2",
            (session, launch, at, inferred_from, cause.as_str()),
        )
        .await?;
    Ok(())
}

/// How soon after a network stop's resume another one goes to the owner instead (#718): the
/// provider answers the daemon's readings but not the agent, so resuming again would loop.
pub const NETWORK_STOP_REPEAT_SECONDS: i64 = 30 * 60;

/// Records that the network ended `launch`'s turn now, as [`record`] does. A second one within
/// [`NETWORK_STOP_REPEAT_SECONDS`] of the session's last network resume goes to the owner.
pub(crate) async fn record_network(
    write: &WriteTransaction<'_>,
    session: &str,
    launch: i64,
) -> Result<(), Error> {
    let mut rows = write
        .query(
            "SELECT 1 FROM session_quota_stops
             WHERE session_id = ?1 AND cause = 'network' AND resumed_at >= ?2 LIMIT 1",
            (session, now() - NETWORK_STOP_REPEAT_SECONDS),
        )
        .await?;
    let is_repeat = rows.next().await?.is_some();
    drop(rows);
    record(write, session, launch, StopCause::Network, None).await?;
    if is_repeat {
        let reason = "the network ended its turn again soon after Torlek resumed it, though its \
                      provider answers Torlek: continue the session yourself";
        hand_open_to_owner(write, &SessionId::parse(session)?, reason).await?;
    }
    Ok(())
}

/// How many failed turns of [`TransientLimits`] within [`TRANSIENT_LIMIT_WINDOW_SECONDS`] make
/// a quota stop (#158, owner decision 2026-11-09).
pub const TRANSIENT_LIMIT_FAILURES: i64 = 3;
pub const TRANSIENT_LIMIT_WINDOW_SECONDS: i64 = 30 * 60;

/// Each provider's failure reasons that it gives both for a spent plan window and for a
/// transient limit ([`cockpit_provider_api::Provider::transient_limit_reasons`]).
#[derive(Debug, Clone, Default)]
pub struct TransientLimits {
    reasons: HashMap<String, &'static [&'static str]>,
}

impl TransientLimits {
    /// The reasons each of `providers` declares.
    pub fn of(providers: &[Arc<dyn Provider>]) -> TransientLimits {
        TransientLimits {
            reasons: providers
                .iter()
                .map(|provider| {
                    (
                        provider.name().as_str().to_owned(),
                        provider.transient_limit_reasons(),
                    )
                })
                .filter(|(_, reasons)| !reasons.is_empty())
                .collect(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.reasons.is_empty()
    }

    pub(crate) fn counts(&self, provider: &str, reason: &str) -> bool {
        self.reasons
            .get(provider)
            .is_some_and(|reasons| reasons.contains(&reason))
    }
}

/// Counts a failed turn of `launch` for `reason` now. What a quota stop is inferred from,
/// once this makes [`TRANSIENT_LIMIT_FAILURES`] of the launch for the same reason within the
/// window; the count then starts over. Another reason starts it over too, so the provenance
/// names the one reason counted.
pub(crate) async fn count_limit_failure(
    write: &WriteTransaction<'_>,
    session: &str,
    launch: i64,
    reason: &str,
) -> Result<Option<String>, Error> {
    let at = now();
    let window_start = at - TRANSIENT_LIMIT_WINDOW_SECONDS;
    write
        .execute(
            "DELETE FROM session_limit_failures
             WHERE session_id = ?1 AND (launch_id <> ?2 OR failed_at <= ?3 OR reason <> ?4)",
            (session, launch, window_start, reason),
        )
        .await?;
    write
        .execute(
            "INSERT INTO session_limit_failures (session_id, launch_id, reason, failed_at)
             VALUES (?1, ?2, ?3, ?4)",
            (session, launch, reason, at),
        )
        .await?;
    let mut rows = write
        .query(
            "SELECT count(*) FROM session_limit_failures WHERE session_id = ?1",
            [session],
        )
        .await?;
    let failures: i64 = match rows.next().await? {
        Some(row) => row.get(0)?,
        None => 0,
    };
    drop(rows);
    if failures < TRANSIENT_LIMIT_FAILURES {
        return Ok(None);
    }
    clear_limit_failures(write, session).await?;
    Ok(Some(format!(
        "{failures} `{reason}` failed turns within {} minutes",
        TRANSIENT_LIMIT_WINDOW_SECONDS / 60
    )))
}

/// Forgets `session`'s counted failures: a turn that ended otherwise breaks the run.
pub(crate) async fn clear_limit_failures(
    write: &WriteTransaction<'_>,
    session: &str,
) -> Result<(), Error> {
    write
        .execute(
            "DELETE FROM session_limit_failures WHERE session_id = ?1",
            [session],
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

/// Hands `session`'s open stop, if any, to the owner for `reason`, as
/// [`hand_quota_stop_to_owner`] does. Whether it did.
pub(crate) async fn hand_open_to_owner(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    reason: &str,
) -> Result<bool, Error> {
    let mut rows = write
        .query(
            &format!("{SELECT_STOP} WHERE session_id = ?1 AND {IS_OPEN}"),
            [session.as_str()],
        )
        .await?;
    let open = match rows.next().await? {
        Some(row) => stop_from(&row)?,
        None => return Ok(false),
    };
    drop(rows);
    hand_quota_stop_to_owner(write, &open, reason).await
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

/// Whether a turn the network ended on `provider` still waits for its resume.
pub async fn is_network_stopped(connection: &Connection, provider: &str) -> Result<bool, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT 1 FROM session_quota_stops
                 WHERE provider = ?1 AND cause = 'network' AND {IS_OPEN} LIMIT 1"
            ),
            [provider],
        )
        .await?;
    Ok(rows.next().await?.is_some())
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
/// Whether the stop was still open to hand over.
pub async fn hand_quota_stop_to_owner(
    write: &WriteTransaction<'_>,
    stop: &QuotaStop,
    reason: &str,
) -> Result<bool, Error> {
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
        return Ok(false);
    }
    let session =
        cockpit_authorization::SessionId::new(stop.session.as_str()).map_err(|error| {
            Error::InvalidStoredRow {
                reason: error.to_string(),
            }
        })?;
    crate::reports::flag_owner_needed(write, &session, stop.launch, reason).await?;
    Ok(true)
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
        cause: StopCause::parse(&row.get::<String>(13)?)?,
        launch: row.get(2)?,
        provider: row.get(3)?,
        account: row.get(4)?,
        stopped_at: row.get(5)?,
        confirmed_at: row.get(6)?,
        resets_at: row.get(7)?,
        switched_at: row.get(8)?,
        inferred_from: row.get(12)?,
        resolution,
    })
}
