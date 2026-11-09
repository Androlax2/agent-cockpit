//! Runs `cockpit-scheduling`'s pass (#43): every `interval`, on `cockpit queue dispatch` and
//! once the owner pauses or resumes the queue,
//! over the sessions as this daemon sees them, then types each placement to its session's
//! agent (#385), and answers `cockpit queue explain` from the records the last pass wrote.
//! This file gathers the inputs, delivers and maps to the wire; the rules live in the
//! scheduling crate.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::sync::Arc;

use cockpit_messaging::{Claim, Deliverer, Delivery, DeliveryState, Failure};
use cockpit_protocol::{
    Command, ErrorCode, ProtocolError, QueueExplanationView, QueuePlacementView,
    QueueSessionNoteView, QueueStepView, Reply,
};
use cockpit_provider_api::Capabilities;
use cockpit_resource_admission::Backend;
use cockpit_scheduling::{
    Candidate, Dispatched, Explanation, Inputs, Placement, Presence, ProjectShare, Standing, Unit,
    UsageWindow,
};
use cockpit_sessions::{AgentState, Session, SessionReport, SessionState, Sessions};
use cockpit_turso::Database;
use cockpit_usage::{AccountQuota, QuotaPolicy, WindowQuota, WindowState};

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
    /// How the recorded plan windows that pace queue starts are judged (#412); `None` when
    /// this daemon reads no quota, which paces nothing.
    pub(crate) quota_policy: Option<QuotaPolicy>,
    /// The owner's per-project caps on each plan window, in percent (#412).
    pub(crate) project_shares: BTreeMap<String, u8>,
    /// Wakes the periodic pass early: `queue explain` answers from the last pass's records,
    /// which a pause or resume makes wrong until the next one (#646).
    pub(crate) queue_changed: Arc<tokio::sync::Notify>,
}

