//! The running sessions (the daemon's snapshot, plus claude-retry-* tmux sessions it does not
//! know), what the column adds to them (status line facts, titles, pins, groups), past sessions,
//! the view state, and the header lines' sources: the restore offer, greyproxy, start files.

use crate::paths;
use crate::util::{self, collapse, now, now_secs, project_name, read_json};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

pub const GLYPHS: [(&str, &str); 5] = [("working", "●"), ("waiting", "○"), ("needs-you", "!"), ("unknown", "·"), ("recent", "↻")];
pub const CONTEXT_COMPACT_AT: f64 = 90.0;
const TITLE_SCAN_BYTES: u64 = 2 * 1024 * 1024;
/// ~/.claude/history.jsonl is Claude's, which every sandbox writes: read up to this much.
const HISTORY_MAX: u64 = 64 * 1024 * 1024;
const TITLE_RETRY_S: f64 = 30.0;
const RESTORE_WINDOW_S: i64 = 600;
const RESTORE_OFFER_HOURS: i64 = 6;
const GREYPROXY_RECENT_MINUTES: i64 = 10;
/// What a session reads only when it starts: after a change, the sessions started before it run
/// on the old version until they restart. Label -> path (a missing file is skipped). Each agent
/// lists the ones it reads (agents::Agent::start_files).
pub const START_FILES: [(&str, &str); 13] = [
    ("CLAUDE.md", "~/.claude/CLAUDE.md"), ("settings", "~/.claude/settings.json"),
    ("plugins", "~/.claude/plugins/installed_plugins.json"), ("ADHD mode", "~/.claude/.i-have-adhd-always"),
    ("session hook", "~/.claude/hooks/agent-environment.sh"), ("sandbox", "~/.config/greywall/greywall.json"),
    ("sandbox profile", "~/.config/greywall/learned/claude.json"), ("launcher", "~/.local/bin/greywall-agents/claude"),
    ("shared launcher", "~/.local/bin/greywall-agents/launcher-common.sh"),
    ("opencode config", "~/.config/opencode/opencode.jsonc"), ("opencode plugin", "~/.config/opencode/plugins/cockpit.ts"),
    ("opencode profile", "~/.config/greywall/learned/opencode.json"), ("opencode launcher", "~/.local/bin/greywall-agents/opencode"),
];

pub fn glyph(state: &str) -> &'static str {
    GLYPHS.iter().find(|(name, _)| *name == state).map(|(_, glyph)| *glyph).unwrap_or("·")
}

/// The Claude Design canvas a session works on (claude-design link, from inside the session).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Design {
    pub url: String,
    pub title: String,
}

/// https://claude.ai/artifact/<id> from a canvas address or a chat address with ?artifact=<id>;
/// None for anything else. Same rule as claude-design.
pub fn canvas_address(text: &str) -> Option<String> {
    let text = text.trim();
    static CANVAS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^https://claude\.ai/(?:code/)?artifact/([A-Za-z0-9-]{8,64})/?$").unwrap());
    static CHAT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^https://claude\.ai/chat/[A-Za-z0-9-]+\?(?:.*&)?artifact=([A-Za-z0-9-]{8,64})(?:&.*)?$").unwrap());
    let id = CANVAS.captures(text).or_else(|| CHAT.captures(text))?.get(1)?.as_str().to_string();
    Some(format!("https://claude.ai/artifact/{id}"))
}

pub fn design_of(session_id: &str) -> Option<Design> {
    let link = util::read_untrusted_json(&paths::designs().join(format!("{session_id}.json")))?;
    let url = canvas_address(link.get("url")?.as_str()?)?;
    let title = link.get("title").and_then(Value::as_str).unwrap_or("canvas").to_string();
    Some(Design { url, title })
}

pub fn role_of(session_id: &str) -> String {
    let role = util::read_untrusted_text(&paths::designs().join(format!("{session_id}.role")), 64).unwrap_or_default();
    match role.trim() {
        role @ ("designer" | "developer") => role.to_string(),
        _ => String::new(),
    }
}

/// The first message of a design session: link the card, watch the comments, then wait.
pub fn design_prompt(url: &str) -> String {
    format!("This is a design session for the Claude Design canvas {url}. First read it with the Artifact tool, then run \
             claude-design link {url} '<the canvas title>' so the cockpit's card shows it, and watch the canvas's comments \
             with ArtifactComments so the comments I leave there reach you. Then tell me in one line what the canvas holds and wait \
             for what I want changed; after each change, say in one line what changed.")
}

