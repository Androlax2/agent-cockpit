//! Resumes quota-stopped sessions (#158): each open quota stop gets one continue message once
//! its account's plan window has refilled, or right after the quota-stop relaunch moved the
//! session to another account (#59).
//!
//! The reset is read from the daemon's own plan-window readings, never from the agent: a
//! reading taken after the stop must show the limit spent, then a later one must show room.
//! A stop no reading can judge goes to the owner as a needs-you question instead: an account
//! the daemon reads no windows for, or windows that show room although the agent hit its
//! limit. Sessions holding a coordinator grant go first; a worker waits while a coordinator
//! due its resume is still starting or busy, though not while it asks the owner something.
//! An asleep session is woken, then resumed once its agent waits at its prompt. Each resume
//! is claimed in the database before the message is sent, so none is sent twice.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use cockpit_sessions::{
    Addressed, AgentState, QuotaStop, Session, SessionState, Sessions, WakeCause,
};
use cockpit_turso::Database;
use cockpit_usage::{AccountQuota, QuotaPolicy, WindowState};

/// The one message a resumed session gets.
pub(crate) const CONTINUE: &str = "Torlek: the plan usage limit that stopped your last turn has \
     reset. Continue where you stopped.";

/// The plan-window readings this daemon takes: the policy that judges them, and the providers
/// it reads the owner's own sign-in of (#58).
#[derive(Debug, Clone)]
pub(crate) struct Readings {
    pub(crate) policy: QuotaPolicy,
    pub(crate) providers: HashSet<String>,
}

/// Runs [`resume_due`] every `interval` until aborted.
pub(crate) async fn resume_forever(
    database: Arc<Database>,
    sessions: Arc<Sessions>,
    readings: Option<Readings>,
    interval: Duration,
) {
    let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        if let Err(error) = resume_due(&database, &sessions, readings.as_ref()).await {
            tracing::error!(%error, "the quota-stop resume pass failed");
        }
    }
}

/// What one open stop needs now.
#[derive(Debug, PartialEq)]
enum Step {
    Wait,
    /// A reading taken at `observed_at`, after the stop, shows the limit spent.
    Confirm {
        observed_at: i64,
        resets_at: Option<i64>,
    },
    Due,
    Drop(String),
    HandToOwner(String),
}

