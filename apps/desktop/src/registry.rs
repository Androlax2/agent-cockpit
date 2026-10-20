//! The Claude session registry behind the OSD daemon (no GTK here).
//!
//! Every live Claude Code session, keyed by session id, fed by `session <json>` reports from
//! ~/.claude/hooks/session-state.sh and mirrored to:
//!   - $XDG_RUNTIME_DIR/claude-sessions.json for the waybar chip (RTMIN+16) and claude-sessions;
//!   - Hyprland, through `hyprctl eval CLAUDE_WINDOW_STATE(pid, state)` (apps/claude.lua), which
//!     colours the session's terminal border by state;
//!   - the OSD banners (the `Banners` given at construction).
//!
//! A report carries session, project, usually a state (working | waiting | needs-you | gone; absent
//! for an informational event that leaves the state as it is), and optionally cwd, task, message,
//! sticky and the focus target of the session's window: kitty_pid, kitty_window, kitty_listen,
//! tmux_pane. The kitty fields are only a fallback: claude-auto-retry keeps a session alive in tmux
//! when its kitty window closes, so the registry asks tmux which client is attached to the pane and
//! reads that client's kitty identity instead.
//!
//! Hooks run inside greywall with no Hyprland or tmux socket, so every tmux and Hyprland call
//! happens here, through `System` (tests give a fake one). Hooks also mislead or go missing, so
//! every few seconds the pane's last lines are read and win: `pane_state` recognises Claude's
//! permission dialog, choice menus, spinner and idle prompt. Also the run and guide queues
//! (ask-to-run, ask-to-guide).

use cockpit::agents::{self, Feature};
use cockpit::paths;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const PRUNE_INTERVAL_S: u32 = 2;
const RECENT_SESSIONS_DAYS: f64 = 7.0;
const RECENT_SESSIONS_WRITE_S: f64 = 60.0;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const TARGET_FIELDS: [&str; 4] = ["kitty_pid", "kitty_window", "kitty_listen", "tmux_pane"];
const CLAUDE_WORKSPACE: i64 = 6;
const PANE_TAIL_LINES: usize = 20;
const MAX_TEXT_LENGTH: usize = 300;
const REQUEST_MAX_LENGTH: usize = 140;
const FULL_REQUEST_MAX_LENGTH: usize = 16384;
const DIALOG_CAPTURE_LINES: usize = 2000;
const FINISHED_MESSAGE: &str = "Finished, waiting for you"; // the hook's Stop message
const RUN_PHASES: [&str; 4] = ["asking", "running", "input", "done"];
/// ask-to-run touches <id>.waiting every few seconds while it waits; this long without it, nobody
/// waits for the answer any more (the session gave up, timed out without --wait, or died). Long
/// enough for a session that timed out to do something else before it waits again.
const ABANDONED_AFTER: Duration = Duration::from_secs(5 * 60);

fn regex(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap()
}

// What Claude Code prints at the bottom of its pane: while busy, while a dialog is open, and the
// input prompt line it shows when idle (also shown while busy, so busy wins over it).
static BUSY_SPINNER: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^\s*\S{0,2}\s*[A-Z][a-z]+… \(\d+[smh]"));
static BUSY_INTERRUPT: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^\s*(?:esc|enter) to interrupt(?: and send)?(?:\s*·.*)?$"));
/// The permission dialog: its question alone on a line, then its first option within a few lines.
static PERMISSION_DIALOG: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^[\s│]*Do you want to proceed\?[\s│]*$(?:\n.*){0,3}?\n[\s│❯]*1\.\s*Yes"));
/// Any of Claude's numbered choice menus with the cursor on an option: "❯ 2. Option". Like the
/// idle prompt it starts with ❯, so it is checked first.
static CHOICE_MENU: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^[\s│]*❯[\s\x{a0}]*\d+\.\s+\S"));
/// Claude pads its prompt with a no-break space.
static PROMPT_LINE: LazyLock<Regex> = LazyLock::new(|| regex(r"(?m)^\s*❯[ \x{a0}]"));
static UPDATE_READY: LazyLock<Regex> = LazyLock::new(|| regex(r"Update installed\s*·\s*Restart to update"));
/// The session binary's process name: Claude Code's version.
static CLAUDE_VERSION_NAME: LazyLock<Regex> = LazyLock::new(|| regex(r"^\d+\.\d+\.\d+$"));
static TMUX_PANE: LazyLock<Regex> = LazyLock::new(|| regex(r"^%[0-9]{1,10}$"));
static KITTY_LISTEN: LazyLock<Regex> = LazyLock::new(|| regex(r"^unix:[@/][A-Za-z0-9_./@-]{1,200}$"));
static NUMBER: LazyLock<Regex> = LazyLock::new(|| regex(r"^[0-9]{1,10}$"));
pub static NOTICE_ID: LazyLock<Regex> = LazyLock::new(|| regex(r"^[a-z][a-z0-9:-]{0,63}$"));
static CRAFT_APP: LazyLock<Regex> = LazyLock::new(|| regex(r"^[a-z]{1,20}craft:(on|off)$"));
pub static RUN_ID: LazyLock<Regex> = LazyLock::new(|| regex(r"^[0-9a-f]{12}$"));
static PROCEED_LINE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[\s│]*Do you want to proceed\?[\s│]*$"));
static BOX_LINE: LazyLock<Regex> = LazyLock::new(|| regex(r"^[\s│]*[╭╰─┌└━]"));
static KHAL_EVENT: LazyLock<Regex> = LazyLock::new(|| regex(r"^(\d{2})/(\d{2})/(\d{4}) (\d{2}):(\d{2})\|(\d{2})/(\d{2})/(\d{4}) (\d{2}):(\d{2})\|"));
/// Games, as in hypr/scripts/is-gaming.sh (GAME_CLASS_REGEX): keep both lists the same.
static GAME_CLASS: LazyLock<Regex> = LazyLock::new(|| regex(r"^(steam_app_[0-9]+|Star Citizen|Dofus\.x64|retroarch)$"));

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn urgency(state: &str) -> i32 {
    match state { "working" => 0, "waiting" => 1, "needs-you" => 2, _ => -1 }
}

/// To the journal (osd.service), and to the log the session manager shares with the column.
pub fn log(message: &str) {
    eprintln!("osd: {message}");
    cockpit::util::log_as("daemon", "INFO", message);
}

// ---- Reports ----

/// The focus fields end up in shell arguments and a Lua string, so they are strict.
fn validate_focus_target(fields: &Map<String, Value>) -> Result<(), String> {
    let text = |key: &str| fields.get(key).map(|value| match value { Value::String(s) => s.clone(), other => other.to_string() });
    for key in ["kitty_pid", "kitty_window"] {
        if text(key).is_some_and(|value| !NUMBER.is_match(&value)) {
            return Err(format!("{key} is not a number"));
        }
    }
    if text("kitty_listen").is_some_and(|value| !KITTY_LISTEN.is_match(&value)) {
        return Err("kitty_listen is not a unix socket address".into());
    }
    if text("tmux_pane").is_some_and(|value| !TMUX_PANE.is_match(&value)) {
        return Err("tmux_pane is not a pane id".into());
    }
    Ok(())
}

/// Session text reaches tmux and the session manager, where #( and #{ are live tmux formats.
pub fn validate_text(key: &str, value: &str) -> Result<(), String> {
    if value.chars().count() > MAX_TEXT_LENGTH {
        return Err(format!("session.{key} is too long"));
    }
    if value.chars().any(|c| (c as u32) < 32 || c as u32 == 127) {
        return Err(format!("session.{key} contains control characters"));
    }
    if value.contains("#(") || value.contains("#{") {
        return Err(format!("session.{key} contains a tmux format sequence"));
    }
    Ok(())
}

/// A hook's report, checked.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub session: String,
    pub project: String,
    pub state: Option<String>,
    pub cwd: Option<String>,
    pub task: Option<String>,
    pub label: Option<String>,
    pub message: Option<String>,
    /// What the turn left in the repository, from the hook (it runs git inside the sandbox).
    pub outcome: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub turn_id: Option<String>,
    pub turn_complete: bool,
    pub sticky: bool,
    /// An explicit blocking chat question, independent of the agent's on-screen dialog state.
    pub needs_input: Option<bool>,
    /// Started with "this terminal" (the claude function's t): the cockpit shows it but leaves it out
    /// of what acts on every session (restart-all, the reboot restore, the queue, the hand-off).
    pub terminal: bool,
    /// Billed to the Console's API credits, not the subscription (the cockpit's $). None when the
    /// report did not say (the daemon's own updates).
    pub credits: Option<bool>,
    /// Commits of the session's repository on no remote yet (the hook counts them in the sandbox).
    pub unpushed: Option<u64>,
    /// Background agents and other background work (shells, monitors, workflows) still in flight
    /// when the turn ended. None when the report did not say (only Stop and a fresh start do).
    pub agents: Option<u64>,
    pub background: Option<u64>,
    /// The desktop apps the session drives through an attached MCP server: "figcraft:on wordcraft:off"
    /// (the hook checks inside the sandbox). None when the report did not say (the daemon's own updates).
    pub apps: Option<String>,
    /// Which agent runs the session (cockpit::agents). None when the report did not say: Claude.
    pub agent: Option<String>,
    pub native_thread: Option<String>,
    /// Set only by the daemon from the sender's launcher incarnation, never parsed from a report.
    pub runtime: Option<String>,
    pub target: BTreeMap<String, String>,
}

fn native_thread_of(report: &Map<String, Value>) -> Result<Option<String>, String> {
    let session = report.get("session").and_then(Value::as_str).unwrap_or("");
    if report.get("agent").and_then(Value::as_str) == Some("codex") || session.starts_with("cdx_") {
        if report.get("agent").and_then(Value::as_str) != Some("codex") {
            return Err("native Codex reports require an explicit codex agent".into());
        }
        let native = report.get("native_thread").and_then(Value::as_str).ok_or("native Codex report has no native thread")?;
        if cockpit::codex::cockpit_id(native)? != session {
            return Err("native Codex thread does not match its cockpit id".into());
        }
        return Ok(Some(native.to_string()));
    }
    if report.contains_key("native_thread") {
        return Err("only native Codex reports may name a native thread".into());
    }
    Ok(None)
}

