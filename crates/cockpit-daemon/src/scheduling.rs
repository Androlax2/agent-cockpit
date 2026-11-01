//! Runs `cockpit-scheduling`'s pass (#43): every `interval` and on `cockpit queue dispatch`,
//! over the sessions as this daemon sees them, and answers `cockpit queue explain` from the
//! records the last pass wrote. This file gathers the inputs and maps to the wire; the rules
//! live in the scheduling crate.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use cockpit_protocol::{
    Command, ErrorCode, ProtocolError, QueueExplanationView, QueuePlacementView,
    QueueSessionNoteView, QueueStepView, Reply,
};
use cockpit_provider_api::Capabilities;
use cockpit_resource_admission::{Backend, Policy};
use cockpit_scheduling::{Candidate, Dispatched, Explanation, Inputs, Placement, Presence, Unit};
use cockpit_sessions::{AgentState, Session, SessionReport, SessionState, Sessions};
use cockpit_turso::Database;

use crate::Manager;
use crate::approvals::{Answer, Failed, job, now};
use crate::resources::demand_view;
use crate::routes::{Route, WorkClass};

/// What a pass reads besides the state database.
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) database: Arc<Database>,
    pub(crate) sessions: Arc<Sessions>,
    pub(crate) admission: Arc<Backend>,
    /// Remembers the session the owner views, which gets no new work.
    pub(crate) manager: Arc<Manager>,
    pub(crate) provider_capabilities: Arc<BTreeMap<String, Capabilities>>,
    pub(crate) settings: cockpit_scheduling::Settings,
}

/// The route for a queue command; `routes.rs` sends only the owner's here. Both are small
/// and the owner's, so they run in the control lane.
pub(crate) fn route(command: Command, context: Context) -> Route {
    let work = async move {
        match command {
            Command::QueueExplain { item } => explain(&context, item).await,
            Command::QueueDispatch => dispatched_reply(pass(&context).await?),
            other => unreachable!("routes.rs sends only queue commands here, not {other:?}"),
        }
    };
    Route::Run {
        class: WorkClass::Control,
        job: job(work),
    }
}

/// Runs a pass every `settings.interval`, for as long as the daemon runs. A failed pass is
/// logged and the next one tries again.
pub(crate) async fn dispatch_forever(context: Context) {
    let mut ticks = tokio::time::interval(context.settings.interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        if let Err(Failed(error)) = pass(&context).await {
            tracing::error!(error = %error.message, "the scheduling pass failed");
        }
    }
}

/// One pass: places what can start, ends what ended and rewrites the explanations, in one
/// write transaction.
async fn pass(context: &Context) -> Result<Dispatched, Failed> {
    let inputs = inputs(context).await?;
    let policy = Policy::for_budget(context.admission.capacity());
    let write = context.database.write().await?;
    let dispatched = cockpit_scheduling::dispatch(
        &write,
        &inputs,
        &context.settings,
        &policy,
        &context.admission.support(),
    )
    .await
    .map_err(|error| internal(&error))?;
    write.commit().await?;
    for placement in &dispatched.placed {
        tracing::info!(
            placement = placement.id.0,
            item = placement.item.0,
            session = placement.session.as_str(),
            "placed waiting work on a session"
        );
    }
    Ok(dispatched)
}

/// The sessions that could take work: every session not ended, with its project, provider
/// capabilities and what its current launch's agent last reported.
async fn inputs(context: &Context) -> Result<Inputs, Failed> {
    let sessions = context.sessions.list(false).await?;
    let read = context.database.read().await?;
    let reports: HashMap<String, SessionReport> = cockpit_sessions::latest_reports(&read)
        .await?
        .into_iter()
        .map(|report| (report.session.as_str().to_owned(), report))
        .collect();
    let projects: HashMap<cockpit_projects::CheckoutId, String> =
        cockpit_projects::checkouts(&read, None)
            .await
            .map_err(|error| internal(&error))?
            .into_iter()
            .map(|checkout| (checkout.id, checkout.project.as_str().to_owned()))
            .collect();
    drop(read);
    let candidates = sessions
        .iter()
        .filter(|session| session.state != SessionState::Ended)
        .map(|session| {
            Ok(Candidate {
                session: cockpit_authorization::SessionId::new(session.id.as_str())
                    .map_err(|error| internal(&error))?,
                project: projects.get(&session.checkout).cloned(),
                // Roles come from cockpit-teams (#53).
                role: None,
                capabilities: context
                    .provider_capabilities
                    .get(session.provider.as_str())
                    .cloned()
                    .unwrap_or_else(Capabilities::none),
                presence: presence(session, reports.get(session.id.as_str())),
            })
        })
        .collect::<Result<_, Failed>>()?;
    Ok(Inputs {
        candidates,
        viewed: context
            .manager
            .viewed()
            .map(|session| cockpit_authorization::SessionId::new(session.as_str()))
            .transpose()
            .map_err(|error| internal(&error))?,
        now: now(),
    })
}

