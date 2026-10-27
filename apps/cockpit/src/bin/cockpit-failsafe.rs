//! The cockpit failsafe (cockpit::failsafe): `check` runs once a minute from cockpit-failsafe.timer.

use cockpit::failsafe::{self, Decision, State, Trip, WATCHED};
use cockpit::{paths, sessions, settings, util};
use serde_json::json;
use std::path::Path;
use std::process::{Command, Stdio};

const HELP: &str = "cockpit-failsafe check    sample osd, the sidebar and greyproxy; freeze or resume the sessions
cockpit-failsafe resume   thaw the sessions the failsafe froze, now
cockpit-failsafe status   the samples and the current trip, as JSON";
const NOTICE_ID: &str = "failsafe";
const LOAD_LINES: usize = 3;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let result = match arguments.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["check"] => check(),
        ["resume"] => resume_now(),
        ["status"] => serde_json::to_string_pretty(&State::load(&paths::failsafe_state()))
            .map(|text| println!("{text}")).map_err(|error| error.to_string()),
        _ => {
            eprintln!("{HELP}");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        util::log_as("failsafe", "ERROR", &error);
        eprintln!("cockpit-failsafe: {error}");
        std::process::exit(1);
    }
}

fn check() -> Result<(), String> {
    let path = paths::failsafe_state();
    let mut state = State::load(&path);
    let now = util::now() as i64;
    for watched in WATCHED {
        state.record(watched.role, failsafe::sample_process(Path::new("/proc"), watched.comm, now));
    }
    match state.decide(now) {
        Decision::Nothing => {}
        Decision::Trip { culprit, reason } => trip(&mut state, culprit, reason, now),
        Decision::Resume { why } => resume(&mut state, &why),
    }
    save(&state)
}

fn resume_now() -> Result<(), String> {
    let mut state = State::load(&paths::failsafe_state());
    if state.tripped.is_none() {
        println!("nothing is frozen by the failsafe");
        return Ok(());
    }
    resume(&mut state, "Theo resumed them");
    save(&state)
}

fn save(state: &State) -> Result<(), String> {
    let path = paths::failsafe_state();
    state.save(&path).map_err(|error| format!("cannot save {}: {error}", path.display()))
}

fn trip(state: &mut State, culprit: String, reason: String, now: i64) {
    let frozen = freeze_sessions();
    let history = state.samples.get(&culprit).cloned().unwrap_or_default();
    let brief = failsafe::fixer_brief(&culprit, &reason, &history, &latest_load_lines(), frozen.len());
    let fixer = start_fixer(&brief);
    let fixer_said = match &fixer { Ok(()) => "a fixer session started".to_string(), Err(error) => format!("no fixer session: {error}") };
    util::log_as("failsafe", "WARN", &format!("{culprit} over its limits ({reason}): froze {} sessions, {fixer_said}", frozen.len()));
    util::send_notice(&json!({"id": NOTICE_ID, "title": format!("Cockpit failsafe: {} sessions frozen", frozen.len()),
        "message": format!("{culprit}: {reason}; {fixer_said}. They resume once it is back under its limits; `cockpit-failsafe resume` resumes them now.")}));
    state.tripped = Some(Trip { at: now, culprit, reason, frozen, calm_since: None });
    state.last_tripped_at = Some(now);
}

fn resume(state: &mut State, why: &str) {
    let Some(trip) = state.tripped.take() else { return };
    let stuck: Vec<&String> = trip.frozen.iter().filter(|scope| !systemctl(&["thaw", scope])).collect();
    let message = if stuck.is_empty() { format!("{} sessions resumed: {why}.", trip.frozen.len()) }
                  else { format!("{} sessions resumed, {} could not be thawed ({}): {why}.", trip.frozen.len() - stuck.len(), stuck.len(),
                                 stuck.iter().map(|scope| scope.as_str()).collect::<Vec<_>>().join(", ")) };
    util::log_as("failsafe", if stuck.is_empty() { "INFO" } else { "ERROR" }, &message);
    util::send_notice(&json!({"id": NOTICE_ID, "title": "Cockpit failsafe: sessions resumed", "message": message}));
}

/// Each agent session's systemd scope, frozen the way game mode does it. Not a session waiting on
/// Theo (an approval or a question), and not one game mode froze: its thaw is game mode's.
fn freeze_sessions() -> Vec<String> {
    let game_frozen = std::fs::read_to_string(paths::game_frozen()).unwrap_or_default();
    let mut frozen: Vec<String> = Vec::new();
    for session in sessions::reported() {
        if session.needs_attention() || session.tmux_pane.is_empty() { continue; }
        let Some(scope) = scope_of_pane(&session.tmux_pane) else {
            util::log_as("failsafe", "WARN", &format!("{} not frozen: no systemd scope for its pane {}", session.session, session.tmux_pane));
            continue;
        };
        if game_frozen.lines().any(|line| line == scope) || frozen.contains(&scope) { continue; }
        if systemctl(&["freeze", &scope]) { frozen.push(scope); }
    }
    frozen
}

fn scope_of_pane(pane: &str) -> Option<String> {
    let output = Command::new("tmux").args(["display-message", "-p", "-t", pane, "#{pane_pid}"]).stderr(Stdio::null()).output().ok()?;
    let pid = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) { return None; }
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    cgroup.lines().find_map(|line| line.strip_prefix("0::"))?.rsplit('/').next()
        .filter(|name| name.ends_with(".scope")).map(str::to_string)
}

fn systemctl(arguments: &[&str]) -> bool {
    let output = Command::new("systemctl").arg("--user").args(arguments).stdin(Stdio::null()).output();
    match output {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            util::log_as("failsafe", "WARN", &format!("systemctl --user {} failed: {}", arguments.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
            false
        }
        Err(error) => {
            util::log_as("failsafe", "WARN", &format!("systemctl --user {} did not start: {error}", arguments.join(" ")));
            false
        }
    }
}

fn start_fixer(brief: &str) -> Result<(), String> {
    let agent = settings::launch_agent()?;
    let folder = paths::home().join("IdeaProjects/agent-cockpit");
    let status = Command::new("cockpit").args(["--agent", &agent, "background"]).arg(&folder).arg(brief)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status()
        .map_err(|error| format!("cockpit did not start: {error}"))?;
    if status.success() { Ok(()) } else { Err(format!("cockpit background exited {status}")) }
}

fn latest_load_lines() -> Vec<String> {
    let log = std::fs::read_to_string(paths::log()).unwrap_or_default();
    let mut lines: Vec<String> = log.lines().rev().filter(|line| line.contains("daemon: load over")).take(LOAD_LINES).map(str::to_string).collect();
    lines.reverse();
    lines
}