pub fn parse_session(raw: &str) -> Result<Report, String> {
    let value: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let report = value.as_object().ok_or("session report is not an object")?;
    for key in ["session", "project"] {
        if !report.get(key).and_then(Value::as_str).is_some_and(|text| !text.is_empty()) {
            return Err(format!("session.{key} missing"));
        }
    }
    if let Some(state) = report.get("state") {
        let state = state.as_str().unwrap_or("");
        if urgency(state) < 0 && state != "gone" {
            return Err(format!("session.state {state:?} unknown"));
        }
    }
    if let Some(apps) = report.get("apps") {
        let valid = apps.as_str().is_some_and(|apps| apps.len() <= 200 && apps.split_whitespace().all(|app| CRAFT_APP.is_match(app)));
        if !valid {
            return Err("session.apps is not a list of app:on|off".into());
        }
    }
    if report.get("credits").is_some_and(|value| !value.is_boolean()) {
        return Err("session.credits is not true or false".into());
    }
    if report.get("needs_input").is_some_and(|value| !value.is_boolean()) {
        return Err("session.needs_input is not true or false".into());
    }
    if report.get("turn_complete").is_some_and(|value| !value.is_boolean()) {
        return Err("session.turn_complete is not true or false".into());
    }
    if let Some(id) = report.get("turn_id") {
        if !id.as_str().is_some_and(cockpit::sessions::valid_turn_id) {
            return Err("session.turn_id is not a 32-character hexadecimal id".into());
        }
    }
    let turn_complete = report.get("turn_complete").and_then(Value::as_bool).unwrap_or(false);
    if turn_complete && !report.contains_key("turn_id") {
        return Err("session completion requires turn_id".into());
    }
    if report.get("repository").is_some_and(|value| !value.as_str().is_some_and(|repository|
        repository.is_empty() || cockpit::sessions::valid_repository(repository))) {
        return Err("session.repository is not a canonical GitHub repository".into());
    }
    if report.get("agent").is_some_and(|agent| !agent.as_str().is_some_and(|agent| agents::find(agent).is_some())) {
        return Err(format!("session.agent is not one of {:?}", agents::names()));
    }
    for key in ["unpushed", "agents", "background"] {
        if report.get(key).is_some_and(|value| !value.is_u64()) {
            return Err(format!("session.{key} is not a count"));
        }
    }
    for key in ["cwd", "task", "message", "outcome", "label", "branch"] {
        if report.get(key).is_some_and(|value| !value.is_string()) {
            return Err(format!("session.{key} is not a string"));
        }
    }
    let session = report["session"].as_str().unwrap().to_string();
    if !agents::is_session_id(&session) {
        return Err(format!("session is not an id of one of the agents {:?}", agents::names()));
    }
    for key in ["session", "project", "cwd", "task", "message", "outcome", "label", "branch"] {
        if let Some(text) = report.get(key).and_then(Value::as_str) {
            validate_text(key, text)?;
        }
    }
    validate_focus_target(report)?;
    let native_thread = native_thread_of(report)?;
    let text = |key: &str| report.get(key).and_then(Value::as_str).map(str::to_string);
    let target = TARGET_FIELDS.iter().filter_map(|key| {
        report.get(*key).map(|value| (key.to_string(), match value { Value::String(s) => s.clone(), other => other.to_string() }))
    }).collect();
    Ok(Report {
        session, project: report["project"].as_str().unwrap().to_string(), state: text("state"), cwd: text("cwd"),
        task: text("task"), message: text("message"), outcome: text("outcome"), sticky: report.get("sticky").is_some_and(|s| s.as_bool().unwrap_or(!s.is_null())),
        repository: text("repository"), branch: text("branch"), turn_id: text("turn_id"), turn_complete,
        label: text("label").map(|label| cockpit::sessions::validate_task_label(&label)).transpose()?,
        terminal: report.get("terminal").and_then(Value::as_bool).unwrap_or(false),
        credits: report.get("credits").and_then(Value::as_bool),
        needs_input: report.get("needs_input").and_then(Value::as_bool),
        unpushed: report.get("unpushed").and_then(Value::as_u64),
        agents: report.get("agents").and_then(Value::as_u64),
        background: report.get("background").and_then(Value::as_u64),
        apps: report.get("apps").and_then(Value::as_str).map(str::to_string),
        agent: report.get("agent").and_then(Value::as_str).map(str::to_string), native_thread, runtime: None, target,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: String,
    pub title: String,
    pub message: String,
}

/// notice {"id", "title", "message"}: a banner that is not a session (the column's budget alerts).
pub fn parse_notice(raw: &str) -> Result<Notice, String> {
    let value: Value = serde_json::from_str(raw).map_err(|error| format!("not JSON: {error}"))?;
    let notice = value.as_object().ok_or("not an object")?;
    let id = notice.get("id").and_then(Value::as_str).filter(|id| NOTICE_ID.is_match(id)).ok_or("bad id")?;
    let mut texts = Vec::new();
    for key in ["title", "message"] {
        let text = notice.get(key).and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or(format!("bad {key}"))?;
        validate_text(key, text)?;
        texts.push(text.to_string());
    }
    Ok(Notice { id: id.into(), title: texts[0].clone(), message: texts[1].clone() })
}

// ---- Panes ----

/// needs-you on a permission dialog or a choice menu, working on the spinner, waiting at the idle
/// prompt, else None.
pub fn pane_state(tail: &str) -> Option<&'static str> {
    if PERMISSION_DIALOG.is_match(tail) || CHOICE_MENU.is_match(tail) {
        Some("needs-you")
    } else if BUSY_SPINNER.is_match(tail) || BUSY_INTERRUPT.is_match(tail) {
        Some("working")
    } else if PROMPT_LINE.is_match(tail) {
        Some("waiting")
    } else {
        None
    }
}

/// What a permission dialog asks, from the pane: the lines above "Do you want to proceed?" up to
/// the dialog's top edge, such as "Bash command · npm test". Empty when there is no dialog.
pub fn permission_request(tail: &str) -> String {
    let lines: Vec<&str> = tail.lines().collect();
    let Some(ask) = (0..lines.len()).rev().find(|index| PROCEED_LINE.is_match(lines[*index])) else { return String::new() };
    let mut block: Vec<String> = Vec::new();
    for line in lines[ask.saturating_sub(10)..ask].iter().rev() {
        if BOX_LINE.is_match(line) {
            break;
        }
        let text = line.trim().trim_matches('│').trim();
        if !text.is_empty() {
            block.insert(0, text.to_string());
        }
    }
    let request: String = block.iter().take(2).cloned().collect::<Vec<_>>().join(" · ")
        .chars().filter(|c| *c as u32 >= 32 && *c as u32 != 127).collect();
    request.replace("#{", "#\u{200b}{").replace("#(", "#\u{200b}(").chars().take(REQUEST_MAX_LENGTH).collect()
}

fn permission_request_full(tail: &str) -> String {
    let lines: Vec<_> = tail.lines().collect();
    let Some(ask) = (0..lines.len()).rev().find(|index| PROCEED_LINE.is_match(lines[*index])) else { return String::new() };
    let edge = (0..ask).rev().find(|index| lines[*index].trim_start().starts_with(['╭', '┌']));
    let mut request = if edge.is_none() { "Captured dialog context (the opening boundary was not found):\n".into() } else { String::new() };
    for line in &lines[edge.map_or(0, |edge| edge + 1)..ask] {
        let frame = line.trim_start();
        let text = if let Some(content) = frame.strip_prefix('│') {
            let content = content.trim_end().strip_suffix('│').unwrap_or(content.trim_end());
            content.strip_prefix(' ').unwrap_or(content)
        } else { line };
        request.extend(text.chars().filter(|character| !character.is_control()));
        request.push('\n');
    }
    request = request.replace("#{", "#\u{200b}{").replace("#(", "#\u{200b}(");
    if request.chars().count() > FULL_REQUEST_MAX_LENGTH {
        let note = "\n[Capture limit reached; inspect the session for the remaining request.]";
        request = request.chars().take(FULL_REQUEST_MAX_LENGTH - note.chars().count()).collect::<String>() + note;
    }
    request.trim_end().into()
}

/// A timed khal event under way now (or starting within the minute). All-day events (a bare date)
/// do not count: a holiday is not a meeting. khal prints dd/mm/yyyy.
pub fn events_now(khal_output: &str, now: f64) -> bool {
    use chrono::{Local, TimeZone};
    khal_output.lines().filter_map(|line| KHAL_EVENT.captures(line)).any(|found| {
        let number = |index: usize| found[index].parse::<u32>().unwrap_or(0);
        let moment = |day: usize| Local.with_ymd_and_hms(number(day + 2) as i32, number(day + 1), number(day), number(day + 3), number(day + 4), 0)
            .single().map(|t| t.timestamp() as f64);
        match (moment(1), moment(6)) {
            (Some(start), Some(end)) => start <= now + 60.0 && end > now,
            _ => false,
        }
    })
}

// ---- What the registry asks of the system ----

/// A window the daemon opened (kitty); finished once its process ended.
pub trait Window {
    fn running(&mut self) -> bool;
    fn terminate(&mut self);
}

impl Window for Child {
    fn running(&mut self) -> bool {
        matches!(self.try_wait(), Ok(None))
    }
    fn terminate(&mut self) {
        let _ = self.kill();
        let _ = self.wait();
    }
}

/// Everything outside the registry: commands, tmux, Hyprland, the process table, the calendar.
pub trait System {
    /// A command; false (and logged) when it fails or exits with another code than `ok`.
    fn run(&mut self, command: &[&str], ok: &[i32]) -> bool;
    fn tmux(&mut self, arguments: &[&str]) -> Option<String>;
    fn open_window(&mut self, command: &[&str]) -> Option<Box<dyn Window>>;
    fn claude_workspace_active(&mut self) -> bool;
    fn gaming(&mut self) -> bool;
    fn in_meeting(&mut self) -> bool;
    fn live_session_ids(&mut self) -> HashSet<String>;
    fn claude_panes(&mut self) -> HashSet<String>;
    fn process_environment(&mut self, pid: &str) -> HashMap<String, String>;
    fn process_alive(&mut self, pid: &str) -> bool;
    /// trusted_target for the process that sent a report.
    fn sender_target(&mut self, sender: u32) -> Option<BTreeMap<String, String>>;
    /// sandbox_directory for the process that sent a report.
    fn sender_directory(&mut self, sender: u32) -> Option<String>;
    fn sender_runtime(&mut self, _sender: u32) -> Option<String> { None }
    fn runtime_is_live(&mut self, _runtime: &str) -> bool { false }
    fn quiet(&mut self) -> bool {
        self.in_meeting() || self.gaming()
    }
}

/// The OSD banners: one per session, and notices of their own.
pub trait Banners {
    fn raise_banner(&mut self, banner: &Banner);
    fn raise_notice(&mut self, notice: &Notice);
    fn drop_banner(&mut self, key: &str);
    fn showing(&self, key: &str) -> bool;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Banner {
    pub session: String,
    pub title: String,
    pub message: String,
    pub sticky: bool,
}

/// The repository a claude-sessions clone (<clones>/<repository>/<name>) was made from, or "".
pub fn parallel_clone_repository(cwd: &str) -> String {
    match Path::new(cwd).strip_prefix(paths::clones_dir()) {
        Ok(relative) => {
            let parts: Vec<_> = relative.components().collect();
            if parts.len() >= 2 { parts[0].as_os_str().to_string_lossy().into_owned() } else { String::new() }
        }
        Err(_) => String::new(),
    }
}

fn basename(path: &str) -> String {
    Path::new(path.trim_end_matches('/')).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

// ---- The run and guide queues ----

pub fn runs_dir() -> PathBuf {
    std::env::var("CLAUDE_RUNS_DIR").map(PathBuf::from).unwrap_or_else(|_| paths::home().join(".cache/claude/runs"))
}

/// The project of a held request: the daemon set it from the folder the sender's sandbox started in.
fn project_from_request(path: &Path) -> String {
    fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|request| request.get("project").and_then(Value::as_str).map(str::to_string)).unwrap_or_else(|| "?".into())
}

/// The answers folder name of a sandbox started in `started_in`: the launcher's CLAUDE_ANSWERS_KEY,
/// the path of the repository's top folder (or of the folder itself, outside one) with every
/// character but a letter or a digit turned into "-". The .git entry is only looked at, never run.
pub fn answers_key_of(started_in: &Path) -> String {
    let root = started_in.ancestors().find(|folder| fs::symlink_metadata(folder.join(".git")).is_ok()).unwrap_or(started_in);
    root.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Requests one sandbox folder can have waiting for Theo: more is a flood (security audit M5).
const WAITING_PER_FOLDER_MAX: usize = 3;
const TODO_UPDATE_MAX_BYTES: usize = 65_536;
const TODO_QUEUE_MAX_BYTES: u64 = 1 << 20;

/// A claude-todo change as a sandbox sent it on the report socket, queued for the column
/// (cockpit::todo::Todo::apply_updates) in the runtime directory, where no sandbox reaches, with the
/// folder its sandbox started in: that folder, never one the update names, decides the project
/// (security audit 2026-10-19, M4). The queue has a size cap: a session cannot flood the list.
pub fn queue_todo_update(raw: &str, started_in: &str) -> Result<(), String> {
    use std::io::Write;
    if raw.len() > TODO_UPDATE_MAX_BYTES {
        return Err(format!("longer than {TODO_UPDATE_MAX_BYTES} bytes"));
    }
    let mut update: Map<String, Value> = match serde_json::from_str(raw) {
        Ok(Value::Object(update)) => update,
        _ => return Err("not a JSON object".into()),
    };
    update.insert("started_in".into(), json!(started_in));
    let path = paths::todo_updates();
    if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > TODO_QUEUE_MAX_BYTES) {
        return Err("the todo queue is full: the column has not applied it (is the cockpit running?)".into());
    }
    let mut file = fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|error| format!("cannot queue it: {error}"))?;
    writeln!(file, "{}", Value::Object(update)).map_err(|error| format!("cannot queue it: {error}"))
}

/// A run or guide request as a sandbox sent it on the report socket (`kind` "run-request" or
/// "guide-request"), kept as the daemon's own copy (cockpit::approval::held_request) before anything
/// else: the approval and the guide window read only that copy, which no sandbox reaches, so no other
/// session can rewrite it before Theo reads it (security audit 2026-10-19, H3). The copy carries
/// what the sandbox cannot choose: the folder its sandbox started in, that folder's project, and the
/// answers folder of that project.
pub fn hold_request(cache: &mut HashMap<String, String>, kind: &str, raw: &str, started_in: &str, verified_session: Option<&str>) -> Result<String, String> {
    let extension = match kind {
        "run-request" => "request",
        "guide-request" => "guide",
        _ => return Err(format!("not a request kind: {kind}")),
    };
    let mut request: Map<String, Value> = match serde_json::from_str(raw) {
        Ok(Value::Object(request)) => request,
        _ => return Err("not a JSON object".into()),
    };
    let id = request.get("id").and_then(Value::as_str).filter(|id| RUN_ID.is_match(id)).ok_or("no valid id")?.to_string();
    let answers = answers_key_of(Path::new(started_in));
    // ask-to-run waits on the folder the launcher named: a different one here would leave it waiting.
    if request.get("answers").and_then(Value::as_str) != Some(answers.as_str()) {
        return Err(format!("its answers folder is not {answers}, the one of the folder its sandbox started in (restart the session)"));
    }
    let path = cockpit::approval::held_request(&id, extension);
    if path.exists() {
        return Err(format!("{id} is already filed"));
    }
    request.insert("started_in".into(), json!(started_in));
    request.insert("project".into(), json!(project_of(cache, started_in, "?")));
    request.insert("answers".into(), json!(answers));
    request.remove("verified_session");
    if let Some(session) = verified_session { request.insert("verified_session".into(), json!(session)); }
    cockpit::util::write_json(&path, &Value::Object(request)).map_err(|error| format!("cannot keep {}: {error}", path.display()))?;
    Ok(id)
}

/// The git directory shared by every checkout of the repository `directory` is in (a worktree's
/// .git file points into the main one), found by reading the .git entries as files: git itself must
/// never run on a session's folder out here, its configuration is the sandbox's to write
/// (core.fsmonitor, filters) and would run as Theo.
fn common_git_dir(directory: &Path) -> Option<PathBuf> {
    for folder in directory.ancestors() {
        let entry = folder.join(".git");
        let Ok(metadata) = fs::symlink_metadata(&entry) else { continue };
        if metadata.is_dir() {
            return Some(entry);
        }
        let text = cockpit::util::read_untrusted_text(&entry, 4096).ok()?;
        let pointed = folder.join(text.trim().strip_prefix("gitdir:")?.trim());
        // A worktree's own git directory names the shared one in its commondir file.
        return Some(match cockpit::util::read_untrusted_text(&pointed.join("commondir"), 4096) {
            Ok(common) => pointed.join(common.trim()),
            Err(_) => pointed,
        });
    }
    None
}

fn normalized(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => { clean.pop(); }
            std::path::Component::CurDir => {}
            other => clean.push(other),
        }
    }
    clean
}

