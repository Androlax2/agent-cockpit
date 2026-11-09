//! Per-item budgets of agent minutes (#412, owner choice A). The queue holds an item past its
//! budget; this module counts the minutes and tells the owner once. When a turn of placed work
//! ends past the item's budget, a `budget-spent` entity goes on the change feed in the turn's
//! own transaction, and `torlek-notify` tells it. Nothing is typed to the agent, and a running
//! turn is never cut. Publishing the same state again changes nothing, so later turns past the
//! same budget tell nothing new; the owner's next decision on the item (a new budget, done,
//! deleted) retracts it, so the next spent budget is news again. A session's budget (#412)
//! is told the same way, through a mark on the session's own view.

use std::collections::BTreeMap;

use cockpit_protocol::TaskBudgetSpentView;
use cockpit_tasks::{Actor, Item, ItemId, ListFilter, Status};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::Connection;

const BUDGET_SPENT: &str = "budget-spent";

/// The agent seconds `item`'s sessions worked it: their turns while each placement held, until
/// `now` for one that still does.
pub(crate) async fn worked_seconds(
    connection: &Connection,
    item: ItemId,
    now: i64,
) -> Result<u64, String> {
    let spans = cockpit_scheduling::work_spans(connection, item)
        .await
        .map_err(|error| error.to_string())?;
    seconds_in(connection, &spans, now).await
}

async fn seconds_in(
    connection: &Connection,
    spans: &[cockpit_scheduling::WorkSpan],
    now: i64,
) -> Result<u64, String> {
    let mut seconds: u64 = 0;
    for span in spans {
        let worked = cockpit_usage::turn_seconds(
            connection,
            span.session.as_str(),
            span.from,
            span.until.unwrap_or(now),
        )
        .await
        .map_err(|error| error.to_string())?;
        seconds = seconds.saturating_add(worked);
    }
    Ok(seconds)
}

/// A turn the recorder just ended, of the session it is keyed by.
pub(crate) struct EndedTurn {
    pub(crate) started_at: i64,
    pub(crate) ended_at: i64,
}

/// Publishes the spent budget of each open item whose placement held during one of `ended`,
/// when its sessions worked it for its budget or longer.
pub(crate) async fn publish_spent(
    write: &WriteTransaction<'_>,
    ended: &BTreeMap<&str, EndedTurn>,
    now: i64,
) -> Result<(), String> {
    let items = cockpit_tasks::list(write, &Actor::Owner, &ListFilter::default())
        .await
        .map_err(|error| error.to_string())?;
    let budgeted = items
        .iter()
        .filter(|item| matches!(item.status, Status::Todo | Status::Doing));
    for item in budgeted {
        let Some(budget_minutes) = item.budget_minutes else {
            continue;
        };
        let spans = cockpit_scheduling::work_spans(write, item.id)
            .await
            .map_err(|error| error.to_string())?;
        let held_during_an_ended_turn = spans.iter().any(|span| {
            ended.get(span.session.as_str()).is_some_and(|turn| {
                span.from <= turn.ended_at
                    && span.until.is_none_or(|until| until >= turn.started_at)
            })
        });
        if !held_during_an_ended_turn {
            continue;
        }
        if seconds_in(write, &spans, now).await? < u64::from(budget_minutes) * 60 {
            continue;
        }
        cockpit_messaging::publish(
            write,
            BUDGET_SPENT,
            &item.id.0.to_string(),
            &crate::session_feed::state_of(&spent_view(item, budget_minutes)),
        )
        .await
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// `session`'s budget against the agent seconds its turns have had since it started (#412),
/// for the queue's hold and the owner's notice alike; `None` without a budget.
pub(crate) async fn session_budget(
    connection: &Connection,
    session: &cockpit_sessions::Session,
    now: i64,
) -> Result<Option<cockpit_scheduling::SessionBudget>, String> {
    let Some(minutes) = session.budget_minutes else {
        return Ok(None);
    };
    let worked_seconds =
        cockpit_usage::turn_seconds(connection, session.id.as_str(), session.created_at, now)
            .await
            .map_err(|error| error.to_string())?;
    Ok(Some(cockpit_scheduling::SessionBudget {
        minutes,
        worked_seconds,
    }))
}

/// Marks each open session of `ended` whose turns since its start reached its budget (#412).
/// The mark is on the session's view, which the session feed publishes after the commit, and
/// `torlek-notify` tells it once; the owner's next budget for the session clears it. A
/// session that cannot be checked is logged and the others still are.
pub(crate) async fn mark_sessions_spent(
    write: &WriteTransaction<'_>,
    ended: &BTreeMap<&str, EndedTurn>,
    now: i64,
) {
    for session in ended.keys() {
        if let Err(problem) = mark_if_spent(write, session, now).await {
            tracing::warn!(session, problem, "telling a session's spent budget failed");
        }
    }
}

async fn mark_if_spent(
    write: &WriteTransaction<'_>,
    session: &str,
    now: i64,
) -> Result<(), String> {
    let id = cockpit_sessions::SessionId::parse(session).map_err(|error| error.to_string())?;
    let session = cockpit_sessions::session(write, &id)
        .await
        .map_err(|error| error.to_string())?;
    let is_over = matches!(
        session.state,
        cockpit_sessions::SessionState::Ended | cockpit_sessions::SessionState::Failed
    );
    if session.is_budget_spent || is_over {
        return Ok(());
    }
    let Some(budget) = session_budget(write, &session, now).await? else {
        return Ok(());
    };
    if budget.worked_seconds < u64::from(budget.minutes) * 60 {
        return Ok(());
    }
    cockpit_sessions::mark_budget_spent(write, &id)
        .await
        .map_err(|error| error.to_string())
}

/// Takes `item`'s spent budget off the feed: the owner decided on it.
pub(crate) async fn retract_spent(
    write: &WriteTransaction<'_>,
    item: ItemId,
) -> Result<(), cockpit_messaging::Error> {
    cockpit_messaging::retract(write, BUDGET_SPENT, &item.0.to_string()).await?;
    Ok(())
}

fn spent_view(item: &Item, budget_minutes: u32) -> TaskBudgetSpentView {
    TaskBudgetSpentView {
        item: item.id.0,
        project: item.project.clone(),
        title: item.title.clone(),
        budget_minutes,
    }
}
