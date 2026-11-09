//! Time with the agent, recorded as turns happen (#700, report 18): a session's turn opens when
//! its agent reports `working` and ends when it reports anything else, when its session stops
//! running, or at its end. No source is polled: a pass runs after each commit, compares the
//! sessions' latest reports with the turns `cockpit-usage` holds open, and records the
//! difference. A pass that finds nothing to record takes no write transaction, so it does
//! not wake itself.
//!
//! A turn that begins and ends between two passes is missed; a turn lasts longer than a pass.
//! A session that stopped while its agent worked ends the turn at the session's last change.

use std::collections::BTreeMap;
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

/// What a pass found: turns to open, with the project's folder name resolved later, and turns
/// to end.
struct Difference {
    begins: Vec<(String, std::path::PathBuf, i64)>,
    ends: Vec<(String, i64)>,
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
        begins: Vec::new(),
        ends: Vec::new(),
    };
    for (id, session) in &live {
        let Some(report) = reports.get(id) else {
            continue;
        };
        let is_open = open.iter().any(|turn| &turn.session == id);
        if is_working(session, report) && !is_open {
            difference
                .begins
                .push((id.clone(), session.working_dir.clone(), report.reported_at));
        }
    }
    for turn in &open {
        let session = live.get(&turn.session);
        let report = reports.get(&turn.session);
        let still_working = matches!((session, report), (Some(session), Some(report)) if is_working(session, report));
        if still_working {
            continue;
        }
        let ended_at = match session {
            Some(session) => end_in_live(session, report),
            None => ended_session_change(sessions, &turn.session)
                .await
                .unwrap_or(now),
        };
        difference.ends.push((turn.session.clone(), ended_at));
    }
    if difference.begins.is_empty() && difference.ends.is_empty() {
        return Ok(());
    }

    let folders = crate::cost::project_folders(database).await?;
    let write = database.write().await.map_err(|error| error.to_string())?;
    for (session, working_dir, began_at) in &difference.begins {
        let project =
            crate::cost::project_of(&folders, Some(working_dir.to_string_lossy().as_ref()));
        cockpit_usage::turn_began(&write, session, &project, *began_at)
            .await
            .map_err(|error| error.to_string())?;
    }
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
    write.commit().await.map_err(|error| error.to_string())
}

/// Whether the session's agent works now: it runs, and its latest launch's report says working
/// (a report of an earlier launch is the previous agent's).
fn is_working(session: &Session, report: &SessionReport) -> bool {
    session.state == SessionState::Running
        && report.state == Some(AgentState::Working)
        && report.launch == session.launch.as_ref().map(|launch| launch.number)
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