/// The repository a directory belongs to, named after its main checkout: a session that moves into
/// a subfolder or runs in a worktree keeps the project's own name. Outside a repository, the
/// directory's name; without a directory, the fallback.
pub fn project_of(cache: &mut HashMap<String, String>, cwd: &str, fallback: &str) -> String {
    if cwd.is_empty() {
        return fallback.into();
    }
    let clone = parallel_clone_repository(cwd);
    if !clone.is_empty() {
        return clone;
    }
    if let Some(known) = cache.get(cwd) {
        return known.clone();
    }
    let project = match common_git_dir(Path::new(cwd)).map(|common| normalized(&common)) {
        Some(common) if common.file_name().is_some_and(|name| name == ".git") => common.parent().map(|main| basename(&main.to_string_lossy())).unwrap_or_default(),
        Some(common) => basename(&common.to_string_lossy()).trim_end_matches(".git").to_string(),
        None => basename(cwd),
    };
    // The name comes from files a session writes (its .git file can point anywhere): it reaches
    // banners, the column, tmux options and prompts, so it obeys the rules of reported text.
    let project = if project.is_empty() || validate_text("project", &project).is_err() || project.chars().any(|c| ('\u{80}'..='\u{9f}').contains(&c)) {
        fallback.to_string()
    } else {
        project
    };
    cache.insert(cwd.into(), project.clone());
    project
}

/// Commands Claude asks Theo to run outside the sandbox (ask-to-run). The daemon runs nothing
/// itself: each request immediately gets claude-run-approve in a tmux session of its own
/// (claude-run-<id>), shown in a pane under the cockpit's session pane. Floating windows take turns
/// on the current workspace while a request needs Theo: before he answers, and again when the command he approved
/// waits for input (sudo, a passphrase), then until it ends (what the answer does, the next prompt);
/// closed while a command that never asked for input runs, and never while he games.
#[derive(Default)]
pub struct RunRequests {
    /// Approvals already available in their panes, waiting their turn for the floating window.
    pub queue: Vec<String>,
    /// The run that has Theo's attention: its approval asks him, or its command waits for input.
    pub current: String,
    /// Commands Theo approved that run on without needing him: they leave `current`, so the next
    /// request is shown at once instead of after their end (several sessions, a project's parallel
    /// clones, can each have one running). Each keeps its tmux session and its approval pane.
    pub running: Vec<String>,
    /// Running commands that then asked for input while another run had Theo's attention: next in
    /// line for it, before the queue.
    waiting_input: Vec<String>,
    pub phase: String,
    /// The current command asked for input once: Theo is following it, the window stays to its end.
    asked_input: bool,
    /// Theo closed the window himself: it stays closed until the command asks him something new.
    dismissed: bool,
    window: Option<Box<dyn Window>>,
}

fn run_session(id: &str) -> String {
    format!("claude-run-{id}")
}

/// Whether nobody waits for this request any more. Its age counts from the last heartbeat, or from
/// its filing when ask-to-run has not written one yet; with neither file there is nothing to judge
/// by, and the request is kept.
fn is_abandoned(id: &str) -> bool {
    let touched = |path: PathBuf| fs::metadata(path).and_then(|metadata| metadata.modified()).ok();
    touched(runs_dir().join(format!("{id}.waiting"))).or_else(|| touched(cockpit::approval::held_request(id, "request")))
        .and_then(|time| time.elapsed().ok())
        .is_some_and(|age| age > ABANDONED_AFTER)
}

/// The answers folder a held run or guide request names (cockpit::approval::request_answers_dir);
/// None when the request is gone.
fn answers_named_by(request: &Path, kind: &str) -> Option<PathBuf> {
    let request: Value = fs::read_to_string(request).ok().and_then(|text| serde_json::from_str(&text).ok())?;
    cockpit::approval::request_answers_dir(&request, kind)
}

fn run_result(id: &str) -> Option<PathBuf> {
    answers_named_by(&cockpit::approval::held_request(id, "request"), "runs").map(|answers| answers.join(format!("{id}.result")))
}

/// A report the daemon refused or corrected; `reason` says why.
fn audit_report(report: &Report, sender: u32, event: &str, outcome: &str, reason: &str) {
    cockpit::audit::record("daemon", event, outcome, json!({"claimed_session": report.session, "claimed_project": report.project,
                                                           "sender_pid": sender, "reason": reason}));
}

/// A run or guide request or a todo update from the report socket, held or refused. What the
/// sandbox wrote goes in as claimed_* or as a fingerprint (the command, a title, a todo text).
fn audit_request(kind: &str, raw: &str, started_in: Option<&str>, sender: u32, result: &Result<(), String>) {
    let request: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let text = |key: &str| request.get(key).and_then(Value::as_str).map(cockpit::audit::fingerprint);
    let (event, held) = match kind {
        "run-request" => ("run", "held"),
        "guide-request" => ("guide", "held"),
        _ => ("todo", "queued"),
    };
    let mut fields = json!({"started_in": started_in, "sender_pid": sender, "request": cockpit::audit::fingerprint(raw),
                            "claimed_id": request.get("id"), "claimed_session": request.get("session"), "claimed_cwd": request.get("cwd")});
    for (key, value) in [("command", text("command")), ("title", text("title")), ("text", text("text"))] {
        if let Some(value) = value {
            fields[key] = value;
        }
    }
    if let Some(action) = request.get("action") {
        fields["action"] = action.clone();
    }
    match result {
        Ok(()) => cockpit::audit::record("daemon", &format!("{event}.{held}"), "ok", fields),
        Err(reason) => {
            fields["reason"] = json!(reason);
            cockpit::audit::record("daemon", &format!("{event}.refused"), "refused", fields);
        }
    }
}

/// The answer ask-to-run --wait finds for a dropped request.
fn write_abandoned(id: &str) {
    cockpit::audit::record("daemon", "run.abandoned", "dropped", json!({"id": id, "reason": "nobody waited for its answer any more"}));
    let Some(path) = run_result(id) else {
        log(&format!("run request {id} dropped without an answer: it names no answers folder"));
        return;
    };
    if path.exists() {
        return;
    }
    if let Err(error) = cockpit::util::write_json(&path, &json!({"status": "abandoned"})) {
        log(&format!("cannot drop request {id}: {error}"));
    }
}