/// One pass over every open quota stop. Without `readings` this daemon takes none, and only
/// stops a relaunch moved are resumed.
pub(crate) async fn resume_due(
    database: &Database,
    sessions: &Sessions,
    readings: Option<&Readings>,
) -> Result<(), String> {
    let (stops, quotas, coordinators) = {
        let read = database.read().await.map_err(|error| error.to_string())?;
        let stops = cockpit_sessions::open_quota_stops(&read)
            .await
            .map_err(|error| error.to_string())?;
        if stops.is_empty() {
            return Ok(());
        }
        let quotas = match readings {
            Some(readings) => {
                cockpit_usage::quotas(&read, &readings.policy, crate::approvals::now())
                    .await
                    .map_err(|error| error.to_string())?
            }
            None => Vec::new(),
        };
        let coordinators: HashSet<String> = cockpit_authorization::active_coordinator_grants(&read)
            .await
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|grant| grant.holder.as_str().to_owned())
            .collect();
        (stops, quotas, coordinators)
    };
    let (mut first, workers): (Vec<QuotaStop>, Vec<QuotaStop>) = stops
        .into_iter()
        .partition(|stop| coordinators.contains(stop.session.as_str()));
    first.extend(workers);
    let mut is_coordinator_pending = false;
    for stop in first {
        let is_coordinator = coordinators.contains(stop.session.as_str());
        let session = match sessions.get(&stop.session).await {
            Ok(session) => session,
            Err(cockpit_sessions::Error::UnknownSession { .. }) => {
                write_drop(database, &stop, "the session is gone").await?;
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        let step = match judge(&stop, &session, &quotas) {
            Step::Wait
                if !is_changing(&session) && needs_switch_check(&stop, &quotas, readings) =>
            {
                switch_or_owner(sessions, &stop).await?
            }
            step => step,
        };
        match step {
            Step::Wait => {}
            Step::Confirm {
                observed_at,
                resets_at,
            } => {
                let write = database.write().await.map_err(|error| error.to_string())?;
                cockpit_sessions::confirm_quota_stop(&write, stop.id, observed_at, resets_at)
                    .await
                    .map_err(|error| error.to_string())?;
                write.commit().await.map_err(|error| error.to_string())?;
                tracing::info!(
                    session = %stop.session,
                    ?resets_at,
                    "a quota stop's plan window reads spent; it resumes once it refills"
                );
            }
            Step::Drop(reason) => write_drop(database, &stop, &reason).await?,
            Step::HandToOwner(reason) => {
                let write = database.write().await.map_err(|error| error.to_string())?;
                cockpit_sessions::hand_quota_stop_to_owner(&write, &stop, &reason)
                    .await
                    .map_err(|error| error.to_string())?;
                write.commit().await.map_err(|error| error.to_string())?;
                tracing::warn!(session = %stop.session, reason, "a quota stop waits on the owner");
            }
            Step::Due if !is_coordinator && is_coordinator_pending => {}
            Step::Due => {
                let resumed = resume(database, sessions, &stop, &session).await?;
                is_coordinator_pending |= is_coordinator && resumed == Resumed::Pending;
            }
        }
    }
    Ok(())
}

/// What `stop` of `session` needs, judged by the recorded `quotas`.
fn judge(stop: &QuotaStop, session: &Session, quotas: &[AccountQuota]) -> Step {
    match session.state {
        SessionState::Ended => return Step::Drop("the session ended".to_owned()),
        SessionState::Failed => return Step::Drop("the session failed".to_owned()),
        _ => {}
    }
    if stop.switched_at.is_some() {
        return Step::Due;
    }
    let Some(quota) = quota_of(stop, quotas) else {
        // Settled by `switch_or_owner`: a relaunch may still move it.
        return Step::Wait;
    };
    let Some(observed_at) = quota.observed_at else {
        return Step::Wait;
    };
    if observed_at <= stop.stopped_at || !quota.is_current {
        return Step::Wait;
    }
    let spent: Vec<_> = quota
        .windows
        .iter()
        .filter(|window| matches!(window.state, WindowState::Exhausted | WindowState::Blocked))
        .collect();
    let is_spent = quota.is_blocked || !spent.is_empty();
    match stop.confirmed_at {
        None if is_spent => Step::Confirm {
            observed_at,
            resets_at: spent.iter().filter_map(|window| window.resets_at).max(),
        },
        None => Step::HandToOwner(
            "quota-stopped, but its plan windows show room: Torlek cannot tell its reset, so \
             continue the session yourself"
                .to_owned(),
        ),
        Some(confirmed_at) if observed_at > confirmed_at && !is_spent => Step::Due,
        Some(_) => Step::Wait,
    }
}

/// Whether `stop` has no reading to judge it by and none will come, so only a relaunch or the
/// owner moves it. A stop on the owner's own sign-in of a provider the daemon reads waits for
/// its first reading.
fn needs_switch_check(
    stop: &QuotaStop,
    quotas: &[AccountQuota],
    readings: Option<&Readings>,
) -> bool {
    let is_read = stop.account.is_none()
        && readings.is_some_and(|readings| readings.providers.contains(&stop.provider));
    stop.switched_at.is_none() && !is_read && quota_of(stop, quotas).is_none()
}

/// Whether `session` is launching or stopping, as a quota-stop relaunch does on its way to
/// another account.
fn is_changing(session: &Session) -> bool {
    matches!(
        session.state,
        SessionState::Launching | SessionState::Stopping { .. }
    )
}

/// For a stop with no reading: waits while the relaunch will move it to another account,
/// otherwise hands it to the owner.
async fn switch_or_owner(sessions: &Sessions, stop: &QuotaStop) -> Result<Step, String> {
    if sessions
        .will_switch_account(&stop.session)
        .await
        .map_err(|error| error.to_string())?
    {
        return Ok(Step::Wait);
    }
    let account = stop
        .account
        .as_deref()
        .unwrap_or(cockpit_config::DEFAULT_ACCOUNT);
    Ok(Step::HandToOwner(format!(
        "quota-stopped on account {account}, whose plan windows Torlek does not read, so it \
         cannot tell the reset: continue the session yourself"
    )))
}

/// The readings of the account `stop` ran on: its launch's, or the owner's own sign-in,
/// which the daemon reads as `default` (#58).
fn quota_of<'quotas>(
    stop: &QuotaStop,
    quotas: &'quotas [AccountQuota],
) -> Option<&'quotas AccountQuota> {
    let account = stop
        .account
        .as_deref()
        .unwrap_or(cockpit_config::DEFAULT_ACCOUNT);
    quotas
        .iter()
        .find(|quota| quota.provider.as_str() == stop.provider && quota.account.as_str() == account)
}