/// The route for a queue command; `routes.rs` sends only the owner's here. Both are small
/// and the owner's, so they run in the control lane.
pub(crate) fn route(command: Command, context: Context) -> Route {
    let work = async move {
        match command {
            Command::TaskHandoff {
                item,
                intent_revision,
                state_revision,
                session,
                expected_launch,
            } => {
                handoff(
                    &context,
                    item,
                    intent_revision,
                    state_revision,
                    &session,
                    expected_launch,
                )
                .await
            }
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

/// Runs a pass every `settings.interval` and whenever the queue is paused or resumed, for as
/// long as the daemon runs. A failed pass is logged and the next one tries again.
pub(crate) async fn dispatch_forever(context: Context) {
    let mut ticks = tokio::time::interval(context.settings.interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            () = context.queue_changed.notified() => {}
        }
        if let Err(Failed(error)) = pass(&context).await {
            tracing::error!(error = %error.message, "the scheduling pass failed");
        }
    }
}

/// One pass: places what can start, ends what ended and rewrites the explanations, in one
/// write transaction.
async fn pass(context: &Context) -> Result<Dispatched, Failed> {
    let inputs = inputs(context).await?;
    let policy = context.admission.policy();
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
    // The placements are committed whatever delivery does: one that is not delivered now is
    // retried by the next pass, or ends `delivery-failed` and its work goes back.
    if let Err(error) = deliver(context).await {
        tracing::error!(%error, "delivering placed work to the agents failed");
    }
    Ok(dispatched)
}

/// The outbox deliveries a pass makes at most; the rest wait for the next pass.
const DELIVERIES_PER_PASS: usize = 64;

/// Delivers scheduling's due envelopes, then ends the placement of each delivery that failed
/// for good, so its work goes back to the queue. One this misses (an error, a crash) the next
/// pass ends from the outbox's state.
async fn deliver(context: &Context) -> Result<(), DeliveryError> {
    let delivered = cockpit_messaging::deliver_due(
        &context.database,
        cockpit_scheduling::DELIVERY_PRODUCER,
        &Typist { context },
        now(),
        DELIVERIES_PER_PASS,
    )
    .await?;
    for (id, state) in delivered {
        let read = context.database.read().await?;
        let Some(envelope) = cockpit_messaging::envelope(&read, id).await? else {
            continue;
        };
        drop(read);
        let Some(placement) = cockpit_scheduling::placement_of_delivery(&envelope.effect_key)
        else {
            continue;
        };
        let reason = envelope.note.as_deref();
        match state {
            DeliveryState::Received => tracing::info!(
                placement = placement.0,
                session = envelope.destination.as_str(),
                "delivered placed work to its agent"
            ),
            // Expiry happens while claiming, so it never comes back from `deliver_due`; the
            // pass ends an expired delivery's placement.
            DeliveryState::Failed => {
                let write = context.database.write().await?;
                let ended = cockpit_scheduling::delivery_failed(&write, placement, now()).await?;
                write.commit().await?;
                if ended {
                    tracing::warn!(
                        placement = placement.0,
                        session = envelope.destination.as_str(),
                        reason,
                        "placed work could not be delivered; it goes back to the queue"
                    );
                }
            }
            DeliveryState::Uncertain => tracing::warn!(
                placement = placement.0,
                session = envelope.destination.as_str(),
                reason,
                "placed work may or may not have reached its agent; the placement stays"
            ),
            DeliveryState::Queued
            | DeliveryState::Submitting
            | DeliveryState::Expired
            | DeliveryState::Superseded => {}
        }
    }
    Ok(())
}

/// Types a placement's line to its session's agent.
struct Typist<'context> {
    context: &'context Context,
}

impl Deliverer for Typist<'_> {
    fn deliver(&self, claim: &Claim) -> impl Future<Output = Delivery> + Send {
        let context = self.context;
        async move {
            let rejected = |reason: String| Delivery::Failed(Failure::Rejected { reason });
            let Some(placement) = cockpit_scheduling::placement_of_delivery(&claim.effect_key)
            else {
                return rejected(format!("{:?} names no placement", claim.effect_key));
            };
            match is_active(context, placement).await {
                Ok(true) => {}
                Ok(false) => return rejected("its placement ended before delivery".to_owned()),
                Err(error) => {
                    return Delivery::Failed(Failure::NotSent {
                        reason: error.to_string(),
                        retry_at: retry_at(),
                    });
                }
            }
            let session = match cockpit_sessions::SessionId::parse(&claim.destination) {
                Ok(session) => session,
                Err(error) => return rejected(error.to_string()),
            };
            match context
                .sessions
                .submit(&session, &claim.payload, cockpit_sessions::Addressed::Work)
                .await
            {
                Ok(()) => Delivery::Received {
                    receipt: format!("delivered to session {session}"),
                },
                Err(
                    error @ (cockpit_sessions::Error::TypingUnconfirmed { .. }
                    | cockpit_sessions::Error::DeliveryUnconfirmed { .. }),
                ) => Delivery::Failed(Failure::Unacknowledged {
                    reason: error.to_string(),
                    retry_at: retry_at(),
                }),
                Err(error @ cockpit_sessions::Error::UnknownSession { .. }) => {
                    rejected(error.to_string())
                }
                // Another operation holds the session for a moment, such as the periodic
                // session pass: that is no failed try (#816).
                Err(error @ cockpit_sessions::Error::SessionBusy { .. }) => {
                    Delivery::Failed(Failure::Busy {
                        reason: error.to_string(),
                        retry_at: retry_at(),
                    })
                }
                // Nothing was sent: the agent was busy with a dialog, the session was
                // between launches, tmux did not answer. A later pass tries again.
                Err(error) => Delivery::Failed(Failure::NotSent {
                    reason: error.to_string(),
                    retry_at: retry_at(),
                }),
            }
        }
    }
}

async fn is_active(
    context: &Context,
    placement: cockpit_scheduling::PlacementId,
) -> Result<bool, DeliveryError> {
    let read = context.database.read().await?;
    Ok(cockpit_scheduling::active_placement(&read, placement)
        .await?
        .is_some())
}

/// Due at once: delivery runs only after a pass, so the next pass tries it again.
fn retry_at() -> i64 {
    now()
}

