//! What the TUI knows of the daemon's state: projects, checkouts, sessions, the agents'
//! reports, the approval queue and the bug report drafts. It changes only by applying a change-feed batch or a
//! reply as a whole; nothing here waits on the daemon.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use cockpit_protocol::{
    AgentState, BugReportView, Change, ChangeBatch, CheckoutView, EntityKind, FeedPosition,
    GuideStateView, GuideView, LaunchKindView, NetworkRequestStateView, NetworkRequestView,
    NetworkSubjectView, ProjectView, QuestionSource, RunRequestView, RunStateView, SessionReport,
    SessionStateView, SessionView, TurnOutcome, ViewArea, ViewStamp,
};

use crate::attention::{Cue, StartSource, Watch};

/// The daemon's state as last applied.
#[derive(Debug, Default, Clone)]
pub struct Board {
    pub(crate) projects: Vec<ProjectView>,
    pub(crate) checkouts: BTreeMap<i64, CheckoutView>,
    pub(crate) sessions: BTreeMap<String, SessionView>,
    /// Sessions the feed took off: a command's late answer must not bring one back.
    pub(crate) retracted: BTreeSet<String>,
    pub(crate) reports: BTreeMap<String, SessionReport>,
    pub(crate) runs: Vec<RunRequestView>,
    pub(crate) guides: Vec<GuideView>,
    /// The hosts sandboxed sessions were refused, asking the owner (#674).
    pub(crate) network: Vec<NetworkRequestView>,
    /// The bug report drafts, by id (#471): waiting for a send, or opened by one (#847).
    pub(crate) bug_reports: BTreeMap<i64, BugReportView>,
    /// The last stamp the feed gave each area the daemon answers by request (#706). Kept
    /// across snapshots: a stamp that differs after one is how an outage's changes are found.
    stamps: BTreeMap<ViewArea, ViewStamp>,
    /// Areas whose stamp moved since the runtime last took them to ask for them again.
    stale_areas: BTreeSet<ViewArea>,
    /// Which sessions need the owner now, for the bell (#682).
    watch: Watch,
}

/// A change the TUI cannot read: the batch is refused whole, never applied in part.
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

/// One project's sessions, in the order the list shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectGroup<'board> {
    /// `None` for sessions whose checkout the TUI does not know.
    pub project: Option<&'board str>,
    pub sessions: Vec<&'board SessionView>,
}

impl Board {
    /// Applies `batch` whole: sessions, checkouts and bug report drafts change, other entity
    /// kinds are not shown by the basic TUI. A snapshot replaces what the feed carries.
    pub fn apply(&mut self, batch: &ChangeBatch) -> Result<(), FeedError> {
        // Every change is decoded before any is applied: only decoding can fail.
        let mut edits = Vec::with_capacity(batch.changes.len());
        for event in &batch.changes {
            let refused = |reason: String| FeedError {
                entity: format!("{:?} {}", event.entity.kind, event.entity.id),
                reason,
            };
            edits.push(match (&event.entity.kind, &event.change) {
                (EntityKind::Session, Change::Upsert { state }) => {
                    Edit::PutSession(decode(state).map_err(refused)?)
                }
                (EntityKind::Session, Change::Tombstone) => {
                    Edit::RemoveSession(event.entity.id.clone())
                }
                (EntityKind::Checkout, Change::Upsert { state }) => {
                    Edit::PutCheckout(decode(state).map_err(refused)?)
                }
                (EntityKind::Checkout, Change::Tombstone) => Edit::RemoveCheckout(
                    event
                        .entity
                        .id
                        .parse()
                        .map_err(|_| refused("a checkout id is a number".to_owned()))?,
                ),
                (EntityKind::BugReport, Change::Upsert { state }) => {
                    Edit::PutBugReport(decode(state).map_err(refused)?)
                }
                (EntityKind::BugReport, Change::Tombstone) => Edit::RemoveBugReport(
                    event
                        .entity
                        .id
                        .parse()
                        .map_err(|_| refused("a bug report id is a number".to_owned()))?,
                ),
                (EntityKind::ViewStamp, Change::Upsert { state }) => {
                    // An area this version does not know has no view here to refresh.
                    let Some(area) = ViewArea::from_id(&event.entity.id) else {
                        continue;
                    };
                    Edit::PutStamp(area, decode(state).map_err(refused)?)
                }
                _ => continue,
            });
        }
        if batch.follows == FeedPosition::Snapshot {
            self.retracted
                .extend(std::mem::take(&mut self.sessions).into_keys());
            self.checkouts.clear();
            self.bug_reports.clear();
        }
        // The feed is the truth: its states replace whatever a command answered.
        for edit in edits {
            match edit {
                Edit::PutSession(session) => {
                    self.retracted.remove(&session.id);
                    self.sessions.insert(session.id.clone(), session);
                }
                Edit::RemoveSession(id) => {
                    self.sessions.remove(&id);
                    self.retracted.insert(id);
                }
                Edit::PutCheckout(checkout) => {
                    self.checkouts.insert(checkout.id, checkout);
                }
                Edit::RemoveCheckout(id) => {
                    self.checkouts.remove(&id);
                }
                Edit::PutBugReport(report) => {
                    self.bug_reports.insert(report.id, report);
                }
                Edit::RemoveBugReport(id) => {
                    self.bug_reports.remove(&id);
                }
                Edit::PutStamp(area, stamp) => {
                    if self.stamps.insert(area, stamp) != Some(stamp) {
                        self.stale_areas.insert(area);
                    }
                }
            }
        }
        self.look_for_cues(Some(StartSource::Sessions));
        Ok(())
    }