/// How far one pass got with a due stop.
#[derive(Debug, PartialEq)]
enum Resumed {
    /// The continue message went out, or no pass will send it.
    Settled,
    /// The session is starting, or busy, so a later pass sends it soon.
    Pending,
    /// The agent asks the owner something first, which may take a while.
    WaitsOnOwner,
}

/// Wakes an asleep session, or sends a running one waiting at its prompt its continue
/// message.
async fn resume(
    database: &Database,
    sessions: &Sessions,
    stop: &QuotaStop,
    session: &Session,
) -> Result<Resumed, String> {
    match session.state {
        SessionState::Asleep => {
            match sessions
                .ensure_awake(&stop.session, WakeCause::QuotaReset)
                .await
            {
                Ok(_) => {
                    tracing::info!(session = %stop.session, "woke a session at its plan's reset")
                }
                Err(error) => {
                    tracing::warn!(session = %stop.session, %error, "waking a session at its plan's reset failed");
                }
            }
            return Ok(Resumed::Pending);
        }
        SessionState::Running => {}
        _ => return Ok(Resumed::Pending),
    }
    match prompt_of(database, session).await? {
        Prompt::Waiting => {}
        Prompt::Asking => return Ok(Resumed::WaitsOnOwner),
        Prompt::NotYet => return Ok(Resumed::Pending),
    }
    let write = database.write().await.map_err(|error| error.to_string())?;
    let is_claimed = cockpit_sessions::claim_quota_resume(&write, stop.id)
        .await
        .map_err(|error| error.to_string())?;
    write.commit().await.map_err(|error| error.to_string())?;
    if !is_claimed {
        return Ok(Resumed::Settled);
    }
    match sessions
        .submit(&stop.session, CONTINUE, Addressed::Work)
        .await
    {
        Ok(()) => {
            tracing::info!(
                session = %stop.session,
                switched = stop.switched_at.is_some(),
                "resumed a quota-stopped session"
            );
        }
        // The message may have reached the agent: sending it again could make two.
        Err(
            error @ (cockpit_sessions::Error::TypingUnconfirmed { .. }
            | cockpit_sessions::Error::DeliveryUnconfirmed { .. }),
        ) => {
            tracing::warn!(session = %stop.session, %error, "a quota stop's continue message may not have arrived; not sent again");
        }
        Err(error) => {
            tracing::warn!(session = %stop.session, %error, "a quota stop's continue message was not sent; a later pass tries again");
            let write = database.write().await.map_err(|error| error.to_string())?;
            cockpit_sessions::release_quota_resume(&write, stop.id)
                .await
                .map_err(|error| error.to_string())?;
            write.commit().await.map_err(|error| error.to_string())?;
            if matches!(error, cockpit_sessions::Error::SessionBusy { .. }) {
                return Ok(Resumed::Pending);
            }
        }
    }
    Ok(Resumed::Settled)
}

/// Where the agent of a running session's current launch stands, by its latest report.
enum Prompt {
    /// At its prompt, with no question open.
    Waiting,
    /// With a question open for the owner.
    Asking,
    /// Not reported on this launch yet, or still busy.
    NotYet,
}

async fn prompt_of(database: &Database, session: &Session) -> Result<Prompt, String> {
    let Some(launch) = &session.launch else {
        return Ok(Prompt::NotYet);
    };
    let holder = cockpit_authorization::SessionId::new(session.id.as_str())
        .map_err(|error| error.to_string())?;
    let read = database.read().await.map_err(|error| error.to_string())?;
    let report = cockpit_sessions::latest_report(&read, &holder)
        .await
        .map_err(|error| error.to_string())?;
    let Some(report) = report.filter(|report| report.launch == Some(launch.number)) else {
        return Ok(Prompt::NotYet);
    };
    if report.needs_input.is_some() {
        return Ok(Prompt::Asking);
    }
    if report.state == Some(AgentState::Waiting) {
        return Ok(Prompt::Waiting);
    }
    Ok(Prompt::NotYet)
}

async fn write_drop(database: &Database, stop: &QuotaStop, reason: &str) -> Result<(), String> {
    let write = database.write().await.map_err(|error| error.to_string())?;
    cockpit_sessions::drop_quota_stop(&write, stop.id, reason)
        .await
        .map_err(|error| error.to_string())?;
    write.commit().await.map_err(|error| error.to_string())?;
    tracing::info!(session = %stop.session, reason, "a quota stop was dropped");
    Ok(())
}
