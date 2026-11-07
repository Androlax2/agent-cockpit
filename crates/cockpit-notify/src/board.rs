//! What the notifier knows of the daemon's state: the sessions from the change feed, and the
//! agents' reports, the approval queue and the network requests the daemon answers by request.
//! From them it derives what each session needs the owner for now. The checkouts on the feed
//! name each session's project for the status file.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use cockpit_protocol::{
    Change, ChangeBatch, EntityKind, FeedPosition, GuideStateView, GuideView,
    NetworkRequestStateView, NetworkRequestView, NetworkSubjectView, QuestionSource,
    RunRequestView, RunStateView, SessionReport, SessionStateView, SessionView, TurnOutcome,
    ViewArea, ViewStamp,
};

use crate::event::Event;

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
}

impl Board {
    /// Applies `batch` whole: sessions, checkouts and view stamps change, other kinds are not
    /// read. A snapshot replaces the sessions and the checkouts.
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
        self.sessions
            .values()
            .flat_map(|session| self.raised_by(session))
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
                && network_session(request) == session.id
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

/// The session a network request came from.
fn network_session(request: &NetworkRequestView) -> &str {
    match &request.subject {
        NetworkSubjectView::Session { id } => id,
    }
}
