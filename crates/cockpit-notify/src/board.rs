//! What the notifier knows of the daemon's state: the sessions from the change feed, and the
//! agents' reports, the approval queue and the network requests the daemon answers by request.
//! From them it derives what each session needs the owner for now. The checkouts on the feed
//! name each session's project for the status file. The host's own events come from the feed
//! too: the low disk, the plan windows' quota alerts and the items' spent budgets.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use cockpit_protocol::{
    Change, ChangeBatch, DiskView, EntityKind, FeedPosition, GuideStateView, GuideView,
    NetworkRequestStateView, NetworkRequestView, NetworkSubjectView, QuestionSource,
    QuotaAlertView, RunRequestView, RunStateView, SessionReport, SessionStateView, SessionView,
    TaskBudgetSpentView, TurnOutcome, ViewArea, ViewStamp,
};

use crate::event::Event;

/// The "session" of an event that is the host's, not a session's: the low disk; with `host:`
/// and the window's `quota-alert` id, a quota alert, so each window's alert is held and still
/// raised apart; with `host:budget/` and the item's id, a spent budget; with `host:network/`
/// and the request's id, a check run's network request, which no session waits on (#352). A
/// session id is `s` and twelve hex digits, so no session can have any of them.
pub(crate) const HOST: &str = "host";

/// One reason a session needs the owner. A session raises it while its cause stands; the
/// owner is told when it is raised, never while it stays.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Raised {
    pub session: String,
    pub event: Event,
    pub cause: Cause,
}

/// What raised an event. Two causes of one event differ, so a second approval while the first
/// is open, or a new question while the old one stands, is news too.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    /// The session's own state or its turn's outcome.
    State,
    Question(String),
    Run(i64),
    Input(i64),
    Guide(i64),
    Network(i64),
    /// The free disk space is below the floor.
    Disk,
    /// A plan window's alert: the `quota-alert` entity id with what it last fired. Each firing
    /// is a new cause, so a higher threshold or the next window's alert is news too.
    Usage {
        window: String,
        threshold: u8,
        fired_at: i64,
    },
    /// An item's spent budget: the budget is part of it, so a raised budget spent again is
    /// news, and a new title is not.
    Budget {
        item: i64,
        budget_minutes: u32,
    },
    /// The session's own spent budget (#412), the budget part of it as for an item's.
    SessionBudget {
        budget_minutes: u32,
    },
}

/// A change the notifier cannot read: the batch is refused whole, never applied in part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedError {
    pub entity: String,
    pub reason: String,
}

impl fmt::Display for FeedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot read the change to {}: {}",
            self.entity, self.reason
        )
    }
}

impl std::error::Error for FeedError {}

#[derive(Debug, Default, Clone)]
pub(crate) struct Board {
    sessions: BTreeMap<String, SessionView>,
    /// Each checkout's project, by checkout id.
    projects: BTreeMap<i64, String>,
    reports: BTreeMap<String, SessionReport>,
    /// The guard's reading while it finds the disk low (#1143); none while the entity is not
    /// on the feed.
    disk: Option<DiskView>,
    /// The last alert of each plan window, by `quota-alert` entity id (#58).
    quota_alerts: BTreeMap<String, QuotaAlertView>,
    /// Each item's spent budget, by item id (#412).
    spent_budgets: BTreeMap<i64, TaskBudgetSpentView>,
    runs: Vec<RunRequestView>,
    guides: Vec<GuideView>,
    network: Vec<NetworkRequestView>,
    /// The last stamp the feed gave each area the daemon answers by request (#706).
    stamps: BTreeMap<ViewArea, ViewStamp>,
    stale_areas: BTreeSet<ViewArea>,
}

enum Edit {
    PutSession(Box<SessionView>),
    RemoveSession(String),
    PutCheckout(i64, String),
    RemoveCheckout(i64),
    PutStamp(ViewArea, ViewStamp),
    PutDisk(DiskView),
    RemoveDisk,
    PutQuotaAlert(String, Box<QuotaAlertView>),
    RemoveQuotaAlert(String),
    PutSpentBudget(Box<TaskBudgetSpentView>),
    RemoveSpentBudget(i64),
}

