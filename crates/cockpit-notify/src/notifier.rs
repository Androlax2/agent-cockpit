//! Turns what the daemon says into notices: each event a session newly raises, once the state
//! the notifier started on is loaded.

use std::collections::BTreeSet;

use cockpit_protocol::{ChangeBatch, Command, Reply, ViewArea};

use crate::board::{Board, FeedError, Raised};
use crate::event::Event;

/// One event to tell the owner about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub event: Event,
    pub session: String,
    /// The name the owner knows the session by: its label, else its id.
    pub name: String,
    /// The question, command, host or reason, when there is more to say than the event.
    pub detail: Option<String>,
}

/// The daemon's state arrives from four places. What is raised before all four answered once
/// is the state the notifier started on, not news. A place the daemon answered with an error
/// counts as answered: a notifier that waits for a request the daemon always refuses would
/// stay silent for the whole run. A daemon out of reach answered nothing; the runtime does
/// not pass that on, or everything already waiting would be news once it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StartSource {
    Sessions,
    Reports,
    Queue,
    Network,
}

const START_SOURCES: [StartSource; 4] = [
    StartSource::Sessions,
    StartSource::Reports,
    StartSource::Queue,
    StartSource::Network,
];

#[derive(Debug, Default, Clone)]
pub struct Notifier {
    board: Board,
    /// What was raised as of the last look.
    raised: BTreeSet<Raised>,
    loaded: BTreeSet<StartSource>,
}

impl Notifier {
    /// What the runtime asks the daemon at start and on every refresh, the slow fallback
    /// beside the view stamps (#706). Sessions come from the change feed.
    pub fn refresh_commands() -> Vec<Command> {
        vec![
            Command::AgentReports,
            Command::ApprovalQueue,
            Command::NetworkList,
        ]
    }

    /// Applies a change-feed batch whole, and returns what it newly raised.
    pub fn apply_changes(&mut self, batch: &ChangeBatch) -> Result<Vec<Notice>, FeedError> {
        self.board.apply(batch)?;
        Ok(self.look(Some(StartSource::Sessions)))
    }

    /// The requests that bring the views whose stamp moved up to date, at once instead of at
    /// the next refresh (#706).
    pub fn changed_view_commands(&mut self) -> Vec<Command> {
        let mut commands = Vec::new();
        for area in self.board.take_stale_areas() {
            match area {
                ViewArea::AgentReports => commands.push(Command::AgentReports),
                ViewArea::Approvals => {
                    commands.extend([Command::ApprovalQueue, Command::NetworkList]);
                }
                ViewArea::Projects | ViewArea::Tasks | ViewArea::Workflows => {}
            }
        }
        commands
    }

    /// Takes the daemon's answer to `command`, a reply or the error it answered with, and
    /// returns what it newly raised.
    pub fn answer(&mut self, command: &Command, answer: Result<Reply, String>) -> Vec<Notice> {
        let source = match command {
            Command::AgentReports => StartSource::Reports,
            Command::ApprovalQueue => StartSource::Queue,
            Command::NetworkList => StartSource::Network,
            _ => return Vec::new(),
        };
        match answer {
            Ok(Reply::AgentReports { reports }) => self.board.set_reports(reports),
            Ok(Reply::ApprovalQueue { runs, guides }) => self.board.set_queue(runs, guides),
            Ok(Reply::NetworkRequests { requests }) => self.board.set_network(requests),
            Ok(reply) => {
                tracing::warn!(?command, ?reply, "the daemon answered something else");
            }
            Err(problem) => tracing::warn!(?command, problem, "the daemon refused the request"),
        }
        self.look(Some(source))
    }

    /// Looks at what is raised now; what was not raised at the last look is news once the
    /// start state is loaded.
    fn look(&mut self, source: Option<StartSource>) -> Vec<Notice> {
        let was_loaded = self.is_loaded();
        self.loaded.extend(source);
        let raised = self.board.raised();
        let news = if was_loaded {
            raised
                .difference(&self.raised)
                .map(|raised| self.notice(raised))
                .collect()
        } else {
            Vec::new()
        };
        self.raised = raised;
        news
    }

    fn notice(&self, raised: &Raised) -> Notice {
        Notice {
            event: raised.event,
            session: raised.session.clone(),
            name: self.board.name_of(&raised.session),
            detail: self.board.detail_of(raised),
        }
    }

    fn is_loaded(&self) -> bool {
        START_SOURCES
            .iter()
            .all(|source| self.loaded.contains(source))
    }
}