/// The status line's last report for a session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub model: String,
    pub effort: String,
    pub context: Option<f64>,
    pub remote: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub session: String,
    pub state: String,
    pub project: String,
    pub cwd: String,
    pub task: String,
    pub since: i64,
    pub started: Option<i64>,
    pub tmux_pane: String,
    pub kitty_pid: String,
    pub request: String,
    pub detail: String,
    pub outcome: String,
    pub update: bool,
    /// Started with "this terminal": shown and usable here, left out of what acts on every session.
    pub terminal: bool,
    /// Billed to the Console's API credits, not the subscription ($ switches it).
    pub credits: bool,
    /// Commits of its repository on no remote yet, as of its last start or turn end (or a U push).
    pub unpushed: u64,
    /// Background agents, and other background work (shells, monitors, workflows), in flight when
    /// its last turn ended.
    pub agents: u64,
    pub background: u64,
    /// Desktop apps it drives through an attached MCP server, as the hook saw them: "figcraft:on".
    pub apps: String,
    /// The name of the agent that runs it (agents::AGENTS).
    pub agent: String,
    // Added by the column.
    pub design: Option<Design>,
    /// "designer" or "developer", as the session declared it (claude-design role), or empty.
    pub role: String,
    pub facts: Facts,
    pub title: String,
    pub pinned: bool,
    pub group_size: usize,
    pub group_position: usize,
}

fn text(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

impl Session {
    pub fn is_external(&self) -> bool {
        self.tmux_pane.is_empty() && self.kitty_pid.is_empty()
    }

    pub fn from_value(row: &Value) -> Option<Session> {
        let session = row.get("session")?.as_str()?.to_string();
        Some(Session {
            session,
            state: row.get("state").and_then(Value::as_str).unwrap_or("waiting").into(),
            project: row.get("project").and_then(Value::as_str).unwrap_or("?").into(),
            cwd: text(row, "cwd"),
            task: text(row, "task"),
            since: row.get("since").and_then(Value::as_f64).map(|since| since as i64).unwrap_or_else(now_secs),
            started: row.get("started").and_then(Value::as_f64).map(|started| started as i64),
            tmux_pane: text(row, "tmux_pane"),
            kitty_pid: text(row, "kitty_pid"),
            request: text(row, "request"),
            detail: text(row, "detail"),
            outcome: text(row, "outcome"),
            update: row.get("update").and_then(Value::as_bool).unwrap_or(false),
            terminal: row.get("terminal").and_then(Value::as_bool).unwrap_or(false),
            credits: row.get("credits").and_then(Value::as_bool).unwrap_or(false),
            unpushed: row.get("unpushed").and_then(Value::as_u64).unwrap_or(0),
            agents: row.get("agents").and_then(Value::as_u64).unwrap_or(0),
            background: row.get("background").and_then(Value::as_u64).unwrap_or(0),
            apps: text(row, "apps"),
            agent: row.get("agent").and_then(Value::as_str).unwrap_or(crate::agents::DEFAULT).into(),
            ..Session::default()
        })
    }

    /// The agent that runs it (its row in agents::AGENTS).
    pub fn agent(&self) -> &'static crate::agents::Agent {
        crate::agents::get(&self.agent)
    }
}

pub fn reported() -> Vec<Session> {
    read_json(&paths::snapshot())
        .and_then(|snapshot| snapshot.get("sessions")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Session::from_value)
        .collect()
}

/// One row per claude-retry-* tmux session: the pane claude runs in, its path and title.
fn tmux_claude_panes() -> Vec<[String; 5]> {
    let format = "#{session_name}\t#{pane_id}\t#{pane_current_path}\t#{pane_title}\t#{session_created}";
    util::output_of("tmux", &["list-panes", "-a", "-F", format])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t').map(str::to_string);
            let row = [parts.next()?, parts.next().unwrap_or_default(), parts.next().unwrap_or_default(),
                       parts.next().unwrap_or_default(), parts.next().unwrap_or_default()];
            row[0].starts_with("claude-retry-").then_some(row)
        })
        .collect()
}