impl Board {
    /// Applies `batch` whole: sessions, checkouts, the disk, quota alerts and view stamps change,
    /// other kinds are not read. A snapshot replaces all but the view stamps.
    pub(crate) fn apply(&mut self, batch: &ChangeBatch) -> Result<(), FeedError> {
        // Every change is decoded before any is applied: only decoding can fail.
        let mut edits = Vec::with_capacity(batch.changes.len());
        for event in &batch.changes {
            let refused = |reason: String| FeedError {
                entity: format!("{:?} {}", event.entity.kind, event.entity.id),
                reason,
            };
            edits.push(match (&event.entity.kind, &event.change) {
                (EntityKind::Session, Change::Upsert { state }) => {
                    Edit::PutSession(Box::new(decode(state).map_err(refused)?))
                }
                (EntityKind::Session, Change::Tombstone) => {
                    Edit::RemoveSession(event.entity.id.clone())
                }
                (EntityKind::Checkout, Change::Upsert { state }) => {
                    let checkout: CheckoutProject = decode(state).map_err(refused)?;
                    Edit::PutCheckout(checkout.id, checkout.project)
                }
                (EntityKind::Checkout, Change::Tombstone) => Edit::RemoveCheckout(
                    event
                        .entity
                        .id
                        .parse()
                        .map_err(|_| refused("a checkout id is a number".to_owned()))?,
                ),
                (EntityKind::Disk, Change::Upsert { state }) => {
                    Edit::PutDisk(decode(state).map_err(refused)?)
                }
                (EntityKind::Disk, Change::Tombstone) => Edit::RemoveDisk,
                (EntityKind::QuotaAlert, Change::Upsert { state }) => Edit::PutQuotaAlert(
                    event.entity.id.clone(),
                    Box::new(decode(state).map_err(refused)?),
                ),
                (EntityKind::QuotaAlert, Change::Tombstone) => {
                    Edit::RemoveQuotaAlert(event.entity.id.clone())
                }
                (EntityKind::BudgetSpent, Change::Upsert { state }) => {
                    Edit::PutSpentBudget(Box::new(decode(state).map_err(refused)?))
                }
                (EntityKind::BudgetSpent, Change::Tombstone) => Edit::RemoveSpentBudget(
                    event
                        .entity
                        .id
                        .parse()
                        .map_err(|_| refused("an item id is a number".to_owned()))?,
                ),
                (EntityKind::ViewStamp, Change::Upsert { state }) => {
                    // An area this version does not know has no view here to ask for.
                    let Some(area) = ViewArea::from_id(&event.entity.id) else {
                        continue;
                    };
                    Edit::PutStamp(area, decode(state).map_err(refused)?)
                }
                _ => continue,
            });
        }
        if batch.follows == FeedPosition::Snapshot {
            self.sessions.clear();
            self.projects.clear();
            self.disk = None;
            self.quota_alerts.clear();
            self.spent_budgets.clear();
        }
        for edit in edits {
            match edit {
                Edit::PutSession(session) => {
                    self.sessions.insert(session.id.clone(), *session);
                }
                Edit::RemoveSession(id) => {
                    self.sessions.remove(&id);
                }
                Edit::PutCheckout(id, project) => {
                    self.projects.insert(id, project);
                }
                Edit::RemoveCheckout(id) => {
                    self.projects.remove(&id);
                }
                Edit::PutDisk(disk) => self.disk = Some(disk),
                Edit::RemoveDisk => self.disk = None,
                Edit::PutQuotaAlert(id, alert) => {
                    self.quota_alerts.insert(id, *alert);
                }
                Edit::RemoveQuotaAlert(id) => {
                    self.quota_alerts.remove(&id);
                }
                Edit::PutSpentBudget(spent) => {
                    self.spent_budgets.insert(spent.item, *spent);
                }
                Edit::RemoveSpentBudget(item) => {
                    self.spent_budgets.remove(&item);
                }
                Edit::PutStamp(area, stamp) => {
                    if self.stamps.insert(area, stamp) != Some(stamp) {
                        self.stale_areas.insert(area);
                    }
                }
            }
        }
        Ok(())
    }

    /// The areas whose stamp moved since the last call.
    pub(crate) fn take_stale_areas(&mut self) -> BTreeSet<ViewArea> {
        std::mem::take(&mut self.stale_areas)
    }

    pub(crate) fn set_reports(&mut self, reports: Vec<SessionReport>) {
        self.reports = reports
            .into_iter()
            .map(|report| (report.session.clone(), report))
            .collect();
    }

    pub(crate) fn set_queue(&mut self, runs: Vec<RunRequestView>, guides: Vec<GuideView>) {
        self.runs = runs;
        self.guides = guides;
    }

