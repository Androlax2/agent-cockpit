//! What the TUI knows of the daemon's state: projects, checkouts, sessions, the agents'
//! reports and the approval queue. It changes only by applying a change-feed batch or a
//! reply as a whole; nothing here waits on the daemon.

use std::collections::BTreeMap;
use std::fmt;

use cockpit_protocol::{
    Change, ChangeBatch, CheckoutView, EntityKind, FeedPosition, GuideStateView, GuideView,
    ProjectView, RunRequestView, RunStateView, SessionReport, SessionStateView, SessionView,
};

/// The daemon's state as last applied.
#[derive(Debug, Default, Clone)]
pub struct Board {
    pub(crate) projects: Vec<ProjectView>,
    pub(crate) checkouts: BTreeMap<i64, CheckoutView>,
    pub(crate) sessions: BTreeMap<String, SessionView>,
    pub(crate) reports: BTreeMap<String, SessionReport>,
    pub(crate) runs: Vec<RunRequestView>,
    pub(crate) guides: Vec<GuideView>,
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
    /// Applies `batch` whole: sessions and checkouts change, other entity kinds are not
    /// shown by the basic TUI. A snapshot replaces what the feed carries.
    pub fn apply(&mut self, batch: &ChangeBatch) -> Result<(), FeedError> {
        let mut sessions = self.sessions.clone();
        let mut checkouts = self.checkouts.clone();
        if batch.follows == FeedPosition::Snapshot {
            sessions.clear();
            checkouts.clear();
        }
        for event in &batch.changes {
            let entity = format!("{:?} {}", event.entity.kind, event.entity.id);
            match (&event.entity.kind, &event.change) {
                (EntityKind::Session, Change::Upsert { state }) => {
                    sessions.insert(event.entity.id.clone(), decode(state, &entity)?);
                }
                (EntityKind::Session, Change::Tombstone) => {
                    sessions.remove(&event.entity.id);
                }
                (EntityKind::Checkout, Change::Upsert { state }) => {
                    let checkout: CheckoutView = decode(state, &entity)?;
                    checkouts.insert(checkout.id, checkout);
                }
                (EntityKind::Checkout, Change::Tombstone) => {
                    let id = event.entity.id.parse().map_err(|_| FeedError {
                        entity: entity.clone(),
                        reason: "a checkout id is a number".to_owned(),
                    })?;
                    checkouts.remove(&id);
                }
                _ => {}
            }
        }
        self.sessions = sessions;
        self.checkouts = checkouts;
        Ok(())
    }

    pub fn set_projects(&mut self, projects: Vec<ProjectView>) {
        self.projects = projects;
    }

    pub fn set_checkouts(&mut self, checkouts: Vec<CheckoutView>) {
        self.checkouts = checkouts
            .into_iter()
            .map(|checkout| (checkout.id, checkout))
            .collect();
    }

    /// The sessions not ended, as `session-list` answers them.
    pub fn set_sessions(&mut self, sessions: Vec<SessionView>) {
        self.sessions = sessions
            .into_iter()
            .map(|session| (session.id.clone(), session))
            .collect();
    }

    /// One session as a command answered it, ahead of the next refresh.
    pub fn put_session(&mut self, session: SessionView) {
        self.sessions.insert(session.id.clone(), session);
    }

    pub fn set_reports(&mut self, reports: Vec<SessionReport>) {
        self.reports = reports
            .into_iter()
            .map(|report| (report.session.clone(), report))
            .collect();
    }

    pub fn set_queue(&mut self, runs: Vec<RunRequestView>, guides: Vec<GuideView>) {
        self.runs = runs;
        self.guides = guides;
    }

    pub fn projects(&self) -> &[ProjectView] {
        &self.projects
    }

    /// Sessions grouped by project name, projects sorted by name; the sessions of an
    /// unknown checkout come last, under no project, so they are never hidden.
    pub fn groups(&self) -> Vec<ProjectGroup<'_>> {
        let mut groups: BTreeMap<Option<&str>, Vec<&SessionView>> = BTreeMap::new();
        for project in &self.projects {
            groups.entry(Some(project.name.as_str())).or_default();
        }
        for session in self.sessions.values() {
            if session.state == SessionStateView::Ended {
                continue;
            }
            let project = self
                .checkouts
                .get(&session.checkout)
                .map(|checkout| checkout.project.as_str());
            groups.entry(project).or_default().push(session);
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

    /// The sessions in list order, flattened across groups.
    pub fn listed_sessions(&self) -> Vec<&SessionView> {
        self.groups()
            .into_iter()
            .flat_map(|group| group.sessions)
            .collect()
    }

    /// Whether the session waits on the owner: an approval or guide it asked for, a command
    /// of its waiting for input, or a question it reported.
    pub fn needs_you(&self, session: &str) -> bool {
        self.runs.iter().any(|run| {
            run.session == session && (run.state == RunStateView::Asking || run.waiting_for_input)
        }) || self
            .guides
            .iter()
            .any(|guide| guide.session == session && guide.state == GuideStateView::Asking)
            || self
                .reports
                .get(session)
                .is_some_and(|report| report.needs_input.is_some())
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

fn decode<View: serde::de::DeserializeOwned>(
    state: &serde_json::Map<String, serde_json::Value>,
    entity: &str,
) -> Result<View, FeedError> {
    serde_json::from_value(serde_json::Value::Object(state.clone())).map_err(|error| FeedError {
        entity: entity.to_owned(),
        reason: error.to_string(),
    })
}
