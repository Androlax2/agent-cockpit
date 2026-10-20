//! The queue's explicit decisions, shared by dispatch and its read-only explanation.

use crate::{sessions::Session, todo::{self, Entry, TodoList}, reviews::Issue, util};
use std::{collections::BTreeMap, fmt};

/// Leave time to read a finished turn before giving an unwatched session more work.
pub const QUEUE_IDLE_S: i64 = 300;
pub const QUEUE_CHECK_S: f64 = 30.0;

pub struct QueueContext {
    pub now: i64,
    pub enabled: bool,
    pub gaming: bool,
    pub viewed: String,
    pub workspace_active: bool,
}

#[derive(Debug, PartialEq)]
pub enum QueueBlock {
    Disabled, Gaming, External, Terminal, MissingDirectory, Unknown, Working,
    NeedsInput, CommandRunning, Busy, PendingMessage, Viewed, Idle { remaining: i64 },
}

impl fmt::Display for QueueBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("queue disabled"),
            Self::Gaming => formatter.write_str("paused while gaming"),
            Self::External => formatter.write_str("no terminal target"),
            Self::Terminal => formatter.write_str("terminal-managed session"),
            Self::MissingDirectory => formatter.write_str("missing checkout directory"),
            Self::Unknown => formatter.write_str("unknown state"),
            Self::Working => formatter.write_str("still working"),
            Self::NeedsInput => formatter.write_str("awaiting your answer, approval or input"),
            Self::CommandRunning => formatter.write_str("approved command is running"),
            Self::Busy => formatter.write_str("assigned todo still in progress"),
            Self::PendingMessage => formatter.write_str("manual message queued"),
            Self::Viewed => formatter.write_str("viewed on the active cockpit workspace"),
            Self::Idle { remaining } => write!(formatter, "idle window: {remaining}s remaining (minimum {QUEUE_IDLE_S}s)"),
        }
    }
}

pub fn block(session: &Session, context: &QueueContext, busy: bool, has_pending_message: bool) -> Option<QueueBlock> {
    if !context.enabled { return Some(QueueBlock::Disabled); }
    if context.gaming { return Some(QueueBlock::Gaming); }
    if session.is_external() { return Some(QueueBlock::External); }
    if session.terminal { return Some(QueueBlock::Terminal); }
    if session.cwd.is_empty() { return Some(QueueBlock::MissingDirectory); }
    if session.needs_attention() { return Some(QueueBlock::NeedsInput); }
    if session.ask_to_run == "running" { return Some(QueueBlock::CommandRunning); }
    if session.state == "working" { return Some(QueueBlock::Working); }
    if session.state != "waiting" { return Some(QueueBlock::Unknown); }
    if busy { return Some(QueueBlock::Busy); }
    if has_pending_message { return Some(QueueBlock::PendingMessage); }
    if context.workspace_active && session.session == context.viewed { return Some(QueueBlock::Viewed); }
    let idle = context.now.saturating_sub(session.since);
    if idle < QUEUE_IDLE_S { return Some(QueueBlock::Idle { remaining: QUEUE_IDLE_S - idle }); }
    None
}

pub fn candidates(session: &Session, todos: &TodoList, issues: &[Issue], checkouts: &BTreeMap<String, String>) -> Vec<Entry> {
    let project = util::project_name(&util::repository_root(&session.cwd));
    let mut entries: Vec<_> = todos.items.iter()
        .filter(|item| util::project_name(&util::repository_root(&item.cwd())) == project
            && item.status() == "todo" && todos.is_queued(&item.id()) && item.is_design() == (session.role == "designer"))
        .map(|item| Entry::Typed(item.clone())).collect();
    for issue in issues.iter().filter(|_| session.role != "designer") {
        let matches = if session.repository.is_empty() {
            checkouts.get(&issue.full_repository).is_some_and(|checkout| util::project_name(&util::repository_root(checkout)) == project)
        } else { session.repository == issue.full_repository.to_lowercase() };
        if matches && todos.is_queued(&issue.key()) && todos.status_of(&issue.key()) == "todo" {
            entries.push(Entry::Issue { issue: issue.clone(), status: "todo".into(), priority: todo::issue_priority(&issue.labels) });
        }
    }
    entries.sort_by(todo::queue_order);
    entries
}