impl RunRequests {
    fn session_phases(&self) -> HashMap<String, &'static str> {
        let mut phases = HashMap::new();
        let current_phase = match self.phase.as_str() {
            "asking" => "approval",
            "input" => "input",
            "running" => "running",
            _ => "",
        };
        // Later entries win: input and approval need Theo before a command running on does.
        let requests = self.running.iter().map(|id| (id, "running"))
            .chain((current_phase == "running").then_some((&self.current, current_phase)))
            .chain(self.queue.iter().map(|id| (id, "approval")))
            .chain((!self.current.is_empty() && current_phase != "running").then_some((&self.current, current_phase)))
            .chain(self.waiting_input.iter().map(|id| (id, "input")));
        for (id, phase) in requests {
            let Some(session) = cockpit::util::read_json(&cockpit::approval::held_request(id, "request"))
                .and_then(|request| request.get("session")?.as_str().map(str::to_string)) else { continue };
            phases.insert(session, phase);
        }
        phases
    }

    pub fn request(&mut self, system: &mut dyn System, banners: &mut dyn Banners, id: &str) -> Result<(), String> {
        if !RUN_ID.is_match(id) {
            return Err("not a request id".into());
        }
        if self.queue.iter().any(|queued| queued == id) || self.current == id || self.running.iter().any(|run| run == id) {
            return Ok(());
        }
        let Some(result) = run_result(id) else {
            log(&format!("run request {id} dropped: it names no answers folder (its session started before answers went per project)"));
            return Ok(());
        };
        if result.exists() {
            return Ok(());
        }
        if is_abandoned(id) {
            write_abandoned(id);
            return Ok(());
        }
        let approve = paths::home().join(".local/bin/claude-run-approve").to_string_lossy().into_owned();
        if !system.run(&["tmux", "new-session", "-d", "-s", &run_session(id), "-x", "180", "-y", "45", &approve, id], &[0]) {
            return Err(format!("cannot start the approval for {id}"));
        }
        system.run(&["claude-sessions", "approval-pane", id], &[0]);
        let project = project_from_request(&cockpit::approval::held_request(id, "request"));
        if !system.quiet() {
            banners.raise_notice(&Notice { id: format!("run-{id}"), title: format!("Claude · {project}"), message: "asks you to run a command".into() });
        }
        log(&format!("run request {id} from {project}"));
        self.queue.push(id.into());
        self.next(system);
        Ok(())
    }

    fn next(&mut self, system: &mut dyn System) {
        if !self.current.is_empty() {
            return;
        }
        if !self.waiting_input.is_empty() {
            let id = self.waiting_input.remove(0);
            self.attend(system, &id, "input");
            return;
        }
        while !self.queue.is_empty() {
            let id = self.queue.remove(0);
            let abandoned = is_abandoned(&id);
            if abandoned || run_result(&id).is_none_or(|result| result.exists()) {
                if abandoned {
                    write_abandoned(&id);
                }
                system.run(&["tmux", "kill-session", "-t", &format!("={}", run_session(&id))], &[0, 1]);
                system.run(&["claude-sessions", "approval-pane", "close", &id], &[0]);
                continue;
            }
            self.current = id.clone();
            self.phase = "asking".into();
            self.asked_input = false;
            self.dismissed = false;
            self.show(system);
            return;
        }
    }

    /// A running command (one of `running`) back in Theo's attention: it asks for input.
    fn attend(&mut self, system: &mut dyn System, id: &str, phase: &str) {
        self.running.retain(|run| run != id);
        self.current = id.into();
        self.phase = phase.into();
        self.asked_input = phase == "input";
        self.dismissed = false;
        self.show(system);
    }

    pub fn phase_changed(&mut self, system: &mut dyn System, banners: &mut dyn Banners, id: &str, phase: &str) {
        if !RUN_PHASES.contains(&phase) {
            return;
        }
        let is_known = self.current == id || self.queue.iter().any(|queued| queued == id) || self.running.iter().any(|run| run == id);
        if is_known {
            match phase {
                "running" if self.current != id || !self.asked_input => {
                    system.run(&["claude-sessions", "approval-pane", "close", id], &[0]);
                }
                "running" => { system.run(&["claude-sessions", "attention-answered", id], &[0]); }
                "input" => { system.run(&["claude-sessions", "approval-pane", id], &[0]); }
                _ => {}
            }
        }
        let input_notice = format!("run-input-{id}");
        if phase == "input" && !system.quiet() {
            banners.raise_notice(&Notice { id: input_notice.clone(), title: "Command waiting".into(), message: "it asks for your input".into() });
        } else {
            banners.drop_banner(&format!("notice:{input_notice}"));
        }
        if self.queue.iter().any(|queued| queued == id) {
            if phase == "asking" {
                return;
            }
            self.queue.retain(|queued| queued != id);
            match phase {
                "done" => {
                    banners.drop_banner(&format!("notice:run-{id}"));
                    system.run(&["claude-sessions", "approval-pane", "close", id], &[0]);
                }
                "running" | "input" => {
                    banners.drop_banner(&format!("notice:run-{id}"));
                    self.running.push(id.into());
                    if phase == "input" {
                        self.waiting_input.push(id.into());
                    }
                }
                _ => unreachable!("validated run phase"),
            }
            self.next(system);
            return;
        }
        if self.running.iter().any(|run| run == id) {
            match phase {
                "done" => {
                    self.running.retain(|run| run != id);
                    self.waiting_input.retain(|run| run != id);
                    system.run(&["claude-sessions", "approval-pane", "close", id], &[0]);
                }
                "input" if self.current.is_empty() => self.attend(system, id, "input"),
                "input" if !self.waiting_input.iter().any(|run| run == id) => self.waiting_input.push(id.into()),
                _ => {}
            }
            return;
        }
        if id != self.current {
            return;
        }
        if phase == "input" && self.phase != "input" {
            self.dismissed = false;
        }
        self.phase = phase.into();
        self.asked_input |= phase == "input";
        match phase {
            "done" => self.finish(system, banners),
            // Approved and running without needing Theo: the next request is shown now, this one
            // runs on with its pane. One that asked for input keeps his attention to its end.
            "running" if !self.asked_input => {
                if let Some(mut window) = self.window.take() {
                    if window.running() {
                        window.terminate();
                    }
                }
                banners.drop_banner(&format!("notice:run-{id}"));
                self.running.push(std::mem::take(&mut self.current));
                self.phase.clear();
                self.dismissed = false;
                self.next(system);
            }
            _ => self.show(system),
        }
    }

    /// The floating window while the approval needs Theo (his yes, or input the running command
    /// asks for), he is off the cockpit, and not gaming. Once he approved, it closes while the
    /// command just runs; the pane under the cockpit's session keeps showing the output. A window
    /// open when he answered input stays to the command's end, but one closed meanwhile (the
    /// cockpit's workspace, or Theo) is not opened again: reopened, it followed him to every
    /// workspace he went to. One Theo closes while it asks stays closed until the next question.
    fn show(&mut self, system: &mut dyn System) {
        let open_now = self.window.as_mut().is_some_and(|window| window.running());
        if self.window.is_some() && !open_now {
            self.window = None;
            self.dismissed = true;
        }
        let asks_now = self.phase == "asking" || self.phase == "input";
        let needs_theo = if open_now { asks_now || self.asked_input } else { asks_now && !self.dismissed };
        let wanted = needs_theo && !system.claude_workspace_active() && !system.quiet();
        if wanted && !open_now {
            let target = format!("={}", run_session(&self.current));
            self.window = system.open_window(&["kitty", "--class", "claude-run", "-T", "Claude asks to run a command", "tmux", "attach-session", "-t", &target]);
            if self.window.is_none() {
                log("cannot open the approval window");
            }
        } else if !wanted && open_now {
            if let Some(mut window) = self.window.take() {
                window.terminate();
            }
        }
    }

    fn finish(&mut self, system: &mut dyn System, banners: &mut dyn Banners) {
        if let Some(mut window) = self.window.take() {
            if window.running() {
                window.terminate();
            }
        }
        banners.drop_banner(&format!("notice:run-{}", self.current));
        system.run(&["claude-sessions", "approval-pane", "close", &self.current], &[0]);
        self.current.clear();
        self.phase.clear();
        self.asked_input = false;
        self.dismissed = false;
        self.next(system);
    }

    /// Every second: the window follows the workspace and the games, and an approval whose tmux
    /// session ended (closed by hand, crashed) is cleared.
    pub fn tick(&mut self, system: &mut dyn System, banners: &mut dyn Banners) {
        for id in self.queue.clone() {
            let target = format!("={}", run_session(&id));
            let abandoned = is_abandoned(&id);
            if abandoned || run_result(&id).is_some_and(|result| result.exists()) || system.tmux(&["has-session", "-t", &target]).is_none() {
                if abandoned {
                    write_abandoned(&id);
                }
                system.run(&["tmux", "kill-session", "-t", &target], &[0, 1]);
                self.phase_changed(system, banners, &id, "done");
            }
        }
        // A running command whose tmux session ended (closed by hand, crashed) is forgotten.
        for id in self.running.clone() {
            if system.tmux(&["has-session", "-t", &format!("={}", run_session(&id))]).is_none() {
                self.running.retain(|run| *run != id);
                self.waiting_input.retain(|run| *run != id);
                system.run(&["claude-sessions", "approval-pane", "close", &id], &[0]);
            }
        }
        if !self.current.is_empty() {
            let target = format!("={}", run_session(&self.current));
            // Only while it asks: a command already running is never cut off.
            if self.phase == "asking" && is_abandoned(&self.current) {
                log(&format!("run request {} dropped: nobody waits for its answer any more", self.current));
                write_abandoned(&self.current);
                system.run(&["tmux", "kill-session", "-t", &target], &[0, 1]);
                self.finish(system, banners);
            } else if system.tmux(&["has-session", "-t", &target]).is_none() {
                self.finish(system, banners);
            } else {
                self.show(system);
            }
        } else if !self.queue.is_empty() {
            self.next(system);
        }
    }
}

/// Steps Claude hands Theo to do by hand (ask-to-guide). Each guide is claude-guide in a kitty
/// window of its own (class claude-guide), opened on the workspace Theo is on, one guide at a time.
/// Never opened while he games; closing the window closes the guide, and Claude is told where he
/// stopped.
#[derive(Default)]
pub struct GuideRequests {
    pub queue: Vec<String>,
    pub current: String,
    /// The answers folder the current guide named, kept from when it opened: the guide file is the
    /// sandbox's to rewrite meanwhile.
    current_answers: PathBuf,
    window: Option<Box<dyn Window>>,
}

impl GuideRequests {
    pub fn request(&mut self, system: &mut dyn System, banners: &mut dyn Banners, id: &str) -> Result<(), String> {
        if !RUN_ID.is_match(id) {
            return Err("not a guide id".into());
        }
        if self.queue.iter().any(|queued| queued == id) || self.current == id {
            return Ok(());
        }
        let project = project_from_request(&cockpit::approval::held_request(id, "guide"));
        if !system.gaming() {
            banners.raise_notice(&Notice { id: format!("guide-{id}"), title: format!("Claude · {project}"), message: "steps for you to do by hand".into() });
        }
        log(&format!("guide {id} from {project}"));
        self.queue.push(id.into());
        self.next(system);
        Ok(())
    }

    fn next(&mut self, system: &mut dyn System) {
        if !self.current.is_empty() || system.gaming() {
            return;
        }
        while !self.queue.is_empty() {
            let id = self.queue.remove(0);
            let Some(answers) = answers_named_by(&cockpit::approval::held_request(&id, "guide"), "guides") else {
                log(&format!("guide {id} dropped: it names no answers folder (its session started before answers went per project)"));
                continue;
            };
            if answers.join(format!("{id}.result")).exists() {
                continue;
            }
            let guide = paths::home().join(".local/bin/claude-guide").to_string_lossy().into_owned();
            match system.open_window(&["kitty", "--class", "claude-guide", "-T", "Claude · steps for you", &guide, &id]) {
                Some(window) => {
                    self.window = Some(window);
                    self.current = id;
                    self.current_answers = answers;
                    return;
                }
                None => log(&format!("cannot open guide {id}")),
            }
        }
    }

    fn finish(&mut self, system: &mut dyn System, banners: &mut dyn Banners) {
        banners.drop_banner(&format!("notice:guide-{}", self.current));
        let result = self.current_answers.join(format!("{}.result", self.current));
        if !result.exists() {
            // The window was closed: Claude must not wait on it, and learns where Theo stopped.
            let progress = fs::read_to_string(self.current_answers.join(format!("{}.progress", self.current))).ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok()).unwrap_or(json!({}));
            let closed = json!({"status": "closed", "step": progress.get("step").cloned().unwrap_or(json!("?")),
                                "total": progress.get("total").cloned().unwrap_or(json!("?"))});
            if let Err(error) = cockpit::util::write_json(&result, &closed) {
                log(&format!("cannot close guide {}: {error}", self.current));
            }
        }
        self.current.clear();
        self.window = None;
        self.next(system);
    }

    pub fn tick(&mut self, system: &mut dyn System, banners: &mut dyn Banners) {
        let ended = self.window.as_mut().is_some_and(|window| !window.running());
        if !self.current.is_empty() && ended {
            self.finish(system, banners);
        } else if self.current.is_empty() && !self.queue.is_empty() {
            self.next(system);
        }
    }
}

// ---- The registry ----

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub state: String,
    pub needs_input: bool,
    pub project: String,
    pub cwd: String,
    pub task: String,
    pub label: String,
    pub since: f64,
    pub started: f64,
    pub target: BTreeMap<String, String>,
    pub update: bool,
    pub request: String,
    pub detail: String,
    pub full_request: String,
    pub outcome: String,
    pub repository: String,
    pub branch: String,
    pub last_result: Option<cockpit::sessions::TurnResult>,
    /// Its hook reports repositories and turn ids, so a missing result is not an outdated hook.
    pub reports_turns: bool,
    pub terminal: bool,
    pub credits: bool,
    pub unpushed: u64,
    pub agents: u64,
    pub background: u64,
    pub apps: String,
    /// The name of the agent that runs it (cockpit::agents).
    pub agent: String,
    pub native_thread: String,
    /// The launcher incarnation that reported it (trusted_runtime); never published.
    pub runtime: String,
}

