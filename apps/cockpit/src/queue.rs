//! The queue's explicit decisions, shared by dispatch and its read-only explanation.

use crate::{sessions::Session, todo::{self, Entry, TodoList}, reviews::Issue, util};
use std::{collections::{BTreeMap, HashSet}, fmt};

/// Leave time to read a finished turn before giving an unwatched session more work.
pub const QUEUE_IDLE_S: i64 = 300;
pub const QUEUE_CHECK_S: f64 = 30.0;

pub struct QueueContext {
    pub now: i64,
    pub enabled: bool,
    pub gaming: bool,
    pub viewed: String,
    pub workspace_active: bool,
    pub qa_sessions: HashSet<String>,
    pub chief_session: Option<String>,
    pub host_available: bool,
}

#[derive(Debug, PartialEq)]
pub enum QueueBlock {
    HostUnavailable,
    Disabled, Gaming, External, Terminal, Qa, Chief, MissingDirectory, Unknown, Working,
    NeedsInput, CommandRunning, Busy, PendingMessage, Viewed, Idle { remaining: i64 },
}

impl fmt::Display for QueueBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HostUnavailable => formatter.write_str("team host unavailable"),
            Self::Disabled => formatter.write_str("queue disabled"),
            Self::Gaming => formatter.write_str("paused while gaming"),
            Self::External => formatter.write_str("no terminal target"),
            Self::Terminal => formatter.write_str("terminal-managed session"),
            Self::Qa => formatter.write_str("QA is reserved for its assigned review"),
            Self::Chief => formatter.write_str("the Chief's own session"),
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
    if !context.host_available { return Some(QueueBlock::HostUnavailable); }
    if session.is_external() { return Some(QueueBlock::External); }
    if session.terminal { return Some(QueueBlock::Terminal); }
    if context.qa_sessions.contains(&session.session) { return Some(QueueBlock::Qa); }
    if context.chief_session.as_deref() == Some(session.session.as_str()) { return Some(QueueBlock::Chief); }
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

pub fn reserve_task(directory: &str, task_id: &str, worker: Option<&str>, automatic: bool) -> Result<String, String> {
    reserve(crate::teams::HostRequest::ReserveTask { task_id: task_id.into(), directory:directory.into(), worker:worker.map(str::to_string), automatic })
}

pub fn reserve_resume(directory: &str, session: &str) -> Result<String, String> {
    reserve(crate::teams::HostRequest::ReserveResume { session:session.into(), directory:directory.into() })
}

pub fn reserve_manual(directory: &str, task_id: &str) -> Result<String,String> {
    reserve(crate::teams::HostRequest::ReserveManual {directory:directory.into(),task_id:task_id.into()})
}

pub fn release_manual(directory: &str, task_id: &str, token: Option<&str>) -> Result<(),String> {
    let Some(token)=token else {return Ok(());};
    crate::teams::client::host_request(&crate::teams::HostRequest::ReleaseManual {directory:directory.into(),task_id:task_id.into(),token:token.into()})
        .and_then(|reply|reply.result).map(|_|()).map_err(|error|error.message)
}

fn reserve(request: crate::teams::HostRequest) -> Result<String, String> {
    let reply = crate::teams::client::host_request(&request).and_then(|reply| reply.result).map_err(|error| error.message)?;
    if reply["reservation"].as_str().is_none_or(str::is_empty) { return Err("scheduling reservation unavailable".into()); }
    Ok(reply["reservation"].as_str().expect("reservation validated").into())
}

fn settle(token: &str, revision: i64, outcome: &str) -> Result<(),String> {
    crate::teams::client::host_request(&crate::teams::HostRequest::SettleReservation {token:token.into(),expected_revision:revision,outcome:outcome.into()})
        .and_then(|reply|reply.result).map(|_|()).map_err(|error|error.message)
}

/// Mark possible execution before spawning; only a failure to spawn proves non-submission.
pub fn launch_reserved(token: &str, arguments: &[&str], background: bool) -> Result<(),String> {
    settle(token,1,"begin")?;
    let child=std::process::Command::new("cockpit").args(arguments).stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).spawn();
    let child=match child {
        Ok(child)=>child,
        Err(error)=>{settle(token,2,"not_submitted")?;return Err(format!("cockpit launch was not submitted: {error}"));},
    };
    let token=token.to_string();
    let finish=move || {
        let output=match child.wait_with_output() {Ok(output)=>output,Err(error)=>{settle(&token,2,"uncertain")?;return Err(format!("launch outcome uncertain: {error}"));}};
        if !output.status.success() {settle(&token,2,"uncertain")?;return Err(format!("launch outcome uncertain; host recovery required: {}",String::from_utf8_lossy(&output.stderr).trim()));}
        settle(&token,2,"submitted")
    };
    if background {
        std::thread::spawn(move || {if let Err(error)=finish() {util::log_event("ERROR",&error);}});
        Ok(())
    } else {finish()}
}

pub fn candidates(session: &Session, todos: &TodoList, issues: &[Issue], checkouts: &BTreeMap<String, String>) -> Vec<Entry> {
    let project = util::project_name(&util::repository_root(&session.cwd));
    let mut entries: Vec<_> = todos.items.iter()
        .filter(|item| util::project_name(&util::repository_root(&item.cwd())) == project
            && item.status() == "todo" && todos.is_queued(&item.id()) && !item.is_waiting() && item.is_design() == (session.role == "designer"))
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