/// Reported sessions first, then tmux sessions the daemon does not know about. Sessions without a
/// known terminal target stay visible; their cards cannot control them.
pub fn running() -> Vec<Session> {
    let mut sessions = reported();
    let known: HashSet<String> = sessions.iter().map(|session| session.tmux_pane.clone()).collect();
    for [name, pane, path, title, created] in tmux_claude_panes() {
        if known.contains(&pane) {
            continue;
        }
        let created = created.parse::<i64>().unwrap_or_else(|_| now_secs());
        let agent = crate::agents::of_tmux_session(&name).map_or(crate::agents::DEFAULT, |agent| agent.name);
        sessions.push(Session {
            session: name, state: "unknown".into(), project: project_name(&path),
            task: title.trim_start_matches(['✳', '◐', '◑', ' ']).to_string(), since: created, started: Some(created),
            agent: agent.into(), tmux_pane: pane, cwd: path, ..Session::default()
        });
    }
    sessions
}

pub fn statusline_facts(session_id: &str) -> Facts {
    let Some(facts) = read_json(&paths::statusline_dir().join(format!("statusline-{session_id}.json"))) else { return Facts::default() };
    Facts {
        model: facts.get("model").and_then(Value::as_str).unwrap_or("").into(),
        effort: facts.get("effort").and_then(Value::as_str).unwrap_or("").into(),
        context: facts.get("context").and_then(Value::as_f64),
        remote: facts.get("remote").map(|remote| remote.as_bool().unwrap_or(!remote.is_null())).unwrap_or(false),
    }
}

/// Stable order, a repository's sessions together: pinned first, then each project at the place
/// of its first session, its other sessions (parallel clones, a second session) right under it.
pub fn grouped_by_project(mut sessions: Vec<Session>) -> Vec<Session> {
    sessions.sort_by(|a, b| {
        (!a.pinned, a.started.unwrap_or(a.since), &a.session).cmp(&(!b.pinned, b.started.unwrap_or(b.since), &b.session))
    });
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<Session>> = HashMap::new();
    for session in sessions {
        if !groups.contains_key(&session.project) {
            order.push(session.project.clone());
        }
        groups.entry(session.project.clone()).or_default().push(session);
    }
    let mut result = Vec::new();
    for project in order {
        let members = groups.remove(&project).unwrap_or_default();
        let size = members.len();
        for (position, mut session) in members.into_iter().enumerate() {
            session.group_size = size;
            session.group_position = position;
            result.push(session);
        }
    }
    result
}

/// The task line, unless it is one of Claude's internal messages (<agent-message …>).
/// A dotfiles clone (the work clone, or <clones>/dotfiles/<name>): its commits reach ~/dotfiles
/// through apply-dotfiles, never a push, so its count is of commits not applied yet (the hook).
pub fn is_dotfiles_clone(cwd: &str) -> bool {
    let cwd = Path::new(cwd);
    cwd.starts_with(paths::dotfiles_work()) || cwd.starts_with(paths::clones_dir().join("dotfiles"))
}

/// Commits ~/dotfiles (the live checkout, which no session writes) holds and its GitHub upstream
/// lacks: what U on a dotfiles card pushes after the apply. 0 when it tracks no upstream.
pub fn dotfiles_unpushed() -> usize {
    let live = paths::dotfiles().to_string_lossy().into_owned();
    util::output_of("git", &["-C", &live, "rev-list", "--count", "@{upstream}..HEAD"]).and_then(|count| count.trim().parse().ok()).unwrap_or(0)
}

/// The card's "◷ 3 agents + background": its background work, agents counted; None without any.
pub fn background_badge(session: &Session) -> Option<String> {
    let agents = match session.agents {
        0 => None,
        1 => Some("1 agent".to_string()),
        count => Some(format!("{count} agents")),
    };
    let other = (session.background > 0).then_some("background");
    match (agents, other) {
        (None, None) => None,
        (Some(agents), None) => Some(format!("◷ {agents}")),
        (None, Some(other)) => Some(format!("◷ {other}")),
        (Some(agents), Some(other)) => Some(format!("◷ {agents} + {other}")),
    }
}

pub fn visible_task(session: &Session) -> String {
    let task = session.task.trim();
    if task.starts_with('<') { String::new() } else { task.to_string() }
}

/// What a session needing you asks: the dialog's lines from its pane, else the hook's sentence.
pub fn waiting_request(session: &Session) -> String {
    if session.state != "needs-you" {
        return String::new();
    }
    if !session.detail.is_empty() { session.detail.clone() } else { session.request.clone() }
}