    pub(crate) fn set_network(&mut self, requests: Vec<NetworkRequestView>) {
        self.network = requests;
    }

    /// Everything every known session needs the owner for now.
    pub(crate) fn raised(&self) -> BTreeSet<Raised> {
        let low_disk = self.disk.map(|_| Raised {
            session: HOST.to_owned(),
            event: Event::Disk,
            cause: Cause::Disk,
        });
        let quota_alerts = self.quota_alerts.iter().map(|(window, alert)| Raised {
            session: format!("{HOST}:{window}"),
            event: Event::Usage,
            cause: Cause::Usage {
                window: window.clone(),
                threshold: alert.threshold,
                fired_at: alert.fired_at,
            },
        });
        let spent_budgets = self.spent_budgets.values().map(|spent| Raised {
            session: format!("{HOST}:budget/{}", spent.item),
            event: Event::Budget,
            cause: Cause::Budget {
                item: spent.item,
                budget_minutes: spent.budget_minutes,
            },
        });
        let check_requests = self
            .network
            .iter()
            .filter(|request| {
                request.state == NetworkRequestStateView::Asking
                    && network_session(request).is_none()
            })
            .map(|request| Raised {
                session: format!("{HOST}:network/{}", request.id),
                event: Event::Hold,
                cause: Cause::Network(request.id),
            });
        self.sessions
            .values()
            .flat_map(|session| self.raised_by(session))
            .chain(low_disk)
            .chain(quota_alerts)
            .chain(spent_budgets)
            .chain(check_requests)
            .collect()
    }

    fn raised_by(&self, session: &SessionView) -> Vec<Raised> {
        let raise = |event: Event, cause: Cause| Raised {
            session: session.id.clone(),
            event,
            cause,
        };
        let report = self.reports.get(&session.id);
        let outcome = report.and_then(|report| report.outcome.as_ref());
        let mut raised = Vec::new();
        let exited_by_itself = session.state.is_over() && session.exit.is_some();
        let turn_failed = matches!(outcome, Some(TurnOutcome::Failed { .. }));
        if session.state == SessionStateView::Failed || exited_by_itself || turn_failed {
            raised.push(raise(Event::Failure, Cause::State));
        }
        if outcome == Some(&TurnOutcome::QuotaStopped) {
            raised.push(raise(Event::Quota, Cause::State));
        }
        if session.finished {
            raised.push(raise(Event::Finished, Cause::State));
        }
        if outcome == Some(&TurnOutcome::Completed) {
            raised.push(raise(Event::Turn, Cause::State));
        }
        if session.budget_spent
            && !session.state.is_over()
            && let Some(budget_minutes) = session.budget_minutes
        {
            raised.push(raise(
                Event::Budget,
                Cause::SessionBudget { budget_minutes },
            ));
        }
        // A question a coordinator holds waits on it, not on the owner (#710).
        if let Some(report) = report.filter(|report| report.held_by.is_none())
            && let Some(question) = &report.needs_input
        {
            let event = match report.question_source {
                Some(QuestionSource::Dialog) => Event::Hold,
                Some(QuestionSource::Agent) | None => Event::Question,
            };
            raised.push(raise(event, Cause::Question(question.clone())));
        }
        for run in self.runs.iter().filter(|run| run.session == session.id) {
            if run.state == RunStateView::Asking {
                raised.push(raise(Event::Hold, Cause::Run(run.id)));
            }
            if run.waiting_for_input {
                raised.push(raise(Event::Question, Cause::Input(run.id)));
            }
        }
        let guides = self
            .guides
            .iter()
            .filter(|guide| guide.session == session.id && guide.state == GuideStateView::Asking);
        for guide in guides {
            raised.push(raise(Event::Question, Cause::Guide(guide.id)));
        }
        let network = self.network.iter().filter(|request| {
            request.state == NetworkRequestStateView::Asking
                && network_session(request) == Some(session.id.as_str())
        });
        for request in network {
            raised.push(raise(Event::Hold, Cause::Network(request.id)));
        }
        raised
    }

    pub(crate) fn sessions(&self) -> impl Iterator<Item = &SessionView> {
        self.sessions.values()
    }

    pub(crate) fn report(&self, session: &str) -> Option<&SessionReport> {
        self.reports.get(session)
    }