pub struct Registry {
    pub system: Box<dyn System>,
    pub banners: Box<dyn Banners>,
    pub sessions: BTreeMap<String, Entry>,
    /// session -> its latest banner, kept back during a meeting or a game
    pub held: BTreeMap<String, Banner>,
    pub runs: RunRequests,
    pub guides: GuideRequests,
    recent_written: (f64, Vec<String>),
    project_cache: HashMap<String, String>,
    /// Linked-session state, opened on first use (teams.rs).
    pub(crate) teams: Option<cockpit::teams::store::Connection>,
}

pub fn snapshot_path() -> PathBuf {
    paths::snapshot()
}

impl Registry {
    pub fn new(system: Box<dyn System>, banners: Box<dyn Banners>) -> Self {
        let mut registry = Registry { system, banners, sessions: BTreeMap::new(), held: BTreeMap::new(), runs: RunRequests::default(),
                                      guides: GuideRequests::default(), recent_written: (0.0, Vec::new()), project_cache: HashMap::new(), teams: None };
        registry.load_snapshot();
        registry
    }

    fn project_of(&mut self, cwd: &str, fallback: &str) -> String {
        project_of(&mut self.project_cache, cwd, fallback)
    }

    pub fn run_request(&mut self, id: &str) -> Result<(), String> {
        self.runs.request(self.system.as_mut(), self.banners.as_mut(), id)?;
        self.publish();
        Ok(())
    }

    /// The run and guide requests waiting (queued or shown) from sandboxes started in `started_in`.
    fn waiting_from(&self, started_in: &str) -> usize {
        let runs = self.runs.queue.iter().chain(std::iter::once(&self.runs.current)).map(|id| cockpit::approval::held_request(id, "request"));
        let guides = self.guides.queue.iter().chain(std::iter::once(&self.guides.current)).map(|id| cockpit::approval::held_request(id, "guide"));
        runs.chain(guides).filter(|path| {
            fs::read_to_string(path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|request| request.get("started_in").and_then(Value::as_str) == Some(started_in))
        }).count()
    }

    /// A run or guide request, or a todo update, from the report socket: held (hold_request) or
    /// queued (queue_todo_update) with the folder the sender's sandbox started in.
    pub fn request_from(&mut self, kind: &str, raw: &str, sender: u32) -> Result<(), String> {
        if kind == "session-request" {
            return self.report_from(parse_session(raw)?, sender);
        }
        let started_in = self.system.sender_directory(sender);
        let result = match &started_in {
            Some(started_in) => self.take_request(kind, raw, started_in, sender),
            None => Err("its sender has no process outside the sandbox".into()),
        };
        audit_request(kind, raw, started_in.as_deref(), sender, &result);
        result
    }

    fn take_request(&mut self, kind: &str, raw: &str, started_in: &str, sender: u32) -> Result<(), String> {
        let started_in = started_in.to_string();
        if kind == "todo-update" {
            return queue_todo_update(raw, &started_in);
        }
        if self.waiting_from(&started_in) >= WAITING_PER_FOLDER_MAX {
            return Err(format!("{WAITING_PER_FOLDER_MAX} requests from this session's folder already wait for Theo"));
        }
        let claimed = serde_json::from_str::<Value>(raw).ok()
            .and_then(|request| request.get("session").and_then(Value::as_str).map(str::to_string));
        let verified_session = self.system.sender_target(sender).and_then(|target| {
            let pane = target.get("tmux_pane").filter(|pane| !pane.is_empty())?;
            let session = claimed.as_deref()?;
            let entry = self.sessions.get(session)?;
            (entry.state != "gone" && entry.target.get("tmux_pane") == Some(pane)).then_some(session)
        });
        let id = hold_request(&mut self.project_cache, kind, raw, &started_in, verified_session)?;
        log(&format!("{kind} {id} held, from a sandbox started in {started_in}"));
        if kind == "run-request" { self.run_request(&id) } else { self.guide_request(&id) }
    }

    pub fn run_phase(&mut self, id: &str, phase: &str) {
        self.runs.phase_changed(self.system.as_mut(), self.banners.as_mut(), id, phase);
        self.publish();
    }

    pub fn guide_request(&mut self, id: &str) -> Result<(), String> {
        self.guides.request(self.system.as_mut(), self.banners.as_mut(), id)
    }

    /// Every second: the run and guide queues follow the workspace, the games and closed windows.
    pub fn tick_queues(&mut self) {
        let phases = self.runs.session_phases();
        self.runs.tick(self.system.as_mut(), self.banners.as_mut());
        self.guides.tick(self.system.as_mut(), self.banners.as_mut());
        if phases != self.runs.session_phases() {
            self.publish();
        }
    }

    /// A report from the report socket: its focus target comes from where the sender really runs,
    /// never from the report, and a session cannot be moved off a pane where its Claude still runs.
    pub fn report_from(&mut self, mut report: Report, sender: u32) -> Result<(), String> {
        let Some(target) = self.system.sender_target(sender) else {
            log(&format!("report for {} refused: its sender (pid {sender}) has no process outside the sandbox", &report.session[..8]));
            audit_report(&report, sender, "report.rejected", "refused", "its sender has no process outside the sandbox");
            return Err("sender has no process outside the sandbox".into());
        };
        if let Some(known) = self.sessions.get(&report.session).and_then(|entry| entry.target.get("tmux_pane")).cloned() {
            if target.get("tmux_pane") != Some(&known) && self.system.claude_panes().contains(&known) {
                log(&format!("report for {} refused: it claims a pane other than {known}, where that session still runs", &report.session[..8]));
                audit_report(&report, sender, "report.rejected", "refused", &format!("it claims a pane other than {known}, where that session still runs"));
                return Err(format!("session already runs in pane {known}"));
            }
        }
        report.target = target;
        report.runtime = self.system.sender_runtime(sender);
        // The session's folder is where its sandbox started, never the one the report claims: a
        // claimed folder of another project would make a restart resume the session there, in that
        // project's sandbox with its token (security audit 2026-10-19, M1).
        if let Some(claimed) = report.cwd.take().filter(|cwd| !cwd.is_empty()) {
            let started_in = self.system.sender_directory(sender);
            if !started_in.as_ref().is_some_and(|started_in| Path::new(&claimed).starts_with(started_in)) {
                log(&format!("report for {} claims the folder {claimed:?}, outside {started_in:?} where its sandbox started: that one is kept",
                             &report.session[..8]));
                audit_report(&report, sender, "report.folder_corrected", "corrected",
                             &format!("claimed {claimed:?}, outside {started_in:?} where its sandbox started"));
            }
            report.cwd = started_in;
        }
        if !self.sessions.contains_key(&report.session) {
            cockpit::audit::record("daemon", "session.first_report", "ok", json!({
                "session": report.session, "claimed_project": report.project, "started_in": report.cwd, "sender_pid": sender,
                "tmux_pane": report.target.get("tmux_pane")}));
        }
        self.update(report);
        Ok(())
    }

    pub fn update(&mut self, report: Report) {
        let session = report.session.clone();
        if report.state.as_deref() == Some("gone") {
            self.remove(&session);
            return;
        }
        let previous_state = self.sessions.get(&session).map(|entry| entry.state.clone());
        let mut changed = false;
        if previous_state.is_none() {
            let time = now();
            self.sessions.insert(session.clone(), Entry { state: report.state.clone().unwrap_or("waiting".into()), project: report.project.clone(),
                                                          since: time, started: time, agent: agents::DEFAULT.into(), ..Entry::default() });
            changed = true;
        }
        let mut entry = self.sessions.remove(&session).unwrap();
        if let Some(runtime) = report.runtime.as_ref().filter(|runtime| **runtime != entry.runtime) {
            entry.runtime = runtime.clone();
        }
        if let Some(native) = report.native_thread.as_ref().filter(|native| **native != entry.native_thread) {
            entry.native_thread = native.clone();
            changed = true;
        }
        if let Some(needs_input) = report.needs_input.filter(|needs_input| *needs_input != entry.needs_input) {
            entry.needs_input = needs_input;
            changed = true;
        }
        if entry.needs_input && (!entry.detail.is_empty() || !entry.full_request.is_empty()) {
            entry.detail.clear();
            entry.full_request.clear();
            changed = true;
        }
        if let Some(state) = &report.state {
            if &entry.state != state {
                entry.state = state.clone();
                changed = true;
            }
        }
        if let Some(task) = &report.task {
            if &entry.task != task {
                entry.task = task.clone();
                changed = true;
            }
        }
        if let Some(label) = report.label.as_ref().filter(|label| **label != entry.label) {
            entry.label = label.clone();
            changed = true;
        }
        // What a session waiting on you asks: the hook's sentence, and the pane's detail once the
        // next reconcile reads the dialog. Gone with the wait.
        if entry.state == "needs-you" && report.message.as_ref().is_some_and(|m| !m.is_empty() && *m != entry.request) {
            entry.request = report.message.clone().unwrap();
            entry.detail.clear();
            entry.full_request.clear();
            changed = true;
        } else if entry.state != "needs-you" && (!entry.request.is_empty() || !entry.detail.is_empty() || !entry.full_request.is_empty()) {
            entry.request.clear();
            entry.detail.clear();
            entry.full_request.clear();
            changed = true;
        }
        // The directory the session started in, and its project: later reports carry Claude's
        // current directory, which wanders into subfolders.
        if let Some(cwd) = report.cwd.as_ref().filter(|cwd| !cwd.is_empty()) {
            if entry.cwd.is_empty() {
                entry.cwd = cwd.clone();
                entry.project = self.project_of(cwd, &report.project);
                changed = true;
            }
        }
        if let Some(apps) = report.apps.as_ref().filter(|apps| **apps != entry.apps) {
            entry.apps = apps.clone();
            changed = true;
        }
        if let Some(unpushed) = report.unpushed.filter(|count| *count != entry.unpushed) {
            entry.unpushed = unpushed;
            changed = true;
        }
        if let Some(agents) = report.agents.filter(|count| *count != entry.agents) {
            entry.agents = agents;
            changed = true;
        }
        if let Some(background) = report.background.filter(|count| *count != entry.background) {
            entry.background = background;
            changed = true;
        }
        if let Some(credits) = report.credits.filter(|credits| *credits != entry.credits) {
            entry.credits = credits;
            changed = true;
        }
        if let Some(agent) = report.agent.as_ref().filter(|agent| **agent != entry.agent) {
            entry.agent = agent.clone();
            changed = true;
        }
        if report.terminal && !entry.terminal {
            entry.terminal = true;
            changed = true;
        }
        let previous_pid = entry.target.get("kitty_pid").cloned();
        let mut target = entry.target.clone();
        target.extend(report.target.clone());
        let resolved_target = self.resolve_target(target);
        if entry.target != resolved_target {
            entry.target = resolved_target;
            changed = true;
        }
        let kitty_pid = entry.target.get("kitty_pid").cloned();
        if previous_state.as_deref() != Some(entry.state.as_str()) {
            entry.since = now();
        }
        // A new turn forgets what the last one did.
        if entry.state == "working" && previous_state.as_deref() != Some("working") {
            entry.outcome.clear();
        }
        if !entry.reports_turns && (report.repository.is_some() || report.turn_id.is_some()) {
            entry.reports_turns = true;
            changed = true;
        }
        if let Some(repository) = report.repository.as_ref().filter(|repository| **repository != entry.repository) {
            entry.repository = repository.clone();
            changed = true;
        }
        if let Some(branch) = report.branch.as_ref().filter(|branch| **branch != entry.branch) {
            entry.branch = branch.clone();
            changed = true;
        }
        if report.turn_complete && report.outcome.as_ref().is_some_and(|outcome| !outcome.is_empty()) {
            if let Some(id) = report.turn_id.as_ref().filter(|id| entry.last_result.as_ref().is_none_or(|result| result.id != **id)) {
                entry.last_result = Some(cockpit::sessions::TurnResult {
                    id: id.clone(), completed_at: now() as i64,
                    outcome: report.outcome.clone().unwrap_or_default(),
                    repository: entry.repository.clone(), branch: entry.branch.clone(),
                    extra: Default::default(),
                });
                changed = true;
            }
        }
        let mut message = report.message.clone();
        if message.as_deref() == Some(FINISHED_MESSAGE) {
            entry.outcome = report.outcome.clone().unwrap_or_default();
            if !entry.outcome.is_empty() {
                message = Some(format!("Finished · {}", entry.outcome));
            }
            changed = true;
        }
        let project = entry.project.clone();
        let state = entry.state.clone();
        let tmux_pane = entry.target.get("tmux_pane").cloned();
        self.sessions.insert(session.clone(), entry);
        self.drop_pane_predecessors(&session, tmux_pane.as_deref());

        match message {
            None if report.label.is_some() => {}
            None => {
                self.banners.drop_banner(&session);
                self.held.remove(&session);
            }
            Some(message) if !self.system.claude_workspace_active() => {
                let banner = Banner { session: session.clone(), title: format!("Claude · {project}"), message: message.clone(), sticky: report.sticky };
                let short: String = session.chars().take(8).collect();
                if self.system.quiet() {
                    self.held.insert(session.clone(), banner);
                    log(&format!("banner held (meeting or game) for {short} ({project}): {message}"));
                } else {
                    self.banners.raise_banner(&banner);
                    log(&format!("banner {}for {short} ({project}): {message}", if report.sticky { "sticky " } else { "" }));
                }
            }
            Some(_) => {}
        }
        // The window a session moved away from loses its tag unless another session still lives
        // there; the new one is tagged.
        let window_changed = kitty_pid != previous_pid;
        if window_changed {
            if let Some(previous) = &previous_pid {
                self.apply_window_state(previous);
            }
        }
        if let Some(pid) = &kitty_pid {
            if window_changed || previous_state.as_deref() != Some(state.as_str()) {
                self.apply_window_state(pid);
            }
        }
        if changed || window_changed {
            self.publish();
        }
    }