/// What stopped delivery; logged, since the pass it follows has committed.
#[derive(Debug)]
enum DeliveryError {
    Database(cockpit_turso::Error),
    Messaging(cockpit_messaging::Error),
    Scheduling(cockpit_scheduling::Error),
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeliveryError::Database(error) => write!(formatter, "database: {error}"),
            DeliveryError::Messaging(error) => write!(formatter, "messaging: {error}"),
            DeliveryError::Scheduling(error) => write!(formatter, "scheduling: {error}"),
        }
    }
}

impl From<cockpit_turso::Error> for DeliveryError {
    fn from(error: cockpit_turso::Error) -> DeliveryError {
        DeliveryError::Database(error)
    }
}

impl From<cockpit_messaging::Error> for DeliveryError {
    fn from(error: cockpit_messaging::Error) -> DeliveryError {
        DeliveryError::Messaging(error)
    }
}

impl From<cockpit_scheduling::Error> for DeliveryError {
    fn from(error: cockpit_scheduling::Error) -> DeliveryError {
        DeliveryError::Scheduling(error)
    }
}

/// The sessions that could take work: every session not ended, with its project, provider
/// capabilities and what its current launch's agent last reported.
async fn inputs(context: &Context) -> Result<Inputs, Failed> {
    let read = context.database.read().await?;
    inputs_in(context, &read).await
}

async fn inputs_in(
    context: &Context,
    read: &cockpit_turso::sql::Connection,
) -> Result<Inputs, Failed> {
    let sessions = cockpit_sessions::sessions(read, false).await?;
    let reports: HashMap<String, SessionReport> = cockpit_sessions::latest_reports(read)
        .await?
        .into_iter()
        .map(|report| (report.session.as_str().to_owned(), report))
        .collect();
    let projects: HashMap<cockpit_projects::CheckoutId, String> =
        cockpit_projects::checkouts(read, None)
            .await
            .map_err(|error| internal(&error))?
            .into_iter()
            .map(|checkout| (checkout.id, checkout.project.as_str().to_owned()))
            .collect();
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
                provider: session.provider.as_str().to_owned(),
                account: account_of(session),
            })
        })
        .collect::<Result<_, Failed>>()?;
    let now = now();
    Ok(Inputs {
        candidates,
        viewed: context
            .manager
            .viewed()
            .map(|session| cockpit_authorization::SessionId::new(session.as_str()))
            .transpose()
            .map_err(|error| internal(&error))?,
        usage: usage(context, read).await?,
        agent_seconds: agent_seconds(read, now).await?,
        now,
    })
}

/// The agent seconds each open item with a budget has had: its placements' sessions' turns
/// while each placement held, until `now` for one that still does (#412).
async fn agent_seconds(
    read: &cockpit_turso::sql::Connection,
    now: i64,
) -> Result<BTreeMap<cockpit_tasks::ItemId, u64>, Failed> {
    let items = cockpit_tasks::list(
        read,
        &cockpit_tasks::Actor::Owner,
        &cockpit_tasks::ListFilter::default(),
    )
    .await
    .map_err(|error| internal(&error))?;
    let mut spent = BTreeMap::new();
    for item in items.iter().filter(|item| item.budget_minutes.is_some()) {
        let spans = cockpit_scheduling::work_spans(read, item.id)
            .await
            .map_err(|error| internal(&error))?;
        let mut seconds: u64 = 0;
        for span in spans {
            let worked = cockpit_usage::turn_seconds(
                read,
                span.session.as_str(),
                span.from,
                span.until.unwrap_or(now),
            )
            .await
            .map_err(|error| internal(&error))?;
            seconds = seconds.saturating_add(worked);
        }
        spent.insert(item.id, seconds);
    }
    Ok(spent)
}

/// The account whose windows pace `session`'s queue starts: its launch's, or for an agent
/// launched on no account, the owner's own sign-in, whose windows the daemon reads as
/// `default` (#58). `None` without a launch: nothing says where it runs.
fn account_of(session: &Session) -> Option<String> {
    let launch = session.launch.as_ref()?;
    Some(
        launch
            .account
            .clone()
            .unwrap_or_else(|| cockpit_config::DEFAULT_ACCOUNT.to_owned()),
    )
}