    /// The project of the checkout `session` runs in, when the feed named it.
    pub(crate) fn project_of(&self, session: &SessionView) -> Option<&str> {
        self.projects.get(&session.checkout).map(String::as_str)
    }

    /// The name the owner knows `session` by: the label its agent reported, else its id.
    pub(crate) fn name_of(&self, session: &str) -> String {
        self.reports
            .get(session)
            .and_then(|report| report.label.clone())
            .unwrap_or_else(|| session.to_owned())
    }

    /// The name a notice of `raised` gives: what a host event is about, else
    /// [`Board::name_of`] its session.
    pub(crate) fn notice_name(&self, raised: &Raised) -> String {
        match &raised.cause {
            Cause::Disk => "Free disk space".to_owned(),
            Cause::Usage { window, .. } => self.quota_alerts.get(window).map_or_else(
                || window.clone(),
                |alert| format!("{} {} window", alert.provider, alert.limit),
            ),
            Cause::Budget { item, .. } => self
                .spent_budgets
                .get(item)
                .map_or_else(|| format!("item {item}"), |spent| spent.title.clone()),
            Cause::Network(id) => self
                .network
                .iter()
                .find(|request| request.id == *id)
                .and_then(|request| match &request.subject {
                    NetworkSubjectView::Check { check, .. } => {
                        Some(format!("check {check} of {}", request.project))
                    }
                    NetworkSubjectView::Session { .. } => None,
                })
                .unwrap_or_else(|| self.name_of(&raised.session)),
            _ => self.name_of(&raised.session),
        }
    }

    /// What `raised` is about, in a line for the owner, when there is more to say than its
    /// event: the question, the command, the host, the reason it failed.
    pub(crate) fn detail_of(&self, raised: &Raised) -> Option<String> {
        let report = self.reports.get(&raised.session);
        match &raised.cause {
            Cause::Question(question) => Some(question.clone()),
            Cause::Run(id) | Cause::Input(id) => self
                .runs
                .iter()
                .find(|run| run.id == *id)
                .map(|run| run.command.clone()),
            Cause::Guide(id) => self
                .guides
                .iter()
                .find(|guide| guide.id == *id)
                .map(|guide| guide.title.text.clone()),
            Cause::Network(id) => self
                .network
                .iter()
                .find(|request| request.id == *id)
                .map(|request| request.target.clone()),
            Cause::Disk => self.disk.map(|disk| {
                format!(
                    "{} MiB free, below the floor of {} MiB",
                    disk.free_mib, disk.floor_mib
                )
            }),
            Cause::Usage { window, .. } => self.quota_alerts.get(window).map(|alert| {
                format!(
                    "{}% used, alert at {}%, account {}",
                    alert.used_percent, alert.threshold, alert.account
                )
            }),
            Cause::Budget { item, .. } => self.spent_budgets.get(item).map(|spent| {
                format!(
                    "item {item} of {}, {} min budget: `torlek queue explain {item}` says how to \
                     go on",
                    spent.project, spent.budget_minutes
                )
            }),
            Cause::SessionBudget { budget_minutes } => Some(format!(
                "worked its {budget_minutes} min budget since it started: it takes no queued \
                 work until you raise that total, `torlek session budget {} MINUTES`",
                raised.session
            )),
            Cause::State if raised.event == Event::Failure => {
                let turn_reason = report.and_then(|report| match &report.outcome {
                    Some(TurnOutcome::Failed { reason }) => reason.clone(),
                    _ => None,
                });
                self.sessions
                    .get(&raised.session)
                    .and_then(|session| session.reason.clone())
                    .or(turn_reason)
            }
            Cause::State => report.and_then(|report| report.doing.clone()),
        }
    }
}

/// What the notifier reads of a checkout: only its project, so a kind or trust this version
/// does not know never refuses a batch that carries the sessions too.
#[derive(serde::Deserialize)]
struct CheckoutProject {
    id: i64,
    project: String,
}

fn decode<View: serde::de::DeserializeOwned>(
    state: &serde_json::Map<String, serde_json::Value>,
) -> Result<View, String> {
    serde_json::from_value(serde_json::Value::Object(state.clone()))
        .map_err(|error| error.to_string())
}

/// The session a network request came from; `None` for a check run's, which no session waits
/// on.
fn network_session(request: &NetworkRequestView) -> Option<&str> {
    match &request.subject {
        NetworkSubjectView::Session { id } => Some(id),
        NetworkSubjectView::Check { .. } => None,
    }
}