    /// The areas whose stamp moved since the last call, for the runtime to ask for again.
    pub(crate) fn take_stale_areas(&mut self) -> BTreeSet<ViewArea> {
        std::mem::take(&mut self.stale_areas)
    }

    pub fn set_projects(&mut self, projects: Vec<ProjectView>) {
        self.projects = projects;
    }

    /// One session as a command answered it, ahead of the feed. A command's answer may
    /// arrive after the feed moved on: an older revision than the one shown, or a session
    /// the feed took off, is ignored; the feed brings any later state.
    pub fn put_session(&mut self, session: SessionView) {
        let is_stale = self.retracted.contains(&session.id)
            || self
                .sessions
                .get(&session.id)
                .is_some_and(|shown| shown.revision > session.revision);
        if is_stale {
            return;
        }
        self.sessions.insert(session.id.clone(), session);
        // Not the start state: only the feed's snapshot says the sessions are loaded.
        self.look_for_cues(None);
    }

    pub fn set_reports(&mut self, reports: Vec<SessionReport>) {
        self.reports = reports
            .into_iter()
            .map(|report| (report.session.clone(), report))
            .collect();
        self.look_for_cues(Some(StartSource::Reports));
    }

    pub fn set_queue(&mut self, runs: Vec<RunRequestView>, guides: Vec<GuideView>) {
        self.runs = runs;
        self.guides = guides;
        self.look_for_cues(Some(StartSource::Queue));
    }

    /// The network requests asking the owner. Not a start source: a request already waiting
    /// when the TUI started may ring once, the safer error (#682).
    pub fn set_network(&mut self, requests: Vec<NetworkRequestView>) {
        self.network = requests;
        self.look_for_cues(None);
    }

    /// Whether the bell should ring for what changed since the last call (#682): a session
    /// raised a cue it did not raise before, all that landed together counting once.
    pub(crate) fn take_ring(&mut self) -> bool {
        self.watch.take_ring()
    }

    fn look_for_cues(&mut self, source: Option<StartSource>) {
        let raised = self
            .sessions
            .values()
            .flat_map(|session| {
                self.cues_of(session)
                    .into_iter()
                    .map(|cue| (session.id.clone(), cue))
            })
            .collect();
        self.watch.look(source, raised);
    }

    fn cues_of(&self, session: &SessionView) -> Vec<Cue> {
        let outcome = self
            .reports
            .get(&session.id)
            .and_then(|report| report.outcome.as_ref());
        let mut cues = Vec::new();
        if session.finished || outcome == Some(&TurnOutcome::Completed) {
            cues.push(Cue::Finished);
        }
        if let Some(what) = self.what_needs_you(&session.id) {
            cues.push(Cue::NeedsYou { what });
        }
        let exited_by_itself = session.state.is_over() && session.exit.is_some();
        let turn_failed = matches!(outcome, Some(TurnOutcome::Failed { .. }));
        if session.state == SessionStateView::Failed || exited_by_itself || turn_failed {
            cues.push(Cue::Failed);
        }
        cues
    }

