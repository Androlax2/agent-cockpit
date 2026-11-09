//! Time with the agent, recorded as turns happen (#700, report 18): a session's turn opens when
//! its agent reports `working` and ends when it reports anything else, when its session stops
//! running, or at its end. No source is polled: a pass runs after each commit, compares the
//! sessions' latest reports with the turns `cockpit-usage` holds open, and records the
//! difference. A pass that finds nothing to record takes no write transaction, so it does
//! not wake itself.
//!
//! A turn's end is also when an item placed on the session may have spent its budget: the
//! pass tells it in the same transaction (`budgets`, #412).
//!
//! A turn that begins and ends between two passes is missed; a turn lasts longer than a pass.
//! A session that stopped while its agent worked ends the turn at the session's last change.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use cockpit_sessions::{AgentState, Session, SessionId, SessionReport, SessionState, Sessions};
use cockpit_turso::Database;

/// How long a pass that failed waits before it runs again, commit or not.
const RETRY: Duration = Duration::from_secs(1);

/// Runs a pass at start and after every commit. Runs until aborted.
pub(crate) async fn track_forever(database: Arc<Database>, sessions: Arc<Sessions>) {
    // Watched before the first pass, so a commit during it runs another.
    let mut commits = database.watch_commits();
    loop {
        if let Err(problem) = pass(&database, &sessions).await {
            tracing::warn!(problem, "recording the sessions' turns failed; retrying");
            tokio::time::sleep(RETRY).await;
            continue;
        }
        if commits.changed().await.is_err() {
            return;
        }
    }
}

/// What a pass found: turns to end, then turns to open with the project's folder name resolved
/// later. Ends are applied first: a session relaunched while its agent worked ends the old
/// launch's turn and opens the new one's.
struct Difference {
    ends: Vec<(String, i64)>,
    begins: Vec<(String, std::path::PathBuf, i64)>,
}

async fn pass(database: &Database, sessions: &Sessions) -> Result<(), String> {
    let live = sessions
        .list(false)
        .await
        .map_err(|error| format!("cannot read the sessions: {error}"))?;
    let (reports, open) = {
        let read = database.read().await.map_err(|error| error.to_string())?;
        let reports = cockpit_sessions::latest_reports(&read)
            .await
            .map_err(|error| error.to_string())?;
        let open = cockpit_usage::open_turns(&read)
            .await
            .map_err(|error| error.to_string())?;
        (reports, open)
    };
    let reports: BTreeMap<String, SessionReport> = reports
        .into_iter()
        .map(|report| (report.session.as_str().to_owned(), report))
        .collect();
    let live: BTreeMap<String, Session> = live
        .into_iter()
        .map(|session| (session.id.to_string(), session))
        .collect();
    let now = crate::approvals::now();

    let mut difference = Difference {
        ends: Vec::new(),
        begins: Vec::new(),
    };
    let open: BTreeMap<&str, &cockpit_usage::OpenTurn> = open
        .iter()
        .map(|turn| (turn.session.as_str(), turn))
        .collect();
    for (id, turn) in &open {
        let session = live.get(*id);
        let report = reports.get(*id);
        let ended_at = match (session, report) {
            (Some(session), Some(report)) if is_working(session, report) => {
                match began_in_an_earlier_launch(session, turn) {
                    Some(launched_at) => launched_at,
                    None => continue,
                }
            }
            (Some(session), report) => end_in_live(session, report),
            (None, _) => ended_session_change(sessions, id).await.unwrap_or(now),
        };
        difference.ends.push(((*id).to_owned(), ended_at));
    }
    for (id, session) in &live {
        let Some(report) = reports.get(id) else {
            continue;
        };
        let continues_open_turn = open
            .get(id.as_str())
            .is_some_and(|turn| began_in_an_earlier_launch(session, turn).is_none());
        if is_working(session, report) && !continues_open_turn {
            difference
                .begins
                .push((id.clone(), session.working_dir.clone(), report.reported_at));
        }
    }
    if difference.begins.is_empty() && difference.ends.is_empty() {
        return Ok(());
    }

    let folders = crate::cost::project_folders(database).await?;
    let write = database.write().await.map_err(|error| error.to_string())?;
    for (session, ended_at) in &difference.ends {
        match cockpit_usage::turn_ended(&write, session, *ended_at).await {
            Ok(_) => {}
            // The turn is dropped with the refusal: retrying it would refuse it again.
            Err(error @ cockpit_usage::Error::Invalid { .. }) => {
                tracing::warn!(session, %error, "a turn could not be recorded and was dropped");
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    if !difference.ends.is_empty() {
        let ended: BTreeSet<&str> = difference
            .ends
            .iter()
            .map(|(session, _)| session.as_str())
            .collect();
        crate::budgets::publish_spent(&write, &ended, now).await?;
    }
    for (session, working_dir, began_at) in &difference.begins {
        let project =
            crate::cost::project_of(&folders, Some(working_dir.to_string_lossy().as_ref()));
        match cockpit_usage::turn_began(&write, session, &project, *began_at).await {
            Ok(_) => {}
            // A report with a time the archive refuses (a wrong clock) must not stop every
            // other session's turns: the turn begins now instead, which is a time it accepts.
            Err(cockpit_usage::Error::Invalid { .. }) => {
                tracing::warn!(
                    session,
                    began_at,
                    "a turn began at a refused time; using now"
                );
                cockpit_usage::turn_began(&write, session, &project, now)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    write.commit().await.map_err(|error| error.to_string())
}

/// Whether the session's agent works now: it runs, and its latest launch's report says working
/// (a report of an earlier launch is the previous agent's).
fn is_working(session: &Session, report: &SessionReport) -> bool {
    session.state == SessionState::Running
        && report.state == Some(AgentState::Working)
        && report.launch == session.launch.as_ref().map(|launch| launch.number)
}

/// The launch's time when `turn` began before the session's current launch: a relaunch (a
/// restart, a wake, a restore) between two passes, with the new agent already working, so the old
/// agent's turn is over and the new one's begins.
fn began_in_an_earlier_launch(session: &Session, turn: &cockpit_usage::OpenTurn) -> Option<i64> {
    session
        .launch
        .as_ref()
        .map(|launch| launch.launched_at)
        .filter(|launched_at| *launched_at > turn.started_at)
}

/// When the open turn of a session still listed ended: with its agent's last report while it
/// runs, else with the session's last change (a stop, a restart, a failure).
fn end_in_live(session: &Session, report: Option<&SessionReport>) -> i64 {
    let current_launch = session.launch.as_ref().map(|launch| launch.number);
    match report {
        Some(report)
            if session.state == SessionState::Running && report.launch == current_launch =>
        {
            report.reported_at
        }
        _ => session.changed_at,
    }
}

/// The last change of a session no longer listed (it ended).
async fn ended_session_change(sessions: &Sessions, id: &str) -> Option<i64> {
    let id = SessionId::parse(id).ok()?;
    sessions
        .get(&id)
        .await
        .ok()
        .map(|session| session.changed_at)
}
