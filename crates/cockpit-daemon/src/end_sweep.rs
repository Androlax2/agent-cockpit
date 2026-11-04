//! Sweeps a clone once every session that worked in it ended (#767): deletes it when the
//! sweep's rules allow, otherwise tries to prune only its `target/` under the owner's prune
//! policy, and records which and why for `torlek storage`. `storage.sweep_on_session_end`
//! turns it off; `torlek checkout sweep` stays for the owner either way.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cockpit_projects::{Checkout, CheckoutKind, Kept};
use cockpit_protocol::{ErrorCode, Outcome, ProtocolError, Reply};
use cockpit_sessions::SessionState;
use cockpit_turso::sql::Connection;

use crate::routes::WorkClass;
use crate::storage::Context;

/// The most clones one pass sweeps; the next pass takes the rest.
const CLONES_PER_PASS: usize = 16;

/// A clone whose sessions all ended, not swept since the latest of them did.
struct EndedClone {
    checkout: Checkout,
    /// The latest session that worked in it, and the number of its latest launch (0 for none):
    /// a resumed session's next end sweeps again.
    session: String,
    session_launch: i64,
}

/// Looks for ended sessions' clones every `interval`, until aborted. Without the owner's
/// setting it does nothing.
pub(crate) async fn sweep_forever(
    context: Context,
    shared: Arc<crate::server::Shared>,
    interval: Duration,
) {
    if !context.sweeps_on_session_end {
        std::future::pending::<()>().await;
        return;
    }
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        if let Err(error) = pass(&context, &shared).await {
            tracing::warn!(error = %error.message, "the session-end clone sweep was deferred");
        }
    }
}

/// Sweeps each ended session's clone as Bulk work, as the owner's requests are. A busy lane
/// ends the pass; the clone is swept by a later one.
async fn pass(context: &Context, shared: &crate::server::Shared) -> Result<(), ProtocolError> {
    let ended = {
        let read = context.database.read().await.map_err(internal)?;
        ended_clones(&read).await?
    };
    for clone in ended {
        if context.stopping.load(Ordering::Acquire) {
            return Ok(());
        }
        let worker = context.clone();
        let work = Box::pin(async move {
            match sweep_and_record(&worker, clone).await {
                Ok(()) => Outcome::Ok(Reply::Pong),
                Err(error) => Outcome::Error(error),
            }
        });
        let admitted = shared
            .lanes
            .admit(WorkClass::Bulk, "clone-end-sweep".into(), work);
        let outcome = match admitted {
            Ok(work) => work.await,
            Err(outcome) => outcome,
        };
        if let Outcome::Error(error) = outcome {
            return Err(error);
        }
    }
    Ok(())
}

/// Clones nobody occupies whose sessions all ended, with each session's agent observed
/// exited, and that the end of the latest of them has not swept yet. A main checkout is
/// shared and never swept.
async fn ended_clones(read: &Connection) -> Result<Vec<EndedClone>, ProtocolError> {
    let checkouts = cockpit_projects::checkouts(read, None)
        .await
        .map_err(internal)?;
    let mut ended = Vec::new();
    for checkout in checkouts {
        if checkout.kind == CheckoutKind::Main || checkout.occupancy.is_some() {
            continue;
        }
        let worked_in = cockpit_sessions::checkout_sessions(read, checkout.id)
            .await
            .map_err(internal)?;
        let has_ended = worked_in.iter().any(|session| session.holds_checkout)
            && worked_in
                .iter()
                .all(|session| session.state == SessionState::Ended);
        if !has_ended {
            continue;
        }
        let launches: Vec<_> = worked_in
            .iter()
            .filter_map(|session| session.launch.as_ref().map(|launch| launch.execution))
            .collect();
        if !cockpit_processes::observed_ended(read, &launches)
            .await
            .map_err(internal)?
        {
            continue;
        }
        let Some(latest) = worked_in
            .iter()
            .max_by_key(|session| (session.changed_at, session.id.as_str()))
        else {
            continue;
        };
        let is_swept = cockpit_projects::end_sweep_recorded(
            read,
            checkout.id,
            checkout.incarnation,
            latest.id.as_str(),
            launch_number(latest),
        )
        .await
        .map_err(internal)?;
        if is_swept {
            continue;
        }
        ended.push(EndedClone {
            session: latest.id.as_str().to_owned(),
            session_launch: launch_number(latest),
            checkout,
        });
        if ended.len() == CLONES_PER_PASS {
            break;
        }
    }
    Ok(ended)
}

/// Sweeps the clone, prunes the `target/` of one kept only for uncommitted changes, and
/// records the outcome. A database failure records nothing, so a later pass sweeps again; any
/// other failed sweep is recorded, so it is not retried every pass: the owner's
/// `torlek checkout sweep` can.
async fn sweep_and_record(context: &Context, clone: EndedClone) -> Result<(), ProtocolError> {
    let swept =
        cockpit_projects::sweep_clone(&context.database, &context.git, clone.checkout.id).await;
    if let Err(
        error @ (cockpit_projects::Error::Database(_)
        | cockpit_projects::Error::Sql(_)
        | cockpit_projects::Error::Blocking(_)),
    ) = &swept
    {
        return Err(internal(error));
    }
    let (checkout, kept, target) = match swept {
        Ok((checkout, None)) => {
            tracing::info!(
                checkout = checkout.id.0,
                session = clone.session,
                path = %checkout.path.display(),
                "a clone was removed at its session's end"
            );
            (checkout, None, None)
        }
        Ok((checkout, Some(kept))) => {
            let target = match kept {
                // The #758 rule: a clone with unpushed commits keeps its `target/` too.
                Kept::UncommittedChanges => Some(
                    match crate::storage::automatic_one(context, checkout.id).await {
                        Ok(prune) => prune.to_string(),
                        // Busy, say: the periodic automatic prune or the owner's
                        // `torlek storage prune` can still take it.
                        Err(error) => format!("not pruned now: {}", error.message),
                    },
                ),
                _ => None,
            };
            let reason = kept.to_string();
            tracing::info!(checkout = checkout.id.0, session = clone.session, %reason, ?target,
                "a clone was kept at its session's end");
            (checkout, Some(reason), target)
        }
        Err(error) => {
            tracing::warn!(checkout = clone.checkout.id.0, %error,
                "the sweep of a clone at its session's end failed");
            (
                clone.checkout,
                Some(format!("the sweep failed: {error}")),
                None,
            )
        }
    };
    let write = context.database.write().await.map_err(internal)?;
    cockpit_projects::record_end_sweep(
        &write,
        &checkout,
        &clone.session,
        clone.session_launch,
        kept.as_deref(),
        target.as_deref(),
    )
    .await
    .map_err(internal)?;
    write.commit().await.map_err(internal)?;
    Ok(())
}

fn launch_number(session: &cockpit_sessions::Session) -> i64 {
    session.launch.as_ref().map_or(0, |launch| launch.number)
}

fn internal(error: impl std::fmt::Display) -> ProtocolError {
    ProtocolError::new(ErrorCode::Internal, error.to_string())
}