    /// The kitty window currently attached to the session's tmux pane, or the reported one.
    pub fn resolve_target(&mut self, target: BTreeMap<String, String>) -> BTreeMap<String, String> {
        let Some(tmux_pane) = target.get("tmux_pane").cloned() else { return self.without_dead_kitty(target) };
        let Some(session_name) = self.system.tmux(&["display-message", "-p", "-t", &tmux_pane, "#{session_name}"]).filter(|n| !n.trim().is_empty()) else {
            return self.without_dead_kitty(target);
        };
        let clients = self.system.tmux(&["list-clients", "-t", session_name.trim(), "-F", "#{client_pid}"]).unwrap_or_default();
        for client in clients.split_whitespace() {
            let environment = self.system.process_environment(client);
            let Some(kitty_pid) = environment.get("KITTY_PID").filter(|pid| !pid.is_empty()).cloned() else { continue };
            if !self.system.process_alive(&kitty_pid) {
                continue;
            }
            let mut resolved = BTreeMap::from([("tmux_pane".to_string(), tmux_pane.clone()), ("kitty_pid".to_string(), kitty_pid)]);
            for (key, variable) in [("kitty_window", "KITTY_WINDOW_ID"), ("kitty_listen", "KITTY_LISTEN_ON")] {
                if let Some(value) = environment.get(variable).filter(|v| !v.is_empty()) {
                    resolved.insert(key.into(), value.clone());
                }
            }
            let fields: Map<String, Value> = resolved.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            if let Err(error) = validate_focus_target(&fields) {
                log(&format!("ignoring tmux client {client}: {error}"));
                continue;
            }
            return resolved;
        }
        self.without_dead_kitty(target)
    }

    /// A KITTY_PID inherited from a tmux server's environment can name a long closed terminal.
    fn without_dead_kitty(&mut self, target: BTreeMap<String, String>) -> BTreeMap<String, String> {
        match target.get("kitty_pid") {
            Some(pid) if !self.system.process_alive(pid) => target.into_iter().filter(|(key, _)| !key.starts_with("kitty_")).collect(),
            _ => target,
        }
    }

    /// A pane runs one Claude at a time, but /clear, --resume and forks give it a new session id
    /// without a SessionEnd for the old one: the previous ids of the same pane are gone.
    fn drop_pane_predecessors(&mut self, session: &str, tmux_pane: Option<&str>) {
        let Some(tmux_pane) = tmux_pane else { return };
        let others: Vec<(String, String)> = self.sessions.iter()
            .filter(|(other, entry)| other.as_str() != session && entry.target.get("tmux_pane").map(String::as_str) == Some(tmux_pane))
            .map(|(other, entry)| (other.clone(), entry.project.clone())).collect();
        for (other, project) in others {
            log(&format!("session {other} ({project}) replaced in {tmux_pane} by {session}"));
            self.remove(&other);
        }
    }

    /// claude-sessions pushed the branch of the sessions started in `directory` (the sidebar's U):
    /// nothing of theirs waits on a push any more, until their hook counts again.
    pub fn pushed(&mut self, directory: &str) {
        if !directory.starts_with('/') || directory.chars().any(char::is_control) {
            return;
        }
        let mut changed = false;
        for entry in self.sessions.values_mut().filter(|entry| entry.cwd == directory && entry.unpushed > 0) {
            entry.unpushed = 0;
            changed = true;
        }
        if changed {
            self.publish();
        }
    }

    /// Theo looked at the session: its banner (sticky ones too, and one held for a game or a
    /// meeting) has said what it had to say.
    pub fn seen(&mut self, session: &str) {
        if !agents::is_session_id(session) {
            return;
        }
        self.held.remove(session);
        self.banners.drop_banner(session);
    }

    pub fn remove(&mut self, session: &str) {
        self.held.remove(session);
        let Some(entry) = self.sessions.remove(session) else { return };
        self.banners.drop_banner(session);
        if let Some(pid) = entry.target.get("kitty_pid") {
            self.apply_window_state(pid);
        }
        self.publish();
    }

    /// Most urgent state among the sessions sharing one kitty window (several tmux windows).
    fn window_state(&self, kitty_pid: &str) -> Option<String> {
        self.sessions.values().filter(|entry| entry.target.get("kitty_pid").map(String::as_str) == Some(kitty_pid))
            .map(|entry| entry.state.clone()).max_by_key(|state| urgency(state))
    }

    fn apply_window_state(&mut self, kitty_pid: &str) {
        let state = self.window_state(kitty_pid).map(|state| format!("\"{state}\"")).unwrap_or_else(|| "nil".into());
        self.system.run(&["hyprctl", "eval", &format!("CLAUDE_WINDOW_STATE({kitty_pid}, {state})")], &[0]);
    }

    pub fn snapshot_rows(&self) -> Vec<Value> {
        let run_phases = self.runs.session_phases();
        let mut rows: Vec<Value> = self.sessions.iter().map(|(session, entry)| {
            let mut row = json!({"session": session, "state": entry.state, "project": entry.project, "cwd": entry.cwd, "task": entry.task, "label": entry.label,
                                 "since": entry.since as i64, "started": entry.started as i64, "update": entry.update,
                                  "request": entry.request, "detail": entry.detail, "full_request": entry.full_request, "outcome": entry.outcome, "needs_input": entry.needs_input,
                                  "repository": entry.repository, "branch": entry.branch, "last_result": entry.last_result, "reports_turns": entry.reports_turns,
                                 "terminal": entry.terminal, "credits": entry.credits, "unpushed": entry.unpushed,
                                  "agents": entry.agents, "background": entry.background, "apps": entry.apps, "agent": entry.agent,
                                  "ask_to_run": run_phases.get(session).copied().unwrap_or("")});
            for (key, value) in &entry.target {
                row[key] = json!(value);
            }
            if !entry.native_thread.is_empty() {
                row["native_thread"] = json!(entry.native_thread);
            }
            row
        }).collect();
        rows.sort_by(|a, b| {
            let key = |row: &Value| (-urgency(row["state"].as_str().unwrap_or("")), row["since"].as_i64().unwrap_or(0));
            key(a).cmp(&key(b))
        });
        rows
    }

    pub fn publish(&mut self) {
        let path = snapshot_path();
        let _ = cockpit::util::write_json(&path, &json!({"sessions": self.snapshot_rows()}));
        self.remember_sessions();
        // Exit 1 is "no waybar running", not an error.
        self.system.run(&["pkill", "-x", "-RTMIN+16", "waybar"], &[0, 1]);
    }

    /// recent-sessions.json for the column's "bring them back" after a reboot: refreshed when the
    /// set of sessions changes, else once a minute; entries age out after a week.
    fn remember_sessions(&mut self) {
        let mut ids: Vec<String> = self.sessions.iter().filter(|(_, entry)| !entry.cwd.is_empty()).map(|(id, _)| id.clone()).collect();
        ids.sort();
        let (written_at, written_ids) = &self.recent_written;
        if ids == *written_ids && now() - written_at < RECENT_SESSIONS_WRITE_S {
            return;
        }
        let path = paths::recent_sessions();
        let mut recent = match cockpit::util::read_json(&path) { Some(Value::Object(map)) => map, _ => Map::new() };
        let time = now();
        for id in &ids {
            let entry = &self.sessions[id];
            recent.insert(id.clone(), json!({"project": entry.project, "cwd": entry.cwd, "last_seen": time as i64, "terminal": entry.terminal,
                                      "credits": entry.credits, "agent": entry.agent}));
            if !entry.native_thread.is_empty() {
                recent[id]["native_thread"] = json!(entry.native_thread);
            }
        }
        let oldest = time - RECENT_SESSIONS_DAYS * 86400.0;
        recent.retain(|_, row| row.get("last_seen").and_then(Value::as_f64).unwrap_or(0.0) >= oldest);
        if let Err(error) = cockpit::util::write_json(&path, &Value::Object(recent)) {
            log(&format!("cannot write {}: {error}", path.display()));
        }
        self.recent_written = (time, ids);
    }

