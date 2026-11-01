//! Sessions and checkouts on the change feed (#199): after each commit the daemon publishes
//! their current wire views and retracts the ones gone, so clients follow the feed instead of
//! polling the lists. The daemon is the only producer of the `session` and `checkout` kinds.
//!
//! The views are published in a transaction of their own right after the commit that changed
//! them, not inside it (`docs/decisions/2026-10-31-199-change-feed-projector.md`). One
//! task publishes, in order, so a view never goes back to an older state. A pass that finds
//! what the last one wrote takes no write transaction, so it does not wake itself.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use cockpit_sessions::Sessions;
use cockpit_turso::{Database, WriteTransaction};
use serde_json::{Map, Value};

const SESSION: &str = "session";
const CHECKOUT: &str = "checkout";
/// How long a pass that failed waits before it runs again, commit or not.
const RETRY: Duration = Duration::from_secs(1);
/// A pass also runs this often without a commit: the changed start files are read from disk
/// and trust from the owner's configuration, which change with no commit.
const PERIOD: Duration = Duration::from_secs(5);

/// Published states by entity id.
type States = BTreeMap<String, Map<String, Value>>;

/// What one pass derived. Equal to the last pass's, there is nothing to write.
#[derive(PartialEq)]
struct Projection {
    sessions: States,
    /// Sessions whose view could not be read this pass: their published state is kept.
    kept_sessions: BTreeSet<String>,
    /// `None` while the owner's configuration cannot be read: the published checkouts stay.
    checkouts: Option<States>,
}

/// Publishes at start, after every commit and every [`PERIOD`]. Runs until aborted.
pub(crate) async fn publish_forever(
    database: Arc<Database>,
    sessions: Arc<Sessions>,
    environment: cockpit_config::Environment,
) {
    // Watched before the first pass, so a commit during it runs another.
    let mut commits = database.watch_commits();
    let mut period = tokio::time::interval(PERIOD);
    period.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut written: Option<Projection> = None;
    let mut config_problem: Option<String> = None;
    loop {
        let (sessions_read, checkouts_read) = tokio::join!(
            session_states(&sessions),
            checkout_states(&database, &environment)
        );
        // A broken owner configuration leaves the published checkouts as they are rather than
        // publishing every checkout untrusted. Logged once per problem, not once per pass.
        let checkouts = match checkouts_read {
            Ok(checkouts) => {
                config_problem = None;
                Some(checkouts)
            }
            Err(error) => {
                let problem = error.to_string();
                if config_problem.as_ref() != Some(&problem) {
                    tracing::warn!(
                        problem,
                        "the checkouts on the change feed are left unchanged"
                    );
                    config_problem = Some(problem);
                }
                None
            }
        };
        let outcome = match sessions_read {
            Err(error) => Err(format!("cannot read the sessions: {error}")),
            Ok((sessions, kept_sessions)) => {
                let projection = Projection {
                    sessions,
                    kept_sessions,
                    checkouts,
                };
                if written.as_ref() == Some(&projection) {
                    Ok(())
                } else {
                    write(&database, &projection)
                        .await
                        .map(|()| written = Some(projection))
                        .map_err(|error| error.to_string())
                }
            }
        };
        if let Err(problem) = outcome {
            tracing::warn!(
                problem,
                "publishing sessions and checkouts failed; retrying"
            );
            tokio::time::sleep(RETRY).await;
            continue;
        }
        tokio::select! {
            changed = commits.changed() => if changed.is_err() { return },
            _ = period.tick() => {}
        }
    }
}

async fn write(
    database: &Database,
    projection: &Projection,
) -> Result<(), cockpit_messaging::Error> {
    let write = database.write().await?;
    let mut has_changed = replace(
        &write,
        SESSION,
        &projection.sessions,
        &projection.kept_sessions,
    )
    .await?;
    if let Some(checkouts) = &projection.checkouts {
        has_changed |= replace(&write, CHECKOUT, checkouts, &BTreeSet::new()).await?;
    }
    if has_changed {
        write.commit().await?;
    } else {
        write.rollback().await?;
    }
    Ok(())
}

/// The sessions not ended, as `session-list` answers them; an ended one is retracted. A
/// session whose view cannot be read is logged and kept as published, so one bad session
/// does not stop the others from reaching the feed.
async fn session_states(
    sessions: &Sessions,
) -> Result<(States, BTreeSet<String>), cockpit_sessions::Error> {
    let listed = sessions.list(false).await?;
    // The finished label needs the task list; without it every session is kept as published,
    // and the checkouts still go out.
    let inputs = match crate::sessions::card_inputs(sessions).await {
        Ok(inputs) => inputs,
        Err(failed) => {
            tracing::warn!(error = ?failed.0, "the sessions on the change feed are left unchanged");
            let kept = listed
                .iter()
                .map(|session| session.id.to_string())
                .collect();
            return Ok((States::new(), kept));
        }
    };
    let mut states = States::new();
    let mut kept = BTreeSet::new();
    for session in listed {
        match crate::sessions::current_view(sessions, &session, &inputs).await {
            Ok(view) => {
                states.insert(view.id.clone(), state_of(&view));
            }
            Err(error) => {
                tracing::warn!(session = %session.id, %error, "a session's view on the change feed is left unchanged");
                kept.insert(session.id.to_string());
            }
        }
    }
    Ok((states, kept))
}

/// The checkouts not removed, as `checkout-list` answers them; a removed one is retracted.
async fn checkout_states(
    database: &Database,
    environment: &cockpit_config::Environment,
) -> Result<States, cockpit_projects::Error> {
    let trusted = crate::projects::trusted_roots(environment.clone()).await?;
    let read = database.read().await?;
    Ok(cockpit_projects::checkouts(&read, None)
        .await?
        .iter()
        .map(|checkout| {
            let view = crate::projects::checkout_view(checkout, &trusted);
            (view.id.to_string(), state_of(&view))
        })
        .collect())
}

/// Makes the published entities of `kind` exactly `states`, leaving the ids in `kept` as
/// they are. Returns whether anything changed.
async fn replace(
    write: &WriteTransaction<'_>,
    kind: &str,
    states: &States,
    kept: &BTreeSet<String>,
) -> Result<bool, cockpit_messaging::Error> {
    let mut has_changed = false;
    for (id, state) in states {
        let published = cockpit_messaging::publish(write, kind, id, state).await?;
        has_changed |= matches!(published, cockpit_messaging::Published::Changed { .. });
    }
    for id in cockpit_messaging::published_ids(write, kind).await? {
        if !states.contains_key(&id) && !kept.contains(&id) {
            has_changed |= cockpit_messaging::retract(write, kind, &id)
                .await?
                .is_some();
        }
    }
    Ok(has_changed)
}

fn state_of(view: &impl serde::Serialize) -> Map<String, Value> {
    match serde_json::to_value(view) {
        Ok(Value::Object(state)) => state,
        other => panic!("a wire view serializes to a JSON object, not {other:?}"),
    }
}