/// Every account's recorded plan windows, judged now under the quota policy.
async fn usage(
    context: &Context,
    read: &cockpit_turso::sql::Connection,
) -> Result<Vec<UsageWindow>, Failed> {
    let Some(policy) = &context.quota_policy else {
        return Ok(Vec::new());
    };
    let quotas = cockpit_usage::quotas(read, policy, now())
        .await
        .map_err(|error| internal(&error))?;
    let mut windows = Vec::new();
    for quota in &quotas {
        for window in &quota.windows {
            let mut usage = usage_window(quota, window);
            if !context.project_shares.is_empty() {
                usage.project_shares =
                    project_shares(read, quota, &usage, &context.project_shares).await?;
            }
            windows.push(usage);
        }
    }
    Ok(windows)
}

/// Each capped project's part of `window`, estimated as the window's used share times what the
/// project's replies on the account since the window opened cost at API prices, against all
/// of the account's (#412). None for a window that caps nothing: not current, scoped to a model
/// or without a length and a reset.
async fn project_shares(
    read: &cockpit_turso::sql::Connection,
    quota: &AccountQuota,
    window: &UsageWindow,
    shares: &BTreeMap<String, u8>,
) -> Result<Vec<ProjectShare>, Failed> {
    let (Some(length), Some(resets_at)) = (window.length_seconds, window.resets_at) else {
        return Ok(Vec::new());
    };
    if window.standing != Standing::Current || window.scope.is_some() {
        return Ok(Vec::new());
    }
    let opened_at = resets_at - i64::from(length);
    let costs =
        cockpit_usage::project_costs_since(read, &quota.provider, &quota.account, opened_at)
            .await
            .map_err(|error| internal(&error))?;
    let total = costs
        .values()
        .fold(0_u64, |sum, cost| sum.saturating_add(*cost));
    Ok(shares
        .iter()
        .map(|(project, share_percent)| {
            let spent = costs.get(project).copied().unwrap_or(0);
            let used_percent = if total == 0 {
                0.0
            } else {
                window.used_percent * spent as f64 / total as f64
            };
            ProjectShare {
                project: project.clone(),
                share_percent: *share_percent,
                used_percent,
            }
        })
        .collect())
}

/// `window` of `quota`'s account as scheduling paces it.
pub(crate) fn usage_window(quota: &AccountQuota, window: &WindowQuota) -> UsageWindow {
    UsageWindow {
        provider: quota.provider.as_str().to_owned(),
        account: quota.account.as_str().to_owned(),
        limit: window.limit.as_str().to_owned(),
        scope: window.scope.clone(),
        used_percent: window.used_percent,
        length_seconds: window.length_seconds,
        resets_at: window.resets_at,
        standing: match window.state {
            WindowState::Available | WindowState::Low | WindowState::Exhausted => Standing::Current,
            WindowState::Blocked => Standing::Blocked,
            WindowState::Stale => Standing::Stale,
        },
        project_shares: Vec::new(),
    }
}

/// The owner's `[usage]` pacing as scheduling applies it.
pub(crate) fn pacing(usage: &cockpit_config::UsageSettings) -> cockpit_scheduling::Pacing {
    match usage.pacing.value {
        cockpit_config::Pacing::Off => cockpit_scheduling::Pacing::Off,
        cockpit_config::Pacing::Even => cockpit_scheduling::Pacing::Even {
            reserve_percent: usage.pacing_reserve_percent.value,
            burst_percent: usage.pacing_burst_percent.value,
        },
    }
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
            let awaits_pass = cockpit_scheduling::awaits_pass(&read, cockpit_tasks::ItemId(item))
                .await
                .map_err(|error| internal(&error))?;
            let message = if awaits_pass {
                format!(
                    "item {item} has no explanation yet: no pass has considered it since it \
                     began waiting; the next one runs within {} seconds, or now with `torlek \
                     queue dispatch`",
                    context.settings.interval.as_secs()
                )
            } else {
                format!(
                    "item {item} has no explanation: the last pass found it placed, being \
                     done, finished or gone; `torlek todo show {item}` says which"
                )
            };
            return Err(Failed(ProtocolError::new(ErrorCode::NotFound, message)));
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

mod handoff;
use handoff::handoff;