    /// A daemon restart keeps the sessions; the next hook event refreshes their state.
    fn load_snapshot(&mut self) {
        let path = snapshot_path();
        let Ok(text) = fs::read_to_string(&path) else { return };
        let Some(rows) = serde_json::from_str::<Value>(&text).ok().and_then(|snapshot| snapshot.get("sessions").and_then(Value::as_array).cloned()) else {
            log(&format!("ignoring snapshot {}: not a session list", path.display()));
            return;
        };
        for row in rows {
            let loaded = (|| -> Result<(String, Entry), String> {
                let session = row.get("session").and_then(Value::as_str).ok_or("no session")?.to_string();
                let native_thread = native_thread_of(row.as_object().ok_or("snapshot row is not an object")?)?.unwrap_or_default();
                let state = row.get("state").and_then(Value::as_str).ok_or("no state")?.to_string();
                if urgency(&state) < 0 {
                    return Err(format!("state {state:?} unknown"));
                }
                let target: Map<String, Value> = TARGET_FIELDS.iter()
                    .filter_map(|key| row.get(*key).filter(|v| !v.is_null() && v.as_str() != Some("")).map(|v| (key.to_string(), json!(match v { Value::String(s) => s.clone(), other => other.to_string() }))))
                    .collect();
                validate_focus_target(&target)?;
                let cwd = row.get("cwd").and_then(Value::as_str).unwrap_or("").to_string();
                let project = row.get("project").and_then(Value::as_str).ok_or("no project")?;
                let since = row.get("since").and_then(Value::as_f64).unwrap_or_else(now);
                let request = row.get("request").and_then(Value::as_str).unwrap_or("");
                validate_text("request", request)?;
                let full_request = row.get("full_request").and_then(Value::as_str).unwrap_or("");
                if full_request.chars().count() > FULL_REQUEST_MAX_LENGTH || full_request.chars().any(|character| character.is_control() && character != '\n')
                    || full_request.contains("#(") || full_request.contains("#{") {
                    return Err("invalid full request in snapshot".into());
                }
                let label = row.get("label").and_then(Value::as_str).filter(|label| !label.is_empty())
                    .map(cockpit::sessions::validate_task_label).transpose()?.unwrap_or_default();
                let repository = row.get("repository").and_then(Value::as_str).unwrap_or("");
                if !repository.is_empty() && !cockpit::sessions::valid_repository(repository) {
                    return Err("invalid repository in snapshot".into());
                }
                let branch = row.get("branch").and_then(Value::as_str).unwrap_or("");
                validate_text("branch", branch)?;
                let last_result = row.get("last_result").filter(|value| !value.is_null())
                    .map(cockpit::sessions::TurnResult::from_value).transpose()?;
                Ok((session, Entry {
                    state, project: project.into(), cwd, task: row.get("task").and_then(Value::as_str).unwrap_or("").into(), since,
                    needs_input: row.get("needs_input").and_then(Value::as_bool).unwrap_or(false),
                    label,
                    repository: repository.into(), branch: branch.into(), last_result,
                    reports_turns: row.get("reports_turns").and_then(Value::as_bool).unwrap_or(false),
                    native_thread,
                    request: request.into(),
                    full_request: full_request.into(),
                    started: row.get("started").and_then(Value::as_f64).unwrap_or(since),
                    target: target.into_iter().map(|(k, v)| (k, v.as_str().unwrap_or("").to_string())).collect(),
                    update: row.get("update").and_then(Value::as_bool).unwrap_or(false),
                    terminal: row.get("terminal").and_then(Value::as_bool).unwrap_or(false),
                    credits: row.get("credits").and_then(Value::as_bool).unwrap_or(false),
                    unpushed: row.get("unpushed").and_then(Value::as_u64).unwrap_or(0),
                    agents: row.get("agents").and_then(Value::as_u64).unwrap_or(0),
                    background: row.get("background").and_then(Value::as_u64).unwrap_or(0),
                    agent: row.get("agent").and_then(Value::as_str).filter(|agent| agents::find(agent).is_some()).unwrap_or(agents::DEFAULT).into(),
                    ..Entry::default()
                }))
            })();
            match loaded {
                Ok((session, mut entry)) => {
                    entry.project = self.project_of(&entry.cwd.clone(), &entry.project.clone());
                    entry.target = self.resolve_target(entry.target.clone());
                    self.sessions.insert(session, entry);
                }
                Err(error) => log(&format!("skipping snapshot row {row}: {error}")),
            }
        }
        self.prune_dead();
        let pids: HashSet<String> = self.sessions.values().filter_map(|entry| entry.target.get("kitty_pid").cloned()).collect();
        for pid in pids {
            self.apply_window_state(&pid);
        }
        self.publish();
    }

    /// Alive while a session binary carries the id. A session without one (an older Claude) is
    /// alive while a Claude process runs in its pane; without a pane the terminal decides.
    pub fn is_dead(&mut self, session: &str, target: &BTreeMap<String, String>, live_ids: &HashSet<String>, live_panes: &HashSet<String>) -> bool {
        if live_ids.contains(session) {
            return false;
        }
        if let Some(pane) = target.get("tmux_pane") {
            return !live_panes.contains(pane);
        }
        match target.get("kitty_pid") {
            Some(pid) => !self.system.process_alive(pid),
            None => false,
        }
    }

    /// After the meeting or the game, the banners it held back, for the sessions still there.
    fn release_held(&mut self) {
        if self.held.is_empty() || self.system.quiet() {
            return;
        }
        let held = std::mem::take(&mut self.held);
        for (session, banner) in held {
            if self.sessions.contains_key(&session) {
                self.banners.raise_banner(&banner);
            }
        }
    }

    /// Every few seconds: gone sessions dropped, panes read, held banners released, stale ones dropped.
    pub fn prune_dead(&mut self) {
        let live_ids = self.system.live_session_ids();
        let live_panes = self.system.claude_panes();
        let sessions: Vec<(String, BTreeMap<String, String>, String)> = self.sessions.iter().map(|(id, entry)| (id.clone(), entry.target.clone(), entry.project.clone())).collect();
        for (session, target, project) in sessions {
            if self.is_dead(&session, &target, &live_ids, &live_panes) {
                log(&format!("session {session} ({project}) is gone, dropping it"));
                self.remove(&session);
            }
        }
        self.reconcile_with_panes();
        self.release_held();
        self.drop_stale_banners();
    }

    /// A working session asks for nothing: its banner goes, even if the report that should have
    /// removed it was missed.
    fn drop_stale_banners(&mut self) {
        let working: Vec<(String, String)> = self.sessions.iter().filter(|(_, entry)| entry.state == "working").map(|(id, entry)| (id.clone(), entry.project.clone())).collect();
        for (session, project) in working {
            if self.banners.showing(&session) {
                log(&format!("banner for {} ({project}) still up while it works: removed", &session[..8.min(session.len())]));
                self.banners.drop_banner(&session);
            }
        }
    }

    /// The pane's last lines say what Claude is doing now; they win over stale hook states. Only
    /// for the agents whose screen the patterns know (Feature::ScreenStates): Claude Code's.
    pub fn reconcile_with_panes(&mut self) {
        let sessions: Vec<(String, String)> = self.sessions.iter().filter(|(_, entry)| agents::get(&entry.agent).can(Feature::ScreenStates))
            .filter_map(|(id, entry)| entry.target.get("tmux_pane").map(|pane| (id.clone(), pane.clone()))).collect();
        for (session, pane) in sessions {
            let capture_lines = if self.sessions.get(&session).is_some_and(|entry| entry.state == "needs-you" && !entry.needs_input) { DIALOG_CAPTURE_LINES } else { PANE_TAIL_LINES };
            let tail = self.system.tmux(&["capture-pane", "-p", "-t", &pane, "-S", &format!("-{capture_lines}")]).unwrap_or_default();
            let update_ready = UPDATE_READY.is_match(&tail);
            let Some(entry) = self.sessions.get_mut(&session) else { continue };
            if update_ready != entry.update {
                entry.update = update_ready;
                self.publish();
            }
            let Some(entry) = self.sessions.get(&session) else { continue };
            // Plain chat questions have no dialog on screen; only the next prompt releases them.
            if entry.needs_input { continue; }
            if let Some(observed) = pane_state(&tail).filter(|observed| *observed != entry.state) {
                let project = entry.project.clone();
                self.update(Report { session: session.clone(), project, state: Some(observed.into()), ..Report::default() });
            }
            let Some(entry) = self.sessions.get_mut(&session) else { continue };
            let detail = if entry.state == "needs-you" { permission_request(&tail) } else { String::new() };
            let full_request = if entry.state == "needs-you" { permission_request_full(&tail) } else { String::new() };
            if detail != entry.detail || full_request != entry.full_request {
                entry.detail = detail;
                entry.full_request = full_request;
                self.publish();
            }
        }
    }

    pub fn clear(&mut self) {
        let sessions: Vec<String> = self.sessions.keys().cloned().collect();
        for session in sessions {
            self.remove(&session);
        }
    }

    /// Shows the most urgent session in the manager; with none, just opens the manager.
    pub fn focus_most_urgent(&mut self) {
        let best = self.sessions.iter().filter(|(_, entry)| entry.target.contains_key("tmux_pane"))
            .max_by(|(a_id, a), (b_id, b)| (urgency(&a.state), -a.since as i64, a_id.as_str()).cmp(&(urgency(&b.state), -b.since as i64, b_id.as_str())))
            .map(|(id, _)| id.clone());
        match best {
            None => { self.system.run(&["claude-sessions", "show"], &[0]); }
            Some(session) => {
                self.system.run(&["claude-sessions", "show", &session], &[0]);
                self.banners.drop_banner(&session);
            }
        }
    }
}

// ---- The real system ----

pub struct RealSystem {
    gaming_cache: (Option<Instant>, bool),
    /// When khal was last asked, and its answer so far: khal runs on a thread of its own, never on
    /// the GTK loop (it takes a few hundred ms to start; security audit 2026-10-19, performance 4).
    meeting_cache: (Option<Instant>, std::sync::Arc<std::sync::atomic::AtomicBool>),
    /// One /proc scan for both live_session_ids and claude_panes within PROCESS_SCAN_FRESH: each
    /// walked every process on its own, twice every PRUNE_INTERVAL_S.
    process_scan: Option<(Instant, HashSet<String>, HashSet<String>)>,
}

const PROCESS_SCAN_FRESH: Duration = Duration::from_millis(500);

impl RealSystem {
    pub fn new() -> Self {
        RealSystem { gaming_cache: (None, false), meeting_cache: (None, Default::default()), process_scan: None }
    }

    /// The live session ids and the Claude panes, from one /proc scan.
    fn scanned(&mut self) -> (HashSet<String>, HashSet<String>) {
        if let Some((at, ids, panes)) = &self.process_scan {
            if at.elapsed() < PROCESS_SCAN_FRESH {
                return (ids.clone(), panes.clone());
            }
        }
        let processes = claude_processes(&Proc);
        let (ids, panes) = (session_ids_of(&Proc, &processes), panes_of(&Proc, &processes));
        self.process_scan = Some((Instant::now(), ids.clone(), panes.clone()));
        (ids, panes)
    }
}

/// A command's exit code, stdout and stderr, or None when it cannot start or outlives `timeout`.
pub fn output_within(command: &[&str], timeout: Duration) -> Option<(i32, String, String)> {
    if command[0] == "hyprctl" {
        cockpit::util::refresh_hyprland_instance();
    }
    let mut child = Command::new(command[0]).args(&command[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => return None,
        }
    }
    let output = child.wait_with_output().ok()?;
    Some((output.status.code().unwrap_or(-1), String::from_utf8_lossy(&output.stdout).into_owned(), String::from_utf8_lossy(&output.stderr).into_owned()))
}

/// The process table: /proc for real, a made-up tree in tests.
pub trait Processes {
    fn pids(&self) -> Vec<String>;
    fn comm(&self, pid: &str) -> String;
    fn command(&self, pid: &str) -> Vec<String>;
    fn environment(&self, pid: &str) -> HashMap<String, String>;
    /// The parent's pid, None at the top or for a process gone.
    fn parent(&self, pid: &str) -> Option<String> {
        let _ = pid;
        None
    }
    /// The process's working directory, None for a process gone.
    fn cwd(&self, pid: &str) -> Option<String> {
        let _ = pid;
        None
    }
    /// The process's pid namespace ("self" for the daemon), to tell sandboxed processes apart.
    fn pid_namespace(&self, pid: &str) -> Option<String> {
        let _ = pid;
        None
    }
    fn start_time(&self, _pid: &str) -> Option<String> { None }
    fn boot_id(&self) -> Option<String> { None }
}

pub(crate) fn parse_process_start_time(stat: &str) -> Option<String> {
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(19)?.parse::<u64>().ok().map(|ticks| ticks.to_string())
}

fn runtime_of_process(table: &dyn Processes, pid: &str) -> Option<String> {
    if pid.parse::<u32>().ok()? == 0 || table.pid_namespace(pid)? != table.pid_namespace("self")? { return None; }
    let boot = table.boot_id()?;
    if boot.is_empty() || boot.len() > 64 || !boot.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-') { return None; }
    let started = table.start_time(pid)?.parse::<u64>().ok()?;
    Some(format!("{boot}:{pid}:{started}"))
}

/// Bind a sandbox peer to its host launch incarnation, not a PID that can later be reused.
pub fn trusted_runtime(table: &dyn Processes, sender: &str) -> Option<String> {
    if table.pid_namespace(sender)? == table.pid_namespace("self")? { return None; }
    runtime_of_process(table, &first_process_outside(table, sender)?)
}

pub fn runtime_is_live(table: &dyn Processes, runtime: &str) -> bool {
    let mut parts = runtime.split(':');
    let (Some(_boot), Some(pid), Some(_started), None) = (parts.next(),parts.next(),parts.next(),parts.next()) else { return false };
    runtime_of_process(table, pid).as_deref() == Some(runtime)
}

