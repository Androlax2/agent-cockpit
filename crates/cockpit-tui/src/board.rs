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
                _ => continue,
            });
        }
        if batch.follows == FeedPosition::Snapshot {
            self.sessions.clear();
            self.checkouts.clear();
        }
        for edit in edits {
            match edit {
                Edit::PutSession(session) => self.put_session(session),
                Edit::RemoveSession(id) => {
                    self.sessions.remove(&id);
                }
                Edit::PutCheckout(checkout) => {
                    self.checkouts.insert(checkout.id, checkout);
                }
                Edit::RemoveCheckout(id) => {
                    self.checkouts.remove(&id);
                }
            }
        }
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

/// One decoded change of a batch.
enum Edit {
    PutSession(SessionView),
    RemoveSession(String),
    PutCheckout(CheckoutView),
    RemoveCheckout(i64),
}

fn decode<View: serde::de::DeserializeOwned>(
    state: &serde_json::Map<String, serde_json::Value>,
) -> Result<View, String> {
    View::deserialize(state).map_err(|error| error.to_string())
}
