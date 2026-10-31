//! Sessions and checkouts on the change feed (#199): after each commit the daemon publishes
//! their current wire views and retracts the ones gone, so clients follow the feed instead of
//! polling the lists. The daemon is the only producer of the `session` and `checkout` kinds.
//!
//! The views are published in a transaction of their own right after the commit that changed
//! them, not inside it (`docs/decisions/2026-10-31-199-change-feed-projector.md`). One
//! task publishes, in order, so a view never goes back to an older state; a pass that changes
//! nothing rolls back, so it does not wake itself.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use cockpit_sessions::Sessions;
use cockpit_turso::{Database, WriteTransaction};
use serde_json::{Map, Value};

const SESSION: &str = "session";
const CHECKOUT: &str = "checkout";
/// How long a pass that failed waits before it runs again, commit or not.
const RETRY: Duration = Duration::from_secs(1);

/// Published states by entity id.
type States = BTreeMap<String, Map<String, Value>>;

/// Publishes once at start, then after every commit, until the database is dropped.
pub(crate) async fn publish_forever(
    database: Arc<Database>,
    sessions: Arc<Sessions>,
    environment: cockpit_config::Environment,
) {
    // Watched before the first pass, so a commit during it runs another.
    let mut commits = database.watch_commits();
    loop {
        if let Err(problem) = publish(&database, &sessions, &environment).await {
            tracing::warn!(
                problem,
                "publishing sessions and checkouts failed; retrying"
            );
            tokio::time::sleep(RETRY).await;
            continue;
        }
        if commits.changed().await.is_err() {
            return;
        }
    }
}

async fn publish(
    database: &Database,
    sessions: &Sessions,
    environment: &cockpit_config::Environment,
) -> Result<(), String> {
    let sessions = session_states(sessions)
        .await
        .map_err(|error| format!("cannot read the sessions: {error}"))?;
    // A broken owner configuration leaves the published checkouts as they are rather than
    // publishing every checkout untrusted; the next commit tries again.
    let checkouts = match checkout_states(database, environment).await {
        Ok(checkouts) => Some(checkouts),
        Err(error) => {
            tracing::warn!(%error, "the checkouts on the change feed are left unchanged");
            None
        }
    };
    let write = database.write().await.map_err(|error| error.to_string())?;
    let mut has_changed = replace(&write, SESSION, &sessions)
        .await
        .map_err(|error| error.to_string())?;
    if let Some(checkouts) = &checkouts {
        has_changed |= replace(&write, CHECKOUT, checkouts)
            .await
            .map_err(|error| error.to_string())?;
    }
    if has_changed {
        write.commit().await
    } else {
        write.rollback().await
    }
    .map_err(|error| error.to_string())
}

/// The sessions not ended, as `session-list` answers them; an ended one is retracted.
async fn session_states(sessions: &Sessions) -> Result<States, cockpit_sessions::Error> {
    let mut states = States::new();
    for session in sessions.list(false).await? {
        let view = crate::sessions::current_view(sessions, &session).await?;
        states.insert(view.id.clone(), state_of(&view));
    }
    Ok(states)
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

/// Makes the published entities of `kind` exactly `states`. Returns whether anything changed.
async fn replace(
    write: &WriteTransaction<'_>,
    kind: &str,
    states: &States,
) -> Result<bool, cockpit_messaging::Error> {
    let mut has_changed = false;
    for (id, state) in states {
        let published = cockpit_messaging::publish(write, kind, id, state).await?;
        has_changed |= matches!(published, cockpit_messaging::Published::Changed { .. });
    }
    for id in cockpit_messaging::published_ids(write, kind).await? {
        if !states.contains_key(&id) {
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