pub fn next_attention_index(sessions: &[Session], selected: usize) -> Option<usize> {
    let candidates: Vec<usize> = sessions.iter().enumerate()
        .filter(|(_, session)| !session.is_external() && session.state != "working" && session.state != "recent")
        .map(|(index, _)| index)
        .collect();
    candidates.iter().find(|index| **index > selected).or(candidates.first()).copied()
}

/// The agent's session title, or empty when none has been written yet.
pub fn read_title(session_id: &str) -> String {
    if crate::agents::of_session_id(session_id).is_some_and(|agent| agent.name == "opencode") {
        return match opencode_title(session_id) {
            Ok(title) => title,
            Err(error) => {
                util::log_event("WARN", &format!("cannot read opencode title for {session_id}: {error}"));
                String::new()
            }
        };
    }
    let Some(path) = transcript_of(session_id) else { return String::new() };
    let Ok(head) = util::read_untrusted(&path, TITLE_SCAN_BYTES) else { return String::new() };
    let mut title = String::new();
    for line in head.split(|byte| *byte == b'\n') {
        if !line.windows(8).any(|window| window == b"ai-title") {
            continue;
        }
        if let Ok(record) = serde_json::from_slice::<Value>(line) {
            if record.get("type").and_then(Value::as_str) == Some("ai-title") {
                if let Some(found) = record.get("aiTitle").and_then(Value::as_str).filter(|found| !found.is_empty()) {
                    title = found.to_string();
                }
            }
        }
    }
    title
}

fn opencode_title(session_id: &str) -> rusqlite::Result<String> {
    use rusqlite::{Connection, OpenFlags, OptionalExtension};
    let database = paths::opencode_database();
    if !database.exists() {
        return Ok(String::new());
    }
    let source = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(std::time::Duration::from_secs(1))?;
    let title: Option<String> = source.query_row(
        "select coalesce(title, '') from session_v2 where id = ?1 and parent_id is null",
        [session_id], |row| row.get(0)).optional()?;
    Ok(title.map(|title| collapse(&title)).unwrap_or_default())
}

fn transcript_of(session_id: &str) -> Option<std::path::PathBuf> {
    fs::read_dir(paths::transcripts()).ok()?.flatten()
        .map(|project| project.path().join(format!("{session_id}.jsonl")))
        .find(|path| path.exists())
}

/// A past session from history.jsonl.
#[derive(Clone, Debug, PartialEq)]
pub struct PastSession {
    pub session: String,
    pub cwd: String,
    pub project: String,
    pub since: i64,
    pub prompt: String,
}

/// The past sessions H offers from opencode's database: the newest ones.
const OPENCODE_PAST_MAX: usize = 200;