/// Where a session report really comes from: the focus target (tmux pane, kitty window) in the
/// environment of the first ancestor of the sending process that runs outside the sandbox, in the
/// daemon's own pid namespace. What runs inside a sandbox can set any TMUX_PANE it likes in its own
/// environment; the launcher's, outside, comes from the pane tmux started it in. None when the
/// sender has no such ancestor (a report the daemon cannot place).
/// The sender itself or its first ancestor in the daemon's own pid namespace: outside the sandbox,
/// out of reach of anything inside it.
fn first_process_outside(table: &dyn Processes, sender: &str) -> Option<String> {
    let own = table.pid_namespace("self")?;
    let mut pid = sender.to_string();
    for _ in 0..64 {
        if table.pid_namespace(&pid).as_ref() == Some(&own) {
            return Some(pid);
        }
        pid = table.parent(&pid)?;
    }
    None
}

pub fn trusted_target(table: &dyn Processes, sender: &str) -> Option<BTreeMap<String, String>> {
    let environment = table.environment(&first_process_outside(table, sender)?);
    let target: BTreeMap<String, String> = [("tmux_pane", "TMUX_PANE"), ("kitty_pid", "KITTY_PID"), ("kitty_window", "KITTY_WINDOW_ID"), ("kitty_listen", "KITTY_LISTEN_ON")]
        .iter().filter_map(|(key, variable)| environment.get(*variable).filter(|value| !value.is_empty()).map(|value| (key.to_string(), value.clone())))
        .collect();
    let fields: Map<String, Value> = target.iter().map(|(key, value)| (key.clone(), json!(value))).collect();
    validate_focus_target(&fields).ok().map(|_| target)
}

/// The folder the sender's sandbox started in: the working directory of its first process outside
/// the sandbox (greywall, bwrap), which nothing inside can change.
pub fn sandbox_directory(table: &dyn Processes, sender: &str) -> Option<String> {
    table.cwd(&first_process_outside(table, sender)?)
}

pub struct Proc;

impl Processes for Proc {
    fn pids(&self) -> Vec<String> {
        fs::read_dir("/proc").map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.chars().all(|c| c.is_ascii_digit())).collect()).unwrap_or_default()
    }

    fn comm(&self, pid: &str) -> String {
        fs::read_to_string(format!("/proc/{pid}/comm")).map(|name| name.trim().to_string()).unwrap_or_default()
    }

    fn command(&self, pid: &str) -> Vec<String> {
        fs::read(format!("/proc/{pid}/cmdline")).map(|bytes| bytes.split(|b| *b == 0).filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned()).collect()).unwrap_or_default()
    }

    fn environment(&self, pid: &str) -> HashMap<String, String> {
        fs::read(format!("/proc/{pid}/environ")).map(|bytes| bytes.split(|b| *b == 0).filter(|e| !e.is_empty()).filter_map(|entry| {
            let text = String::from_utf8_lossy(entry);
            text.split_once('=').map(|(name, value)| (name.to_string(), value.to_string()))
        }).collect()).unwrap_or_default()
    }

    fn cwd(&self, pid: &str) -> Option<String> {
        fs::read_link(format!("/proc/{pid}/cwd")).ok().map(|path| path.to_string_lossy().into_owned())
    }

    fn parent(&self, pid: &str) -> Option<String> {
        let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        status.lines().find_map(|line| line.strip_prefix("PPid:")).map(|parent| parent.trim().to_string()).filter(|parent| parent != "0")
    }

    fn pid_namespace(&self, pid: &str) -> Option<String> {
        fs::read_link(format!("/proc/{pid}/ns/pid")).ok().map(|link| link.to_string_lossy().into_owned())
    }
    fn start_time(&self, pid: &str) -> Option<String> {
        parse_process_start_time(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }
    fn boot_id(&self) -> Option<String> {
        fs::read_to_string("/proc/sys/kernel/random/boot_id").ok().map(|boot| boot.trim().to_string())
    }
}

/// Pids of the Claude launchers and session binaries, and of opencode. Since Claude Code 2.1.300 a
/// session is a tree: the launcher `claude`, `claude daemon run` (the launcher's environment
/// inherited, and it outlives the launcher), a `bg-pty-host`, and the session binary named after
/// the version with `--session-id <id>`; only the launcher and the session count. opencode is one
/// process named opencode, its session id on no command line: its pane keeps it alive.
pub fn claude_processes(table: &dyn Processes) -> Vec<String> {
    let version = &*CLAUDE_VERSION_NAME;
    let infrastructure = ["daemon", "bg-pty-host", "bg-spare"];
    let mut found: Vec<String> = table.pids().into_iter().filter(|pid| {
        let name = table.comm(pid);
        if agents::AGENTS.iter().any(|agent| agent.process == Some(name.as_str())) {
            return true;
        }
        if name != "claude" && !version.is_match(&name) {
            return false;
        }
        let command = table.command(pid);
        // The pty host's argv[0] is the two words "claude bg-pty-host"; the daemon's role is argv[1].
        let mut leading: Vec<String> = command.first().map(|first| first.split_whitespace().skip(1).map(str::to_string).collect()).unwrap_or_default();
        leading.extend(command.get(1).cloned());
        !leading.iter().any(|word| infrastructure.contains(&word.as_str()))
    }).collect();
    found.sort_by_key(|pid| pid.parse::<u64>().unwrap_or(0));
    found
}

/// The session ids on the command lines of the running session binaries.
pub fn live_session_ids_in(table: &dyn Processes) -> HashSet<String> {
    session_ids_of(table, &claude_processes(table))
}

fn session_ids_of(table: &dyn Processes, processes: &[String]) -> HashSet<String> {
    processes.iter().flat_map(|pid| {
        let command = table.command(pid);
        command.windows(2).filter(|pair| pair[0] == "--session-id" && agents::is_session_id(&pair[1])).map(|pair| pair[1].clone()).collect::<Vec<_>>()
    }).collect()
}

/// The tmux panes in which a Claude process runs, from its environment: the rule for sessions
/// whose id is not on a command line (older Claude versions).
pub fn claude_panes_in(table: &dyn Processes) -> HashSet<String> {
    panes_of(table, &claude_processes(table))
}

fn panes_of(table: &dyn Processes, processes: &[String]) -> HashSet<String> {
    processes.iter().filter_map(|pid| table.environment(pid).get("TMUX_PANE").cloned()).collect()
}

pub fn live_session_ids() -> HashSet<String> {
    live_session_ids_in(&Proc)
}

/// Bind a live conversation to its launcher's pane and tmux server, not a recyclable pane ID.
pub fn session_owns_pane_in(table: &dyn Processes, session: &str, pane: &str, server: &str) -> bool {
    if !agents::is_session_id(session) {
        return false;
    }
    claude_processes(table).iter().any(|pid| {
        let command = table.command(pid);
        let environment = table.environment(pid);
        let identity = command.windows(2)
            .find(|pair| matches!(pair[0].as_str(), "--session-id" | "--session"))
            .map(|pair| pair[1].as_str())
            .or_else(|| environment.get("CLAUDE_CODE_SESSION_ID").map(String::as_str));
        if identity != Some(session) {
            return false;
        }
        let Some(launcher) = first_process_outside(table, pid) else { return false };
        let trusted = table.environment(&launcher);
        trusted.get("TMUX_PANE").map(String::as_str) == Some(pane)
            && trusted.get("TMUX").and_then(|value| value.rsplit(',').nth(1)) == Some(server)
    })
}

pub fn claude_panes() -> HashSet<String> {
    claude_panes_in(&Proc)
}

/// Game mode on, or a game's window focused.
pub fn is_game(submap: &str, window_class: &str) -> bool {
    submap == "gamemode" || GAME_CLASS.is_match(window_class)
}

impl System for RealSystem {
    fn run(&mut self, command: &[&str], ok: &[i32]) -> bool {
        match output_within(command, COMMAND_TIMEOUT) {
            None => {
                log(&format!("{} failed or timed out", command[0]));
                false
            }
            Some((code, stdout, stderr)) if !ok.contains(&code) => {
                log(&format!("{} exited {code}: {}", command.join(" "), (stdout + &stderr).trim()));
                false
            }
            Some(_) => true,
        }
    }

    fn tmux(&mut self, arguments: &[&str]) -> Option<String> {
        let mut command = vec!["tmux"];
        command.extend_from_slice(arguments);
        output_within(&command, COMMAND_TIMEOUT).filter(|(code, _, _)| *code == 0).map(|(_, stdout, _)| stdout)
    }

    fn open_window(&mut self, command: &[&str]) -> Option<Box<dyn Window>> {
        use std::os::unix::process::CommandExt;
        let mut process = Command::new(command[0]);
        process.args(&command[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0);
        match process.spawn() {
            Ok(child) => Some(Box::new(child)),
            Err(error) => {
                log(&format!("cannot open {}: {error}", command[0]));
                None
            }
        }
    }

    fn claude_workspace_active(&mut self) -> bool {
        output_within(&["hyprctl", "-j", "activeworkspace"], COMMAND_TIMEOUT).filter(|(code, _, _)| *code == 0)
            .and_then(|(_, stdout, _)| serde_json::from_str::<Value>(&stdout).ok())
            .is_some_and(|workspace| workspace.get("id").and_then(Value::as_i64) == Some(CLAUDE_WORKSPACE))
    }

    /// Game mode on (the gamemode submap) or a game window focused: banners and approval windows
    /// wait. A fullscreen video is not a game. Asked at most every 2 seconds.
    fn gaming(&mut self) -> bool {
        if self.gaming_cache.0.is_some_and(|checked| checked.elapsed() < Duration::from_secs(2)) {
            return self.gaming_cache.1;
        }
        let submap = output_within(&["hyprctl", "submap"], COMMAND_TIMEOUT).map(|(_, out, _)| out.trim().to_string()).unwrap_or_default();
        let class = output_within(&["hyprctl", "-j", "activewindow"], COMMAND_TIMEOUT).and_then(|(_, out, _)| serde_json::from_str::<Value>(&out).ok())
            .and_then(|window| window.get("class").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
        let now = is_game(&submap, &class);
        if now != self.gaming_cache.1 {
            log(if now { "gaming: banners and approval windows wait" } else { "no game: banners and approval windows shown again" });
        }
        self.gaming_cache = (Some(Instant::now()), now);
        now
    }

    /// Whether the calendar shows a meeting, as khal last said: asked at most once a minute, on a
    /// thread of its own, so the answer can be up to a minute and a khal run late.
    fn in_meeting(&mut self) -> bool {
        use std::sync::atomic::Ordering;
        let answer = self.meeting_cache.1.clone();
        if self.meeting_cache.0.is_none_or(|checked| checked.elapsed() >= Duration::from_secs(60)) {
            self.meeting_cache.0 = Some(Instant::now());
            let answer = answer.clone();
            std::thread::spawn(move || {
                let now = output_within(&["khal", "list", "now", "1m", "--format", "{start}|{end}|{title}"], COMMAND_TIMEOUT)
                    .filter(|(code, _, _)| *code == 0).is_some_and(|(_, out, _)| events_now(&out, self::now()));
                if answer.swap(now, Ordering::SeqCst) != now {
                    log(if now { "meeting under way: Claude banners held until it ends" } else { "no meeting: banners shown again" });
                }
            });
        }
        answer.load(Ordering::SeqCst)
    }

    fn live_session_ids(&mut self) -> HashSet<String> {
        self.scanned().0
    }

    fn claude_panes(&mut self) -> HashSet<String> {
        self.scanned().1
    }

    fn process_environment(&mut self, pid: &str) -> HashMap<String, String> {
        Proc.environment(pid)
    }

    fn process_alive(&mut self, pid: &str) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    fn sender_target(&mut self, sender: u32) -> Option<BTreeMap<String, String>> {
        trusted_target(&Proc, &sender.to_string())
    }

    fn sender_directory(&mut self, sender: u32) -> Option<String> {
        sandbox_directory(&Proc, &sender.to_string())
    }
    fn sender_runtime(&mut self, sender: u32) -> Option<String> {
        trusted_runtime(&Proc, &sender.to_string())
    }
    fn runtime_is_live(&mut self, runtime: &str) -> bool {
        runtime_is_live(&Proc, runtime)
    }
}