    /// Drops a project the daemon removed, ahead of the next project list.
    pub fn remove_project(&mut self, name: &str) {
        self.projects.retain(|project| project.name != name);
    }

    pub fn projects(&self) -> &[ProjectView] {
        &self.projects
    }

    /// Sessions grouped by project name, projects sorted by name; the sessions of an
    /// unknown checkout come last, under no project, so they are never hidden. Finished
    /// sessions are not in them: they fold into [`Board::finished_sessions`].
    pub fn groups(&self) -> Vec<ProjectGroup<'_>> {
        let mut groups: BTreeMap<Option<&str>, Vec<&SessionView>> = BTreeMap::new();
        for project in &self.projects {
            groups.entry(Some(project.name.as_str())).or_default();
        }
        for session in self.sessions.values() {
            if session.state == SessionStateView::Ended || self.is_folded(session) {
                continue;
            }
            groups
                .entry(self.project_of(session))
                .or_default()
                .push(session);
        }
        let mut ordered: Vec<ProjectGroup<'_>> = groups
            .into_iter()
            .map(|(project, mut sessions)| {
                sessions.sort_by(|left, right| {
                    (left.created_at, &left.id).cmp(&(right.created_at, &right.id))
                });
                ProjectGroup { project, sessions }
            })
            .collect();
        // `None` sorts first in a map; it belongs at the end.
        if ordered.first().is_some_and(|group| group.project.is_none()) {
            ordered.rotate_left(1);
        }
        ordered
    }

    /// The sessions the task list says finished their work, that have not ended and do not
    /// wait on the owner, folded into one group the owner exits together (#157, decided in #201).
    pub fn finished_sessions(&self) -> Vec<&SessionView> {
        self.sessions
            .values()
            .filter(|session| self.is_folded(session) && session.state != SessionStateView::Ended)
            .collect()
    }

    /// A finished session folds away unless it waits on the owner or a coordinator holds its
    /// question (#710): that one stays listed, so its question or approval is never hidden
    /// behind a count.
    fn is_folded(&self, session: &SessionView) -> bool {
        session.finished
            && !self.needs_you(&session.id)
            && self.question_holder(&session.id).is_none()
    }

    /// The sessions in list order, flattened across groups.
    pub fn listed_sessions(&self) -> Vec<&SessionView> {
        self.groups()
            .into_iter()
            .flat_map(|group| group.sessions)
            .collect()
    }

    /// Whether the session waits on the owner: an approval or guide it asked for, a command
    /// of its waiting for input, or a question it reported that no coordinator holds (#710).
    pub fn needs_you(&self, session: &str) -> bool {
        self.has_open_request(session)
            || self
                .reports
                .get(session)
                .is_some_and(|report| report.needs_input.is_some() && report.held_by.is_none())
    }

    /// The coordinator session that holds the session's question before the owner (#710).
    pub fn question_holder(&self, session: &str) -> Option<&str> {
        self.reports
            .get(session)
            .and_then(|report| report.held_by.as_deref())
    }

    /// An approval, guide or network request the session asked for, or a command of its
    /// waiting for input.
    pub(crate) fn has_open_request(&self, session: &str) -> bool {
        self.open_requests(session).next().is_some()
    }

    /// Names each approval or guide the session asked for and each command of its waiting for
    /// input, so a new request differs from one already open.
    fn open_requests<'board>(
        &'board self,
        session: &'board str,
    ) -> impl Iterator<Item = String> + 'board {
        let runs = self
            .runs
            .iter()
            .filter(move |run| {
                run.session == session
                    && (run.state == RunStateView::Asking || run.waiting_for_input)
            })
            .map(|run| format!("run {} waiting_for_input={}", run.id, run.waiting_for_input));
        let guides = self
            .guides
            .iter()
            .filter(move |guide| guide.session == session && guide.state == GuideStateView::Asking)
            .map(|guide| format!("guide {}", guide.id));
        let network = self
            .network
            .iter()
            .filter(move |request| {
                request.state == NetworkRequestStateView::Asking
                    && network_session(request) == Some(session)
            })
            .map(|request| format!("network {}", request.id));
        runs.chain(guides).chain(network)
    }

    /// What the session waits on the owner for, as [`Board::needs_you`] decides it: every open
    /// request and the question it reported, or `None` when it waits for nothing.
    fn what_needs_you(&self, session: &str) -> Option<String> {
        let question = self
            .reports
            .get(session)
            .and_then(|report| report.needs_input.as_deref())
            .map(|question| format!("question {question}"));
        let what: Vec<String> = self.open_requests(session).chain(question).collect();
        (!what.is_empty()).then(|| what.join("; "))
    }

    /// Records that the request for `source` failed, so a daemon that never answers it does
    /// not keep the bell silent for the whole run.
    pub(crate) fn start_source_failed(&mut self, source: StartSource) {
        self.look_for_cues(Some(source));
    }

    /// Whether all that waits on the owner is the agent's own question, asked while it still
    /// works (#182): it goes on with its turn instead of being blocked on a dialog, a request
    /// or a guide (#215). A report without a question source, from a daemon before #182,
    /// counts as blocked.
    pub fn asked_while_working(&self, session: &str) -> bool {
        !self.has_open_request(session)
            && self.reports.get(session).is_some_and(|report| {
                report.needs_input.is_some()
                    && report.question_source == Some(QuestionSource::Agent)
                    && report.state == Some(AgentState::Working)
            })
    }

    /// Whether the session's process runs while its agent waits with nothing in progress
    /// (#864): the lifecycle says `running`, but nothing happens until the owner types.
    /// A report from before the session's latest launch says nothing about the agent that
    /// runs now, so a resumed session counts as idle until a report from after the resume
    /// says otherwise.
    pub fn idle(&self, session: &SessionView) -> Option<Idle> {
        if session.state != SessionStateView::Running || self.needs_you(&session.id) {
            return None;
        }
        let report = self.reports.get(&session.id);
        let resumed_at = session
            .launch
            .as_ref()
            .filter(|launch| launch.kind == LaunchKindView::Resume)
            .map(|launch| launch.launched_at);
        if let Some(resumed_at) = resumed_at
            && report.is_none_or(|report| report.reported_at < resumed_at)
        {
            return Some(Idle::Resumed);
        }
        let report = report?;
        (report.state == Some(AgentState::Waiting) && report.doing.is_none())
            .then_some(Idle::Waiting)
    }

    /// The bug report drafts, oldest first: the ones waiting for the owner to send them and
    /// the ones a send opened, whose issue is not recorded yet (#847).
    pub fn bug_report_drafts(&self) -> Vec<&BugReportView> {
        self.bug_reports.values().collect()
    }

    pub fn report(&self, session: &str) -> Option<&SessionReport> {
        self.reports.get(session)
    }

    pub fn project_of(&self, session: &SessionView) -> Option<&str> {
        self.checkouts
            .get(&session.checkout)
            .map(|checkout| checkout.project.as_str())
    }
}