/// Where the session's agent is: only a report from its current launch says anything about
/// the agent running now.
fn presence(session: &Session, report: Option<&SessionReport>) -> Presence {
    let state = match session.state {
        SessionState::Running => None,
        SessionState::Launching => Some("launching"),
        SessionState::Stopping { .. } => Some("stopping"),
        SessionState::Asleep => Some("asleep"),
        SessionState::Ended => Some("ended"),
        SessionState::Failed => Some("failed"),
    };
    if let Some(state) = state {
        return Presence::Inactive {
            state: state.to_owned(),
        };
    }
    let current_launch = session.launch.as_ref().map(|launch| launch.number);
    let Some(report) =
        report.filter(|report| current_launch.is_some() && report.launch == current_launch)
    else {
        return Presence::Unreported;
    };
    if report.needs_input.is_some() {
        return Presence::NeedsInput;
    }
    match report.state {
        Some(AgentState::Working) => Presence::Working,
        Some(AgentState::Waiting) => Presence::Waiting {
            since: report.reported_at,
        },
        None => Presence::Unreported,
    }
}

async fn explain(context: &Context, item: Option<i64>) -> Answer {
    let read = context.database.read().await?;
    let mut explanations = cockpit_scheduling::explanations(&read)
        .await
        .map_err(|error| internal(&error))?;
    if let Some(item) = item {
        explanations.retain(|explanation| explanation.item.0 == item);
        if explanations.is_empty() {
            return Err(Failed(ProtocolError::new(
                ErrorCode::NotFound,
                format!(
                    "item {item} has no explanation: the last pass found it placed, being \
                     done, finished or gone; `cockpit todo show {item}` says which"
                ),
            )));
        }
    }
    Ok(Reply::QueueExplanations {
        explanations: explanations.into_iter().map(explanation_view).collect(),
    })
}

fn dispatched_reply(dispatched: Dispatched) -> Answer {
    Ok(Reply::QueueDispatched {
        placed: dispatched.placed.into_iter().map(placement_view).collect(),
        ended: dispatched
            .ended
            .iter()
            .map(|placement| placement.0)
            .collect(),
        waiting: u64::try_from(dispatched.explanations.len()).unwrap_or(u64::MAX),
    })
}

fn explanation_view(explanation: Explanation) -> QueueExplanationView {
    QueueExplanationView {
        item: explanation.item.0,
        project: explanation.project,
        title: explanation.title,
        step: step_view(&explanation.unit),
        reason: explanation.reason.as_str().to_owned(),
        why: explanation.why,
        limited_by: explanation.limited_by,
        sessions: explanation
            .sessions
            .into_iter()
            .map(|note| QueueSessionNoteView {
                session: note.session.as_str().to_owned(),
                why: note.why,
            })
            .collect(),
        behind: explanation.behind.map(|item| item.0),
        action: explanation.action,
        action_by: explanation.action_by.as_str().to_owned(),
        priority: explanation.priority,
        aging_boost: explanation.aging_boost,
        demand: demand_view(explanation.demand),
        headroom_percent: explanation.headroom_percent,
        waiting_since: explanation.waiting_since,
        evaluated_at: explanation.evaluated_at,
        next_evaluation_at: explanation.next_evaluation_at,
    }
}

fn placement_view(placement: Placement) -> QueuePlacementView {
    QueuePlacementView {
        id: placement.id.0,
        item: placement.item.0,
        project: placement.project,
        step: step_view(&placement.unit),
        session: placement.session.as_str().to_owned(),
        demand: demand_view(placement.demand),
        placed_at: placement.placed_at,
    }
}

fn step_view(unit: &Unit) -> Option<QueueStepView> {
    match unit {
        Unit::Item => None,
        Unit::Step {
            run,
            position,
            name,
        } => Some(QueueStepView {
            run: run.0,
            position: *position,
            name: (*name).to_owned(),
        }),
    }
}

fn internal(error: &dyn std::fmt::Display) -> Failed {
    tracing::error!(%error, "a scheduling request failed");
    Failed(ProtocolError::new(
        ErrorCode::Internal,
        "the daemon failed; the request may or may not have taken effect",
    ))
}