/// opencode's past sessions, newest first, each with its title: the top-level ones of its database
/// (session_v2; a subagent's has a parent), not archived. Read-only, as its sandboxes write it;
/// none when opencode never ran.
pub fn opencode_past_sessions(database: &Path) -> rusqlite::Result<Vec<(PastSession, String)>> {
    use rusqlite::{Connection, OpenFlags};
    if !database.exists() {
        return Ok(Vec::new());
    }
    let source = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(std::time::Duration::from_secs(1))?;
    let mut statement = source.prepare(
        "select id, directory, coalesce(title, ''), time_updated from session_v2
         where parent_id is null and time_archived is null order by time_updated desc limit ?1")?;
    let rows = statement.query_map([OPENCODE_PAST_MAX as i64], |row| {
        let (session, cwd, title, updated_ms): (String, String, String, i64) = (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
        Ok((PastSession { project: project_name(&cwd), session, cwd, since: updated_ms / 1000, prompt: String::new() }, collapse(&title)))
    })?;
    rows.collect()
}

/// Past sessions from history.jsonl (re-read when it changes) with lazily loaded titles.
#[derive(Default)]
pub struct History {
    mtime: Option<std::time::SystemTime>,
    pub entries: Vec<PastSession>,
    titles: HashMap<String, String>,
    title_checked: HashMap<String, f64>,
}

impl History {
    pub fn refresh(&mut self) {
        let Ok(mtime) = fs::metadata(paths::history()).and_then(|m| m.modified()) else {
            self.entries.clear();
            return;
        };
        if self.mtime == Some(mtime) {
            return;
        }
        self.mtime = Some(mtime);
        let mut latest: HashMap<String, PastSession> = HashMap::new();
        for line in util::read_untrusted_text(&paths::history(), HISTORY_MAX).unwrap_or_default().lines() {
            let Ok(row) = serde_json::from_str::<Value>(line) else { continue };
            let (Some(session), Some(project)) = (row.get("sessionId").and_then(Value::as_str), row.get("project").and_then(Value::as_str)) else { continue };
            if session.is_empty() || project.is_empty() {
                continue;
            }
            let since = row.get("timestamp").and_then(Value::as_i64).unwrap_or(0) / 1000;
            let prompt = collapse(row.get("display").and_then(Value::as_str).unwrap_or(""));
            let entry = latest.entry(session.to_string()).or_insert_with(|| PastSession {
                session: session.into(), cwd: project.into(), project: project_name(project), since: 0, prompt: String::new(),
            });
            entry.since = entry.since.max(since);
            // Slash commands and paste placeholders say nothing about the session: keep the last real prompt.
            if !prompt.is_empty() && !prompt.starts_with('/') && !prompt.starts_with("[Pasted text") {
                entry.prompt = prompt;
            }
        }
        let mut entries: Vec<_> = latest.into_values().collect();
        entries.sort_by(|a, b| b.since.cmp(&a.since));
        self.entries = entries;
    }

    pub fn title(&self, session_id: &str) -> String {
        self.titles.get(session_id).cloned().unwrap_or_default()
    }

    /// Reads the titles not known yet. Claude writes a title after the first exchange, so an empty
    /// one is read again every TITLE_RETRY_S.
    pub fn load_titles(&mut self, session_ids: &[String], mut budget: usize) {
        let time = now();
        for session_id in session_ids {
            if budget == 0 {
                return;
            }
            let known = self.titles.get(session_id).is_some_and(|title| !title.is_empty());
            if known || time - self.title_checked.get(session_id).copied().unwrap_or(0.0) < TITLE_RETRY_S {
                continue;
            }
            self.titles.insert(session_id.clone(), read_title(session_id));
            self.title_checked.insert(session_id.clone(), time);
            budget -= 1;
        }
    }
}

/// What Theo chose: pins, hidden past sessions, the boot whose sessions were brought back.
/// Unknown keys of view.json are kept.
pub struct ViewState {
    saved: Value,
    pub pinned: Vec<String>,
    pub restored_boot: i64,
}

impl ViewState {
    pub fn load() -> Self {
        let saved = match read_json(&paths::view_state()) { Some(value @ Value::Object(_)) => value, _ => json!({}) };
        let pinned = saved.get("pinned").and_then(Value::as_array).map(|items| items.iter().filter_map(|i| i.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let restored_boot = saved.get("restored_boot").and_then(Value::as_i64).unwrap_or(0);
        Self { saved, pinned, restored_boot }
    }

    pub fn save(&mut self) {
        self.saved["pinned"] = json!(self.pinned);
        self.saved["restored_boot"] = json!(self.restored_boot);
        let _ = util::write_json(&paths::view_state(), &self.saved);
    }

    pub fn toggle_pin(&mut self, session: &str) {
        match self.pinned.iter().position(|pinned| pinned == session) {
            Some(index) => { self.pinned.remove(index); }
            None => self.pinned.push(session.to_string()),
        }
        self.save();
    }
}

pub fn boot_time() -> i64 {
    fs::read_to_string("/proc/uptime").ok()
        .and_then(|uptime| uptime.split_whitespace().next()?.parse::<f64>().ok())
        .map(|uptime| (now() - uptime) as i64)
        .unwrap_or(0)
}

/// A session to bring back after a reboot.
#[derive(Clone, Debug, PartialEq)]
pub struct Restorable {
    pub session: String,
    pub cwd: String,
    pub project: String,
}

/// The sessions still alive when the machine went down: seen before this boot, within
/// RESTORE_WINDOW_S of the last one seen, not ended on purpose, not running now. One nobody typed in
/// yet comes back too, as a fresh session in its folder (claude-sessions restore): leaving it out
/// made sessions opened and not used yet vanish at each reboot. One started in a terminal of its
/// own ("this terminal") is that terminal's to bring back, not the cockpit's.
pub fn sessions_to_restore(recent: &Value, ended: &HashSet<String>, running: &HashSet<String>, boot: i64) -> Vec<Restorable> {
    let Some(recent) = recent.as_object() else { return Vec::new() };
    let before: Vec<(&String, &Value)> = recent.iter()
        .filter(|(_, row)| row.get("last_seen").and_then(Value::as_i64).unwrap_or(0) < boot && row.get("cwd").and_then(Value::as_str).is_some_and(|cwd| !cwd.is_empty()))
        .collect();
    let Some(last) = before.iter().filter_map(|(_, row)| row.get("last_seen").and_then(Value::as_i64)).max() else { return Vec::new() };
    let mut rows: Vec<Restorable> = before.into_iter()
        .filter(|(session, row)| row.get("last_seen").and_then(Value::as_i64).unwrap_or(0) >= last - RESTORE_WINDOW_S
                && !ended.contains(*session) && !running.contains(*session)
                && !row.get("terminal").and_then(Value::as_bool).unwrap_or(false))
        .map(|(session, row)| Restorable {
            session: session.clone(),
            cwd: row.get("cwd").and_then(Value::as_str).unwrap_or("").into(),
            project: row.get("project").and_then(Value::as_str).unwrap_or("").into(),
        })
        .collect();
    rows.sort_by(|a, b| a.project.cmp(&b.project));
    rows
}

pub fn restore_offer(view: &ViewState, running: &HashSet<String>) -> Vec<Restorable> {
    let boot = boot_time();
    if boot == 0 || view.restored_boot == boot || now_secs() - boot > RESTORE_OFFER_HOURS * 3600 {
        return Vec::new();
    }
    let Some(recent) = read_json(&paths::recent_sessions()) else { return Vec::new() };
    let ended: HashSet<String> = fs::read_to_string(paths::ended_sessions()).unwrap_or_default().split_whitespace().map(str::to_string).collect();
    sessions_to_restore(&recent, &ended, running, boot)
}

/// host:port of the requests greyproxy holds for approval, retried in the last few minutes.
pub fn greyproxy_held(database: &Path) -> Vec<String> {
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI;
    let Ok(connection) = rusqlite::Connection::open_with_flags(database, flags) else { return Vec::new() };
    let _ = connection.busy_timeout(std::time::Duration::from_secs(1));
    let query = "select destination_host, destination_port from pending_requests where last_seen >= datetime('now', ?) order by last_seen desc";
    let Ok(mut statement) = connection.prepare(query) else { return Vec::new() };
    let rows = statement.query_map([format!("-{GREYPROXY_RECENT_MINUTES} minutes")], |row| {
        Ok(format!("{}:{}", row.get::<_, String>(0)?, row.get::<_, rusqlite::types::Value>(1).map(|port| match port {
            rusqlite::types::Value::Integer(port) => port.to_string(),
            rusqlite::types::Value::Text(port) => port,
            _ => String::new(),
        })?))
    });
    rows.map(|rows| rows.flatten().collect()).unwrap_or_default()
}

pub fn start_file_mtimes() -> Vec<(&'static str, f64)> {
    START_FILES.iter().filter_map(|(label, path)| {
        let modified = fs::metadata(paths::expand(path)).and_then(|m| m.modified()).ok()?;
        Some((*label, modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64()))
    }).collect()
}

/// What changed after the session started, so it does not have it yet; none without a start time.
pub fn changed_since_start(session: &Session, mtimes: &[(&'static str, f64)]) -> Vec<&'static str> {
    let Some(started) = session.started else { return Vec::new() };
    let reads = |label: &str| session.agent().start_files.contains(&label);
    mtimes.iter().filter(|(label, mtime)| *mtime > started as f64 && reads(label)).map(|(label, _)| *label).collect()
}

/// Project directories from Claude's history, most recently used first.
fn recent_projects() -> Vec<String> {
    let mut last_used: HashMap<String, f64> = HashMap::new();
    for line in util::read_untrusted_text(&paths::history(), HISTORY_MAX).unwrap_or_default().lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        if let (Some(project), Some(stamp)) = (entry.get("project").and_then(Value::as_str), entry.get("timestamp").and_then(Value::as_f64)) {
            let used = last_used.entry(project.to_string()).or_insert(0.0);
            *used = used.max(stamp);
        }
    }
    let mut projects: Vec<_> = last_used.into_iter().collect();
    projects.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    projects.into_iter().map(|(project, _)| project).collect()
}

/// The picker's list: the project directories (~/IdeaProjects/*, ~/dotfiles), the ones used with
/// Claude most recently first (a session in a subfolder counts for its project), the rest by name.
pub fn project_choices() -> Vec<String> {
    let mut known: Vec<String> = fs::read_dir(paths::home().join("IdeaProjects")).into_iter().flatten().flatten()
        .filter(|entry| entry.path().is_dir() && !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path().to_string_lossy().into_owned())
        .collect();
    known.sort();
    let mut ranked: Vec<String> = Vec::new();
    for used in recent_projects() {
        if let Some(owner) = known.iter().find(|project| used == **project || used.starts_with(&format!("{project}/"))) {
            if !ranked.contains(owner) {
                ranked.push(owner.clone());
            }
        }
    }
    let rest: Vec<String> = known.into_iter().filter(|project| !ranked.contains(project)).collect();
    ranked.extend(rest);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, project: &str, started: i64, pinned: bool) -> Session {
        Session { session: id.into(), project: project.into(), started: Some(started), pinned, ..Session::default() }
    }

    #[test]
    fn a_repositorys_sessions_are_grouped_under_its_first_card() {
        let grouped = grouped_by_project(vec![session("a", "opusline", 1, false), session("b", "runway", 2, false),
                                              session("c", "opusline", 3, false), session("d", "dotfiles", 4, true)]);
        let order: Vec<_> = grouped.iter().map(|s| (s.session.as_str(), s.group_size, s.group_position)).collect();
        assert_eq!(order, vec![("d", 1, 0), ("a", 2, 0), ("c", 2, 1), ("b", 1, 0)]);
    }

    #[test]
    fn sessions_alive_at_shutdown_are_offered_back() {
        let recent = json!({"a": {"cwd": "/a", "project": "a", "last_seen": 1000}, "b": {"cwd": "/b", "project": "b", "last_seen": 100},
                            "c": {"cwd": "/c", "project": "c", "last_seen": 990}, "d": {"cwd": "/d", "project": "d", "last_seen": 995},
                            "e": {"cwd": "/e", "project": "e", "last_seen": 998, "terminal": true}});
        let ended: HashSet<String> = ["c".to_string()].into();
        let offered = sessions_to_restore(&recent, &ended, &HashSet::new(), 2000);
        assert_eq!(offered.iter().map(|r| r.session.as_str()).collect::<Vec<_>>(), vec!["a", "d"]);
    }

    #[test]
    fn next_attention_wraps_to_the_first_waiting_one() {
        let rows: Vec<Session> = ["working", "waiting", "working", "needs-you"].iter()
            .map(|state| Session { state: state.to_string(), tmux_pane: "%1".into(), ..Session::default() }).collect();
        assert_eq!(next_attention_index(&rows, 1), Some(3));
        assert_eq!(next_attention_index(&rows, 3), Some(1));
    }

    #[test]
    fn changes_after_the_start_are_named() {
        let started = Session { started: Some(900), ..Session::default() };
        assert_eq!(changed_since_start(&started, &[("CLAUDE.md", 1000.0), ("settings", 800.0)]), vec!["CLAUDE.md"]);
        assert!(changed_since_start(&Session::default(), &[("CLAUDE.md", 1000.0)]).is_empty());
    }
}

/// All checkout roots served by the running instances. /proc preserves argument boundaries,
/// unlike pgrep -a, where a root containing spaces is indistinguishable from extra arguments.
pub fn craft_roots(app: &str) -> Option<HashSet<String>> {
    let processes = util::output_of("pgrep", &["-x", app])?;
    Some(processes.split_whitespace().filter_map(|pid| {
        let pid = pid.parse::<u32>().ok()?;
        write_root(&fs::read(format!("/proc/{pid}/cmdline")).ok()?)
    }).collect())
}

fn write_root(command_line: &[u8]) -> Option<String> {
    let arguments: Vec<_> = command_line.split(|byte| *byte == 0).collect();
    let mut roots = arguments.windows(2).filter(|pair| pair[0] == b"--automation-write-root");
    let root = std::str::from_utf8(roots.next()?[1]).ok()?;
    if root.is_empty() || roots.next().is_some() { return None; }
    Some(root.to_string())
}