/// Why a running session shows as waiting (#864).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idle {
    /// Its agent reported waiting and has nothing in progress.
    Waiting,
    /// It was resumed and nothing was sent to it since.
    Resumed,
}

/// One decoded change of a batch.
enum Edit {
    PutSession(SessionView),
    RemoveSession(String),
    PutCheckout(CheckoutView),
    RemoveCheckout(i64),
    PutBugReport(BugReportView),
    RemoveBugReport(i64),
    PutStamp(ViewArea, ViewStamp),
}

fn decode<View: serde::de::DeserializeOwned>(
    state: &serde_json::Map<String, serde_json::Value>,
) -> Result<View, String> {
    View::deserialize(state).map_err(|error| error.to_string())
}

/// The session a network request came from; `None` for a check run's.
fn network_session(request: &NetworkRequestView) -> Option<&str> {
    match &request.subject {
        NetworkSubjectView::Session { id } => Some(id),
        NetworkSubjectView::Check { .. } => None,
    }
}

/// Who a network request came from, as the owner reads it.
pub(crate) fn network_requester(request: &NetworkRequestView) -> String {
    match &request.subject {
        NetworkSubjectView::Session { id } => id.clone(),
        NetworkSubjectView::Check { run, check } => format!("check {check} (run {run})"),
    }
}
