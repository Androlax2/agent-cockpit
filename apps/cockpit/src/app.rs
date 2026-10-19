//! The column itself: four stacked panels (Status, Sessions, Pull requests, Todo), the header and
//! the footer, the effort menu, the help, and every key. The focused panel takes half the column;
//! Status keeps its size.

use crate::costs::Costs;
use crate::errors::ErrorReporter;
use crate::paths;
use crate::reviews::{self, Review, Reviews};
use crate::sessions::{self, CONTEXT_COMPACT_AT, FOCUS_STATES, History, PastSession, Restorable, Session, ViewState};
use crate::status::{self, ClaudeStatus};
use crate::todo::{self, Entry, TodoList};
use crate::usage::{self, BudgetAlerts, Refresher, Window};
use crate::util::{self, age, current_session, manager, manager_in_background, money, now, project_name};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// kitty's 16-colour palette is Tokyo Night, so plain colour indexes match the rest of the desktop.
pub const RED: u8 = 1;
pub const GREEN: u8 = 2;
pub const YELLOW: u8 = 3;
pub const BLUE: u8 = 4;
pub const MAGENTA: u8 = 5;
pub const CYAN: u8 = 6;
pub const FG_DARK: u8 = 7;
pub const BG_HIGHLIGHT: u8 = 8;
pub const FG: u8 = 15;
const PROJECT_COLORS: [u8; 5] = [MAGENTA, CYAN, GREEN, BLUE, YELLOW];
const HEADER_LINES: u16 = 1;
const MIN_PANEL_LINES: u16 = 4;
const POLL: Duration = Duration::from_millis(250);
const DEFAULT_COLUMN_WIDTH: u16 = 60;
const TITLES_PER_TICK: usize = 8;
const GREYPROXY_POLL_S: f64 = 10.0;
/// How long a session waits idle at its prompt before the queue gives it work, and how often the
/// queue looks: long enough that Theo, reading what it just did, is not cut off.
const QUEUE_IDLE_S: i64 = 5 * 60;
const QUEUE_CHECK_S: f64 = 30.0;
const RESTORE_CHECK_S: f64 = 30.0;
const COSTS_LABEL: &str = "At API prices";
const CLAUDE_LABEL: &str = "Claude";
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const PANEL_NAMES: [&str; 4] = ["Status", "Sessions", "Pull requests", "Todo"];
const PANEL_KEYS: [&str; 4] = ["0", "1", "2", "3"];
const GLOBAL_HINTS: [(&str, &str); 6] = [("⇥", "panel"), ("/", "filter"), (":", "actions"), ("a", "new"), ("?", "help"), ("q", "close")];
const GLOBAL_HELP: [(&str, &str); 17] = [
    ("0-3", "focus a panel"), ("H", "resume a past session, searched by project and prompt"),
    ("^1 ^2 ^3", "focus a panel from inside the session (tmux)"), ("⇥ ⇧⇥", "next / previous panel"), ("j/k ↑↓", "move inside the panel"),
    ("/", "filter the focused panel, Esc clears"), (":", "every action, searchable"), ("a", "new session in a project"),
    ("A", "new session on its own branch, in a separate clone (parallel work on one repository)"),
    ("K", "the clones of A, + and reviews: delete those with no session, no uncommitted change and nothing unpushed"),
    ("V", "a design session: a Claude Design canvas, worked on by a session in a project (link, then project)"),
    ("D", "the digest: yesterday, today, plan usage, who waits, pull requests (also once a day by itself)"),
    ("P W G", "windows on top: pull requests, Claude on the web, greyproxy (also Super+Ctrl+P/A/G)"),
    ("≈ $", "today's Claude Code replies on this machine at API prices (the plan is flat); week and month in Status"),
    ("^⇥", "cursor between the column and the session (tmux)"),
    ("l →  ←", "into the session from any panel; ← from the session comes back"),
    ("q ^C", "close the manager window; sessions keep running"),
];
const LEGEND: [(&str, &str); 11] = [
    ("●", "working"), ("○", "waiting for you"), ("!", "needs you: permission or question"), ("·", "running, state unknown"),
    ("▌", "shown in the session pane"), ("⚲", "pinned"), ("⇅", "remote controlled"),
    ("▭ terminal", "started with \"this terminal\": no ^R, restore, queue, error hand-off or ask-to-run"),
    ("⬆", "Claude updated itself: r restarts on the new version"),
    ("/compact", "context over 90%: /compact in the session before it gets slow and costly"), ("high ▾", "effort level: click to change it"),
];

pub fn color(index: u8) -> Color {
    Color::Indexed(index)
}

pub fn style(fg: u8) -> Style {
    Style::default().fg(color(fg))
}

fn bold(fg: u8) -> Style {
    style(fg).add_modifier(Modifier::BOLD)
}

fn dim(fg: u8) -> Style {
    style(fg).add_modifier(Modifier::DIM)
}

fn on(fg: u8, bg: u8) -> Style {
    Style::default().fg(color(fg)).bg(color(bg))
}

pub fn project_color(project: &str) -> u8 {
    PROJECT_COLORS[project.chars().map(|c| c as usize).sum::<usize>() % PROJECT_COLORS.len()]
}

fn state_color(state: &str) -> u8 {
    match state { "working" => BLUE, "waiting" => YELLOW, "needs-you" => RED, "recent" => CYAN, _ => FG_DARK }
}

fn usage_color(percentage: Option<i64>) -> u8 {
    match percentage { None => FG_DARK, Some(p) if p >= 90 => RED, Some(p) if p >= 70 => YELLOW, Some(_) => MAGENTA }
}

fn context_color(percentage: Option<f64>) -> u8 {
    match percentage { None => FG_DARK, Some(p) if p >= 80.0 => RED, Some(p) if p >= 50.0 => YELLOW, Some(_) => GREEN }
}

fn claude_status_color(indicator: &str) -> u8 {
    match indicator { "none" => GREEN, "minor" => YELLOW, "major" | "critical" => RED, "maintenance" => CYAN, _ => YELLOW }
}

pub fn width_of(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// The text cut to `available` cells, with an ellipsis when cut.
pub fn fit(text: &str, available: usize) -> String {
    if available <= 1 {
        return String::new();
    }
    if width_of(text) <= available {
        return text.to_string();
    }
    let mut kept = String::new();
    let mut used = 0;
    for character in text.chars() {
        let cell = character.width().unwrap_or(0);
        if used + cell > available - 1 {
            break;
        }
        kept.push(character);
        used += cell;
    }
    kept + "…"
}

/// Writes text at (row, column), never past the column's last cell but one (like curses addnstr).
pub fn put(buffer: &mut Buffer, row: u16, column: u16, text: &str, style: Style) {
    let area = buffer.area;
    if row >= area.height || column + 1 >= area.width {
        return;
    }
    let available = (area.width - column - 1) as usize;
    // Replaces the cells' style like curses does; ratatui would add to it (a bold border under a
    // dim subtitle would stay bold).
    buffer.set_stringn(column, row, text, available, Style::reset().patch(style));
}

/// A right-aligned part of a line; a click on it runs its action.
#[derive(Clone)]
pub struct Part {
    pub text: String,
    pub style: Style,
    pub action: Option<&'static str>,
}

fn part(text: impl Into<String>, style: Style) -> Part {
    Part { text: text.into(), style, action: None }
}

/// One rendered line of a panel: segments at columns inside the box, right-aligned parts, the
/// item it belongs to (None for headings) and a bar in the left padding (the session in the pane).
#[derive(Clone, Default)]
pub struct Line {
    pub segments: Vec<(u16, String, Style)>,
    pub right: Vec<Part>,
    pub item: Option<usize>,
    pub accent: Option<Style>,
}

impl Line {
    fn new(segments: Vec<(u16, String, Style)>) -> Self {
        Self { segments, ..Self::default() }
    }

    #[cfg(test)]
    pub fn texts(&self) -> String {
        self.segments.iter().map(|(_, text, _)| text.as_str()).collect()
    }
}

fn segment(column: u16, text: impl Into<String>, style: Style) -> (u16, String, Style) {
    (column, text.into(), style)
}

#[derive(Default)]
pub struct PanelState {
    pub lines: Vec<Line>,
    pub items: usize,
    pub selected: usize,
    pub scroll: usize,
}

impl PanelState {
    fn clamp(&mut self) {
        self.selected = if self.items == 0 { 0 } else { self.selected.min(self.items - 1) };
    }

    fn move_by(&mut self, delta: i64) {
        self.selected = if self.items == 0 { 0 } else { (self.selected as i64 + delta).clamp(0, self.items as i64 - 1) as usize };
    }

    fn keep_selected_visible(&mut self, inner_height: usize) {
        let target = self.lines.iter().position(|line| line.item == Some(self.selected)).unwrap_or(0);
        if target < self.scroll {
            self.scroll = target.saturating_sub(1);
        } else if target >= self.scroll + inner_height {
            self.scroll = target + 1 - inner_height;
        }
    }
}

/// A key or a click, from crossterm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Enter,
    Tab,
    BackTab,
    Esc,
    Backspace,
    Up,
    Down,
    Left,
    Right,
    F(u8),
    Click { x: u16, y: u16, double: bool },
}

/// The effort picker over a session card. Choosing another level restarts the session on it.
pub struct EffortMenu {
    pub(crate) session: Session,
    anchor_row: u16,
    pub(crate) current: String,
    pub(crate) selected: usize,
    rows: HashMap<u16, usize>,
    area: (u16, u16, u16, u16), // top, left, bottom, right
}

const MENU_WIDTH: u16 = 30;

impl EffortMenu {
    fn new(session: Session, anchor_row: u16) -> Self {
        let current = session.facts.effort.clone();
        let selected = EFFORT_LEVELS.iter().position(|level| *level == current).unwrap_or(2);
        Self { session, anchor_row, current, selected, rows: HashMap::new(), area: (0, 0, 0, 0) }
    }

    fn draw(&mut self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        let box_height = EFFORT_LEVELS.len() as u16 + 3;
        let top = if self.anchor_row + 1 + box_height < height.saturating_sub(1) { self.anchor_row + 1 } else { self.anchor_row.saturating_sub(box_height).max(1) };
        let left = width.saturating_sub(3 + MENU_WIDTH).max(1);
        let right = left + MENU_WIDTH - 1;
        self.area = (top, left, top + box_height - 1, right);
        let frame = on(BLUE, BG_HIGHLIGHT).add_modifier(Modifier::BOLD);
        let ground = on(FG, BG_HIGHLIGHT);
        let inner = (MENU_WIDTH - 2) as usize;
        put(buffer, top, left, &format!("╭{}╮", "─".repeat(inner)), frame);
        let title: String = format!(" Effort · {} ", self.session.project).chars().take(inner - 2).collect();
        put(buffer, top, left + 2, &title, frame);
        self.rows.clear();
        for (index, level) in EFFORT_LEVELS.iter().enumerate() {
            let row = top + 1 + index as u16;
            let chosen = index == self.selected;
            put(buffer, row, left, &format!("│{}│", " ".repeat(inner)), frame);
            put(buffer, row, left + 1, &" ".repeat(inner), if chosen { on(BG_HIGHLIGHT, BLUE) } else { ground });
            let is_current = *level == self.current;
            let text_style = if chosen { on(BG_HIGHLIGHT, BLUE).add_modifier(Modifier::BOLD) }
                             else if is_current { on(GREEN, BG_HIGHLIGHT).add_modifier(Modifier::BOLD) } else { ground };
            put(buffer, row, left + 2, &format!("{} {level}", if is_current { "●" } else { " " }), text_style);
            if is_current {
                put(buffer, row, right - 9, "current", if chosen { on(BG_HIGHLIGHT, BLUE) } else { on(FG_DARK, BG_HIGHLIGHT) });
            }
            self.rows.insert(row, index);
        }
        let note_row = top + 1 + EFFORT_LEVELS.len() as u16;
        put(buffer, note_row, left, &format!("│{}│", " ".repeat(inner)), frame);
        put(buffer, note_row, left + 2, "⏎ restarts on it  esc", on(FG_DARK, BG_HIGHLIGHT).add_modifier(Modifier::DIM));
        put(buffer, note_row + 1, left, &format!("╰{}╯", "─".repeat(inner)), frame);
    }

    fn contains(&self, x: u16, y: u16) -> bool {
        let (top, left, bottom, right) = self.area;
        (top..=bottom).contains(&y) && (left..=right).contains(&x)
    }
}

/// What the terminal does for the app: draw it, read keys, and lend the terminal to fzf or a prompt.
pub trait Screen {
    fn draw(&mut self, app: &mut App);
    fn next_key(&mut self, timeout: Option<Duration>) -> Option<Key>;
    fn outside(&mut self, action: &mut dyn FnMut() -> Option<String>) -> Option<String>;
}

pub struct App {
    pub history: History,
    pub view: ViewState,
    pub(crate) usage: Refresher,
    pub(crate) alerts: BudgetAlerts,
    pub costs: Costs,
    pub reviews: Reviews,
    pub todos: TodoList,
    pub errors: ErrorReporter,
    pub claude_status: ClaudeStatus,
    pub panels: [PanelState; 4],
    pub focus: usize,
    pub query: String,
    pub filtering: bool,
    pub help_open: bool,
    pub message: String,
    pub(crate) labelled: (String, String),
    pub(crate) menu: Option<EffortMenu>,
    pub restore: Vec<Restorable>,
    pub focus_waiting: bool,
    pub held: Vec<String>,
    pub(crate) held_checked: f64,
    pub(crate) queue_checked: f64,
    /// Whether Theo games right now; the queue waits meanwhile (a test sets its own).
    pub gaming: fn() -> bool,
    pub(crate) restore_checked: f64,
    pub(crate) start_files: Vec<(&'static str, f64)>,
    pub width: u16,
    pub sessions: Vec<Session>,
    pub(crate) windows: Vec<Window>,
    pub(crate) review_rows: Vec<Review>,
    pub todo_entries: Vec<Entry>,
    pub(crate) todo_scope: String,
    pub(crate) hit_lines: HashMap<u16, (usize, Option<usize>)>,
    pub(crate) hit_actions: HashMap<u16, (u16, u16, &'static str)>,
}

impl App {
    pub fn new() -> Self {
        Self {
            history: History::default(), view: ViewState::load(), usage: Refresher::new(), alerts: BudgetAlerts::new(util::send_notice),
            costs: Costs::default(), reviews: Reviews::default(), todos: TodoList::load(), errors: ErrorReporter::new(),
            claude_status: ClaudeStatus::default(), panels: Default::default(), focus: 1, query: String::new(), filtering: false,
            help_open: false, message: String::new(), labelled: Default::default(), menu: None, restore: Vec::new(),
            focus_waiting: false, held: Vec::new(), held_checked: 0.0, queue_checked: 0.0, gaming: util::gaming, restore_checked: 0.0, start_files: Vec::new(),
            width: DEFAULT_COLUMN_WIDTH, sessions: Vec::new(), windows: Vec::new(), review_rows: Vec::new(), todo_entries: Vec::new(),
            todo_scope: String::new(), hit_lines: HashMap::new(), hit_actions: HashMap::new(),
        }
    }

    /// A panel taking the focus shows from its top: while small it scrolled to keep its selection
    /// in view, and that scroll would otherwise stay once it is big again.
    pub fn focus_panel(&mut self, index: usize) {
        if index != self.focus {
            self.panels[index].scroll = 0;
        }
        self.focus = index;
    }

    pub fn tick(&mut self) {
        self.todos.apply_updates();
        self.errors.tick(&sessions::reported());
        self.claude_status.tick();
        self.usage.tick(false);
        self.alerts.check(&usage::windows());
        self.costs.tick();
        self.reviews.tick();
        self.build();
        self.push_viewed_state();
        self.dispatch_queue();
    }

    /// Every half minute: a session idle at its prompt for QUEUE_IDLE_S, with nothing given to it
    /// still in progress, gets its project's first queued item. Not the session Theo has on screen
    /// in the cockpit: he may be about to type in it.
    pub fn dispatch_queue(&mut self) {
        let now = util::now();
        if now - self.queue_checked < QUEUE_CHECK_S || self.todos.queued.is_empty() {
            return;
        }
        self.queue_checked = now;
        // No new work while Theo plays: it would compete with the game (game-performance lowers
        // the sessions' priority, it does not stop them).
        if (self.gaming)() {
            return;
        }
        let viewed = current_session();
        let watched = !viewed.is_empty() && util::claude_workspace_active();
        let idle: Vec<Session> = self.sessions.iter()
            .filter(|session| session.state == "waiting" && !session.terminal && !session.cwd.is_empty() && now as i64 - session.since >= QUEUE_IDLE_S)
            .filter(|session| !(watched && session.session == viewed) && !self.todos.busy(&session.session))
            .cloned().collect();
        for session in idle {
            let Some(entry) = self.next_queued(&session) else { continue };
            let (key, text) = match &entry {
                Entry::Typed(item) => (item.id(), item.text()),
                Entry::Issue { issue, .. } => (issue.key(), todo::issue_prompt(issue)),
            };
            let error = manager(&["say", &session.session, &todo::with_todo_note(&text, &key)]);
            if !error.is_empty() {
                util::log_event("WARN", &format!("queue: cannot give {key} to {}: {error}", session.project));
                continue;
            }
            self.todos.set_status(&key, "doing", &session.session);
            util::log_event("INFO", &format!("queue: {key} given to {} ({}), idle for {} min", session.project,
                                             &session.session[..8.min(session.session.len())], (now as i64 - session.since) / 60));
            self.message = format!("queue: gave {} its next item", session.project);
        }
    }

    /// The project's first queued item still to do: typed items of its project, issues of its
    /// repository.
    fn next_queued(&self, session: &Session) -> Option<Entry> {
        // By folder, not by name: a session can call its project anything (its .git file).
        let root = util::repository_root(&session.cwd);
        let mut candidates: Vec<Entry> = self.todos.items.iter()
            .filter(|item| util::repository_root(&item.cwd()) == root && item.status() == "todo" && self.todos.is_queued(&item.id()))
            .map(|item| Entry::Typed(item.clone())).collect();
        for issue in self.reviews.issues() {
            let key = issue.key();
            let checkout = self.reviews.checkouts.get(&issue.full_repository);
            if self.todos.is_queued(&key) && self.todos.status_of(&key) == "todo"
                && checkout.is_some_and(|checkout| util::repository_root(checkout) == root) {
                let priority = todo::issue_priority(&issue.labels);
                candidates.push(Entry::Issue { issue, status: "todo".into(), priority });
            }
        }
        candidates.sort_by(todo::queue_order);
        candidates.into_iter().next()
    }

    pub fn build(&mut self) {
        self.build_status();
        self.build_sessions();
        self.build_reviews();
        self.build_todo();
    }

    /// The viewed session's state goes to the manager (its title line's colour) when it changes.
    fn push_viewed_state(&mut self) {
        let viewed = current_session();
        let state = self.sessions.iter().find(|s| s.session == viewed).map(|s| s.state.clone()).unwrap_or_default();
        if (viewed.clone(), state.clone()) == self.labelled {
            return;
        }
        self.labelled = (viewed.clone(), state.clone());
        if !viewed.is_empty() && !state.is_empty() {
            manager_in_background(&["label-state", &state]);
        }
    }

    // ---- Status ----

    fn build_status(&mut self) {
        self.windows = usage::windows();
        let windows = &self.windows;
        // Every row's label shares the column, the cost and Claude rows too: sized on the usage rows
        // alone, it shrank to nothing before the first usage fetch and the values covered the labels.
        let label_width = windows.iter().map(|w| width_of(&w.label)).chain([width_of(COSTS_LABEL), width_of(CLAUDE_LABEL)]).max().unwrap_or(0) as u16;
        let resets: Vec<String> = windows.iter().map(usage::resets_short).collect();
        let resets_width = resets.iter().map(|r| width_of(r)).max().unwrap_or(0);
        let budget = self.width as i64 - 4 - label_width as i64 - 2 - 5 - if resets_width > 0 { resets_width as i64 + 2 } else { 0 };
        let bar_width = budget.clamp(usage::USAGE_BAR_MIN as i64, usage::USAGE_BAR_MAX as i64) as usize;
        let mut lines = Vec::new();
        for (index, (window, reset)) in windows.iter().zip(resets).enumerate() {
            let tint = bold(usage_color(window.percentage));
            let value = window.percentage.map(|p| format!("{p:3}%")).unwrap_or_else(|| "   –".into());
            lines.push(Line {
                segments: vec![segment(0, &window.label, style(FG)), segment(label_width + 2, usage::bar(window.percentage, bar_width), tint),
                               segment(label_width + 3 + bar_width as u16, value, tint)],
                right: vec![part(reset, dim(FG_DARK))], item: Some(index), accent: None,
            });
        }
        if windows.is_empty() {
            lines.push(Line::new(vec![segment(0, "No usage yet (claude-usage.sh has not fetched).", dim(FG_DARK))]));
        }
        let totals = self.costs.totals();
        if totals.today > 0.0 || totals.week > 0.0 || totals.month > 0.0 {
            let mut segments = vec![segment(0, COSTS_LABEL, style(FG))];
            let mut column = label_width + 2;
            for (label, amount) in [("today", totals.today), ("week", totals.week), ("month", totals.month)] {
                let amount = money(amount);
                segments.push(segment(column, label, dim(FG_DARK)));
                segments.push(segment(column + label.len() as u16 + 1, &amount, bold(MAGENTA)));
                column += (label.len() + width_of(&amount) + 3) as u16;
            }
            lines.push(Line::new(segments));
        }
        let error = self.costs.error();
        if !error.is_empty() {
            lines.push(Line::new(vec![segment(0, error, style(YELLOW))]));
        }
        lines.extend(claude_status_lines(&self.claude_status, label_width));
        let panel = &mut self.panels[0];
        panel.items = self.windows.len();
        panel.clamp();
        panel.lines = lines;
    }

    fn status_subtitle(&self) -> String {
        let plan = usage::plan();
        let all_time = self.costs.all_time();
        if all_time > 0.0 { format!("{plan} · all time {}", money(all_time)).trim_start_matches([' ', '·']).to_string() } else { plan }
    }

    pub(crate) fn handle_status(&mut self, key: Key) -> String {
        match key {
            Key::Enter => manager(&["usage"]),
            Key::Char('r') => {
                self.usage.tick(true);
                "fetching usage …".into()
            }
            Key::Char('o') => {
                let _ = Command::new("xdg-open").arg(status::PAGE).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                "opening status.claude.com …".into()
            }
            _ => String::new(),
        }
    }

    // ---- Sessions ----

    fn build_sessions(&mut self) {
        let mut running: Vec<Session> = sessions::running().into_iter()
            .filter(|s| sessions::matches(&[&s.project, &s.task], &self.query))
            .filter(|s| !self.focus_waiting || FOCUS_STATES.contains(&s.state.as_str()))
            .collect();
        let known: Vec<String> = running.iter().filter(|s| s.state != "unknown").map(|s| s.session.clone()).collect();
        self.history.load_titles(&known, TITLES_PER_TICK);
        for session in &mut running {
            if session.state != "unknown" {
                session.facts = sessions::statusline_facts(&session.session);
                session.title = self.history.title(&session.session);
            }
            session.pinned = self.view.pinned.contains(&session.session);
            session.design = sessions::design_of(&session.session);
        }
        // A card keeps its place: pinned first, then in the order the sessions started.
        let running = sessions::grouped_by_project(running);
        let current = current_session();
        let mut lines = Vec::new();
        if now() - self.restore_checked > RESTORE_CHECK_S {
            self.restore_checked = now();
            self.restore = sessions::restore_offer(&self.view, &running.iter().map(|s| s.session.clone()).collect());
        }
        if now() - self.held_checked > GREYPROXY_POLL_S {
            self.held_checked = now();
            self.held = sessions::greyproxy_held(&paths::greyproxy_db());
        }
        let (claude, _) = self.claude_status.snapshot();
        if let Some(claude) = claude.filter(|c| !c.indicator.is_empty() && c.indicator != "none") {
            let tint = bold(claude_status_color(&claude.indicator));
            lines.push(Line::new(vec![segment(0, format!("⚠ Claude: {}", claude.description), tint)]));
            let mut details: Vec<String> = claude.degraded.iter().take(2).map(|(name, state)| format!("{name}: {state}")).collect();
            details.extend(claude.incidents.iter().take(1).cloned());
            if !details.is_empty() {
                lines.push(Line::new(vec![segment(2, details.join(" · "), dim(FG_DARK))]));
            }
            lines.push(Line::new(vec![segment(2, "0 then o opens status.claude.com", dim(FG_DARK))]));
            lines.push(Line::default());
        }
        // game-performance froze the sessions for a game: their cards stand still until it ends.
        let paused = std::fs::read_to_string(paths::game_frozen()).map(|list| list.lines().filter(|line| !line.trim().is_empty()).count()).unwrap_or(0);
        if paused > 0 {
            lines.push(Line::new(vec![segment(0, format!("⏸ {paused} session{} paused while you game", if paused > 1 { "s" } else { "" }), bold(BLUE))]));
            lines.push(Line::new(vec![segment(2, "they resume where they stopped when the game ends", dim(FG_DARK))]));
            lines.push(Line::default());
        }
        self.start_files = sessions::start_file_mtimes();
        let outdated: Vec<&Session> = running.iter().filter(|s| !sessions::changed_since_start(s, &self.start_files).is_empty()).collect();
        if !outdated.is_empty() {
            let mut changes: Vec<&str> = outdated.iter().flat_map(|s| sessions::changed_since_start(s, &self.start_files)).collect();
            changes.sort();
            changes.dedup();
            let count = format!("{} session{}", outdated.len(), if outdated.len() > 1 { "s" } else { "" });
            lines.push(Line::new(vec![segment(0, format!("⟳ {count} started before a change to {}", changes.join(", ")), bold(YELLOW))]));
            lines.push(Line::new(vec![segment(2, "^R restarts the ones waiting at their prompt", dim(FG_DARK))]));
            lines.push(Line::default());
        }
        if !self.held.is_empty() {
            let mut hosts = self.held.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            if self.held.len() > 3 {
                hosts += &format!(" and {} more", self.held.len() - 3);
            }
            lines.push(Line::new(vec![segment(0, format!("⛔ greyproxy holds {hosts}"), bold(YELLOW))]));
            lines.push(Line::new(vec![segment(2, "G opens its dashboard to approve or deny", dim(FG_DARK))]));
            lines.push(Line::default());
        }
        if !self.restore.is_empty() {
            let names = self.restore.iter().map(|r| if r.project.is_empty() { "?" } else { r.project.as_str() }).collect::<Vec<_>>().join(", ");
            lines.push(Line::new(vec![segment(0, format!("↺ {} running before the restart: {names}", self.restore.len()), bold(CYAN))]));
            lines.push(Line::new(vec![segment(2, "B brings them back, one after the other", dim(FG_DARK))]));
            lines.push(Line::default());
        }
        for (index, session) in running.iter().enumerate() {
            let tint = bold(state_color(&session.state));
            let accent = (session.session == current).then_some(tint);
            let branch = util::git_branch(&session.cwd);
            let grouped = session.group_size > 1;
            // Sessions of one repository: its name once as a heading, then each card on a rail,
            // named by its branch, with no gap between them.
            let indent: u16 = if grouped { 2 } else { 0 };
            let rail = if grouped { vec![segment(0, "│", dim(project_color(&session.project)))] } else { vec![] };
            if grouped && session.group_position == 0 {
                lines.push(Line::new(vec![segment(0, &session.project, bold(project_color(&session.project))),
                                          segment(width_of(&session.project) as u16 + 1, format!("· {} sessions", session.group_size), dim(FG_DARK))]));
            }
            let segments = if grouped {
                let name = if branch.is_empty() { session.project.clone() } else { branch.clone() };
                [rail.clone(), vec![segment(2, sessions::glyph(&session.state), tint), segment(4, name, bold(FG))]].concat()
            } else {
                let mut segments = vec![segment(0, sessions::glyph(&session.state), tint), segment(2, &session.project, bold(project_color(&session.project)))];
                if !branch.is_empty() {
                    segments.push(segment(4 + width_of(&session.project) as u16, format!("⎇ {branch}"), dim(FG_DARK)));
                }
                segments
            };
            let facts = &session.facts;
            let mut badges = Vec::new();
            if facts.context.is_some_and(|context| context >= CONTEXT_COMPACT_AT) {
                badges.push(part("/compact", bold(YELLOW)));
            }
            if session.update {
                badges.push(part("⬆ update", bold(GREEN)));
            }
            if !sessions::changed_since_start(session, &self.start_files).is_empty() {
                badges.push(part("⟳ restart", bold(YELLOW)));
            }
            if facts.remote {
                badges.push(part("⇅ remote", bold(CYAN)));
            }
            if session.terminal {
                badges.push(part("▭ terminal", bold(MAGENTA)));
            }
            if session.pinned {
                badges.push(part("⚲", style(YELLOW)));
            }
            badges.push(part(age(session.since), tint));
            lines.push(Line { segments, right: badges, item: Some(index), accent });
            let title = if !session.title.is_empty() { session.title.clone() } else { sessions::visible_task(session) };
            let mut right = Vec::new();
            if session.state != "unknown" {
                right.push(part(&facts.model, dim(FG_DARK)));
                let effort = if facts.effort.is_empty() { "effort" } else { facts.effort.as_str() };
                right.push(Part { text: format!("{effort} ▾"), style: style(CYAN), action: Some("effort") });
                right.push(part(facts.context.map(|c| format!("{}%", c.round())).unwrap_or_default(), bold(context_color(facts.context))));
            }
            right.retain(|p| !p.text.is_empty());
            lines.push(Line { segments: [rail.clone(), vec![segment(2 + indent, title, style(FG))]].concat(), right, item: Some(index), accent });
            if let Some(design) = &session.design {
                lines.push(Line { segments: [rail.clone(), vec![segment(2 + indent, format!("◆ {}", design.title), style(MAGENTA))]].concat(),
                                  right: vec![part("o canvas", dim(FG_DARK))], item: Some(index), accent });
            }
            let task = sessions::visible_task(session);
            let asks = sessions::waiting_request(session);
            let extra = if !asks.is_empty() {
                Some((format!("⚠ {asks}"), style(RED)))
            } else if session.state == "waiting" && !session.outcome.is_empty() {
                Some((format!("✓ {}", session.outcome), dim(GREEN)))
            } else if session.state == "working" && !session.title.is_empty() && !task.is_empty() && task != session.title {
                Some((task, dim(FG_DARK)))
            } else {
                None
            };
            if let Some((text, tint)) = extra {
                lines.push(Line { segments: [rail.clone(), vec![segment(2 + indent, text, tint)]].concat(), right: vec![], item: Some(index), accent });
            }
            if !grouped || session.group_position == session.group_size - 1 {
                lines.push(Line::default());
            }
        }
        if running.is_empty() {
            let empty = if self.focus_waiting { "Nothing waits on you." } else if !self.query.is_empty() { "Nothing matches." } else { "No running session." };
            lines.push(Line::new(vec![segment(0, empty, dim(FG_DARK))]));
            lines.push(Line::new(vec![segment(0, "a  start one in a project", dim(FG_DARK))]));
        }
        self.sessions = running;
        let panel = &mut self.panels[1];
        panel.items = self.sessions.len();
        panel.clamp();
        panel.lines = lines;
    }

    fn selected_session(&self) -> Option<Session> {
        self.sessions.get(self.panels[1].selected).cloned()
    }

    pub(crate) fn sessions_hints(&self) -> Vec<(&'static str, &'static str)> {
        let selected = self.selected_session().unwrap_or_default();
        if selected.state == "needs-you" && !sessions::waiting_request(&selected).is_empty() {
            return vec![("⏎", "open"), ("Y", "approve"), ("N", "decline"), ("n", "next"), ("g", "git"), ("z", "zoom")];
        }
        if selected.facts.context.is_some_and(|c| c >= CONTEXT_COMPACT_AT) {
            return vec![("⏎", "open"), ("C", "/compact"), ("e", "effort"), ("g", "git"), ("z", "zoom"), ("x", "exit")];
        }
        if selected.update {
            return vec![("⏎", "open"), ("r", "restart to apply the update"), ("e", "effort"), ("g", "git"), ("x", "exit")];
        }
        if selected.design.is_some() {
            return vec![("⏎", "open"), ("o", "canvas"), ("n", "peek"), ("e", "effort"), ("g", "git"), ("x", "exit")];
        }
        vec![("⏎", "open"), ("n", "peek"), ("+", "another"), ("e", "effort"), ("g", "git"), ("z", "zoom"), ("x", "exit")]
    }

    fn sessions_help() -> Vec<(&'static str, &'static str)> {
        vec![("⏎", "show the session and move the cursor into it"), ("n", "peek at the next session needing you"),
             ("x", "exit the session: Ctrl-C twice, then killed if it lingers"), ("r", "restart in place: same conversation, new process"),
             ("^R", "restart every session that waits for you; working ones and pending permissions are left alone"),
             ("R", "toggle Remote Control (phone / claude.ai/code)"), ("p", "pin on top"),
             ("e", "effort level: pick one, the session restarts on it (or click the level on the card)"),
             ("g", "lazygit on the session's repository, in a popup (q closes it)"),
             ("U", "push the session's branch to origin with your SSH key, after a yes (lazygit's sandbox cannot)"),
             ("+", "another session on this repository, on its own branch in a separate clone"),
             ("C", "/compact in the session, when it waits (the /compact badge suggests it)"),
             ("B", "after a reboot: bring back the sessions that were running"),
             ("z", "the session over the whole window; Ctrl+Tab comes back"),
             ("f", "focus: only the sessions waiting on you; f again shows all"),
             ("Y N", "approve or decline the permission a card asks for, after a y/n confirmation"),
             ("m", "a message for the session without opening it: sent at its prompt, queued while it works"),
             ("o", "open the Claude Design canvas a design session works on (◆ on its card)")]
    }

    pub(crate) fn handle_sessions(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let session = self.selected_session();
        let known = session.as_ref().filter(|s| s.state != "unknown").cloned();
        match key {
            Key::Enter => session.map(|s| manager(&["open", &s.session])).unwrap_or_default(),
            Key::Char('n') => match sessions::next_attention_index(&self.sessions, self.panels[1].selected) {
                None => "nothing needs you".into(),
                Some(index) => {
                    self.panels[1].selected = index;
                    manager(&["view", &self.sessions[index].session])
                }
            },
            Key::Char('p') => {
                if let Some(s) = known {
                    self.view.toggle_pin(&s.session);
                }
                String::new()
            }
            Key::Char('R') => match known {
                Some(s) => {
                    manager_in_background(&["remote", &s.session]);
                    format!("toggling remote control for {} …", s.project)
                }
                None => String::new(),
            },
            Key::Char('r') => match known {
                Some(s) if self.confirm(screen, &format!("restart {}? same conversation, new process", s.project)) => {
                    manager_in_background(&["restart", &s.session]);
                    format!("restarting {} …", s.project)
                }
                _ => String::new(),
            },
            Key::Ctrl('r') => self.restart_all(screen),
            Key::Char('e') => match session {
                Some(s) => self.open_effort_menu(s, None),
                None => String::new(),
            },
            Key::Char('B') if !self.restore.is_empty() => {
                let pairs: Vec<String> = self.restore.iter().flat_map(|r| [r.cwd.clone(), r.session.clone()]).collect();
                let mut arguments = vec!["restore"];
                arguments.extend(pairs.iter().map(String::as_str));
                manager_in_background(&arguments);
                self.view.restored_boot = sessions::boot_time();
                self.view.save();
                let count = self.restore.len();
                self.restore.clear();
                format!("bringing back {count} session{} …", if count != 1 { "s" } else { "" })
            }
            Key::Char('+') => match session {
                Some(s) if s.cwd.is_empty() => "no directory known for this session".into(),
                Some(s) => {
                    let suggested = chrono::Local::now().format("parallel-%H%M").to_string();
                    let typed = input_line(screen, &format!("branch for another {} session (Enter: {suggested})> ", s.project));
                    let branch = if typed.is_empty() { suggested } else { typed };
                    manager_in_background(&["parallel", &s.cwd, &branch]);
                    format!("cloning {} for {branch} …", s.project)
                }
                None => String::new(),
            },
            Key::Char('C') => match known {
                Some(s) => {
                    let error = manager(&["compact", &s.session]);
                    if error.is_empty() { format!("/compact sent to {}", s.project) } else { error }
                }
                None => String::new(),
            },
            Key::Char('z') => manager(&["zoom"]),
            Key::Char('o') => match session.and_then(|s| s.design) {
                Some(design) => {
                    let _ = Command::new("xdg-open").arg(&design.url).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                    format!("opening {} …", design.title)
                }
                None => "no canvas: V starts a design session".into(),
            },
            Key::Char(answer @ ('Y' | 'N')) => match session.filter(|s| s.state == "needs-you") {
                Some(s) => {
                    let approve = answer == 'Y';
                    let asks = sessions::waiting_request(&s);
                    let asks: String = if asks.is_empty() { "its request".into() } else { asks.chars().take(60).collect() };
                    if !self.confirm(screen, &format!("{} {}: {asks}?", if approve { "approve" } else { "decline" }, s.project)) {
                        return String::new();
                    }
                    let error = manager(&["answer", &s.session, if approve { "yes" } else { "no" }]);
                    if error.is_empty() { format!("{} for {}", if approve { "approved" } else { "declined" }, s.project) } else { error }
                }
                None => String::new(),
            },
            Key::Char('m') => match known {
                Some(s) => {
                    let message = input_line(screen, &format!("message for {} (Enter sends, empty cancels)> ", s.project));
                    if message.is_empty() {
                        return String::new();
                    }
                    let error = manager(&["say", &s.session, &message]);
                    if !error.is_empty() { error } else if s.state != "working" { format!("sent to {}", s.project) } else { format!("queued for {}", s.project) }
                }
                None => String::new(),
            },
            Key::Char('f') => {
                self.focus_waiting = !self.focus_waiting;
                if self.focus_waiting { "only the sessions waiting on you (f shows all)".into() } else { String::new() }
            }
            Key::Char('g') => match session {
                Some(s) if s.cwd.is_empty() => "no directory known for this session".into(),
                Some(s) => {
                    manager_in_background(&["git", &s.cwd]);
                    String::new()
                }
                None => String::new(),
            },
            Key::Char('U') => match session {
                Some(s) if s.cwd.is_empty() => "no directory known for this session".into(),
                Some(s) => {
                    manager_in_background(&["push", &s.cwd]);
                    String::new()
                }
                None => String::new(),
            },
            Key::Char('x') => match session {
                Some(s) if self.confirm(screen, &format!("exit {}?", s.project)) => kill_session(&s),
                _ => String::new(),
            },
            _ => String::new(),
        }
    }

    /// Restart every session sitting at its prompt: one that works or asks for a permission would
    /// lose its turn, so it is skipped and named.
    pub(crate) fn restart_all(&mut self, screen: &mut dyn Screen) -> String {
        // A terminal session is its terminal's: restart-all leaves it alone.
        let ready: Vec<Session> = self.sessions.iter().filter(|s| s.state == "waiting" && !s.terminal).cloned().collect();
        let busy: Vec<Session> = self.sessions.iter().filter(|s| (s.state == "working" || s.state == "needs-you") && !s.terminal).cloned().collect();
        if ready.is_empty() {
            return if busy.is_empty() { "no session to restart".into() } else { "nothing to restart: every session is busy".into() };
        }
        let skipped = if busy.is_empty() { String::new() } else { format!(", leaving {} busy", busy.len()) };
        let question = format!("restart {} session{}{skipped}? same conversations, new processes", ready.len(), if ready.len() > 1 { "s" } else { "" });
        if !self.confirm(screen, &question) {
            return String::new();
        }
        let mut arguments = vec!["restart-all"];
        arguments.extend(ready.iter().map(|s| s.session.as_str()));
        manager_in_background(&arguments);
        let busy_names = busy.iter().map(|s| s.project.as_str()).collect::<Vec<_>>().join(", ");
        format!("restarting {} …", ready.len()) + &if busy.is_empty() { String::new() } else { format!(" busy, not restarted: {busy_names}") }
    }

    pub(crate) fn open_effort_menu(&mut self, session: Session, anchor_row: Option<u16>) -> String {
        if session.state == "unknown" {
            return "no effort for a session the daemon does not know".into();
        }
        // Under the card's last line, so the card and its effort stay readable above the menu.
        let selected = self.panels[1].selected;
        let anchor = anchor_row.unwrap_or_else(|| {
            self.hit_lines.iter().filter(|(_, (panel, item))| *panel == 1 && *item == Some(selected)).map(|(row, _)| *row).max().unwrap_or(HEADER_LINES)
        });
        self.menu = Some(EffortMenu::new(session, anchor));
        String::new()
    }

    fn choose_effort(&mut self, screen: &mut dyn Screen) -> String {
        let Some(menu) = self.menu.take() else { return String::new() };
        let level = EFFORT_LEVELS[menu.selected];
        let session = menu.session;
        if level == menu.current {
            return format!("{} already runs on {level}", session.project);
        }
        if session.state == "working" && !self.confirm(screen, &format!("{} is working: restart it on {level}?", session.project)) {
            return String::new();
        }
        manager_in_background(&["restart", &session.session, level]);
        format!("restarting {} on {level} effort …", session.project)
    }

    /// Keys and clicks while the effort menu is open; anything else closes it.
    fn handle_menu(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let Some(menu) = self.menu.as_mut() else { return String::new() };
        match key {
            Key::Click { x, y, .. } => {
                if let Some(index) = menu.rows.get(&y).copied() {
                    menu.selected = index;
                    return self.choose_effort(screen);
                }
                if !menu.contains(x, y) {
                    self.menu = None;
                }
            }
            Key::Char('j') | Key::Down => menu.selected = (menu.selected + 1).min(EFFORT_LEVELS.len() - 1),
            Key::Char('k') | Key::Up => menu.selected = menu.selected.saturating_sub(1),
            Key::Char(digit @ '1'..='5') => {
                menu.selected = digit as usize - '1' as usize;
                return self.choose_effort(screen);
            }
            Key::Enter => return self.choose_effort(screen),
            _ => self.menu = None,
        }
        String::new()
    }

    // ---- Pull requests ----

    fn build_reviews(&mut self) {
        let (rows, error, loading) = self.reviews.current();
        self.review_rows = rows.into_iter().filter(|row| sessions::matches(&[&row.repository, &row.title], &self.query)).collect();
        let mut lines = Vec::new();
        if !error.is_empty() {
            lines.push(Line::new(vec![segment(0, error, style(YELLOW))]));
        } else if self.review_rows.is_empty() && !loading {
            lines.push(Line::new(vec![segment(0, "No open pull request here.", dim(FG_DARK))]));
        }
        for (index, row) in self.review_rows.iter().enumerate() {
            let label = format!("{}#{}", row.repository, row.number);
            let mut right = Vec::new();
            if row.draft {
                right.push(part("draft", dim(FG_DARK)));
            }
            match row.review.as_str() {
                "APPROVED" => right.push(part("approved", style(GREEN))),
                "CHANGES_REQUESTED" => right.push(part("changes", style(RED))),
                "REVIEW_REQUIRED" => right.push(part("review", style(YELLOW))),
                _ => {}
            }
            match row.ci.as_str() {
                "failing" => right.push(part("✗", bold(RED))),
                "pending" => right.push(part("●", bold(YELLOW))),
                "passing" => right.push(part("✓", bold(GREEN))),
                _ => {}
            }
            right.push(part(reviews::review_age(&row.updated), dim(FG_DARK)));
            right.retain(|p| !p.text.is_empty());
            lines.push(Line { segments: vec![segment(0, &label, bold(MAGENTA)), segment(1 + width_of(&label) as u16, &row.title, style(FG_DARK))],
                              right, item: Some(index), accent: None });
        }
        let panel = &mut self.panels[2];
        panel.items = self.review_rows.len();
        panel.clamp();
        panel.lines = lines;
    }

    fn reviews_subtitle(&self) -> String {
        let scope = self.reviews.repositories.iter().map(|r| r.rsplit('/').next().unwrap_or("")).collect::<Vec<_>>().join(", ");
        let scope = if scope.is_empty() { "awaiting my review".to_string() } else { scope };
        scope + if self.reviews.current().2 { "  ⟳" } else { "" }
    }

    pub(crate) fn handle_reviews(&mut self, key: Key) -> String {
        let row = self.review_rows.get(self.panels[2].selected).cloned();
        match key {
            Key::Enter => row.map(|row| open_in_scratchpad("prs", &row.url)).unwrap_or_default(),
            Key::Char('c') => match row {
                Some(row) => match self.reviews.checkouts.get(&row.full_repository) {
                    None => format!("no local checkout of {}: start a session there first", row.repository),
                    Some(checkout) => {
                        manager_in_background(&["review", checkout, &row.number.to_string()]);
                        format!("preparing a review of {}#{} …", row.repository, row.number)
                    }
                },
                None => String::new(),
            },
            Key::Char('r') => {
                self.reviews.force_refresh();
                "refreshing pull requests …".into()
            }
            _ => String::new(),
        }
    }

    // ---- Todo ----

    fn build_todo(&mut self) {
        // Like the issues: the viewed session's project only, every project when none is viewed.
        let viewed = current_session();
        self.todo_scope = self.sessions.iter().find(|s| s.session == viewed).map(|s| s.project.clone()).unwrap_or_default();
        let scope = self.todo_scope.clone();
        let mut entries: Vec<Entry> = self.todos.items.iter()
            .filter(|item| (scope.is_empty() || item.project() == scope) && sessions::matches(&[&item.project(), &item.text()], &self.query))
            .map(|item| Entry::Typed(item.clone()))
            .collect();
        for issue in self.reviews.issues() {
            if !sessions::matches(&[&issue.repository, &issue.title], &self.query) {
                continue;
            }
            let status = self.todos.status_of(&issue.key());
            let priority = todo::issue_priority(&issue.labels);
            entries.push(Entry::Issue { issue, status, priority });
        }
        entries.sort_by(todo::todo_order);
        let mut lines = Vec::new();
        if entries.is_empty() {
            lines.push(Line::new(vec![segment(0, "Nothing to do. t adds an item for the viewed project.", dim(FG_DARK))]));
        }
        for (index, entry) in entries.iter().enumerate() {
            let status = entry.status();
            let done = status == "done";
            let mark = if done { String::new() } else { todo::priority_mark(entry.priority()) };
            let mark_style = if entry.priority() > todo::PRIORITY_NORMAL { bold(RED) } else { dim(FG_DARK) };
            let start = 2 + if mark.is_empty() { 0 } else { width_of(&mark) as u16 + 1 };
            let (glyph, glyph_style) = match status.as_str() {
                "doing" => ("◐", bold(BLUE)),
                "done" => ("✓", dim(GREEN)),
                _ => ("•", bold(YELLOW)),
            };
            let faded = done.then(|| dim(FG_DARK));
            let queued = status == "todo" && self.todos.is_queued(&entry.key());
            let filed_by_claude = matches!(entry, Entry::Typed(item) if !item.source().is_empty());
            let right = if status == "doing" { vec![part("in progress", style(BLUE))] }
                        else if queued { vec![part("queued", bold(CYAN))] }
                        else if filed_by_claude { vec![part("by Claude", style(YELLOW))] } else { vec![] };
            let line = match entry {
                Entry::Typed(item) => Line {
                    segments: vec![segment(0, glyph, glyph_style), segment(2, &mark, mark_style),
                                   segment(start, item.project(), faded.unwrap_or(bold(project_color(&item.project())))),
                                   segment(start + 1 + width_of(&item.project()) as u16, item.text(), faded.unwrap_or(style(FG)))],
                    right, item: Some(index), accent: None,
                },
                Entry::Issue { issue, .. } => {
                    let label = format!("{}#{}", issue.repository, issue.number);
                    let labels = issue.labels.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                    let (glyph, glyph_style) = if status == "todo" { ("◇", style(MAGENTA)) } else { (glyph, glyph_style) };
                    let right = if !right.is_empty() { right } else if !labels.is_empty() { vec![part(labels, dim(FG_DARK))] } else { vec![] };
                    let right = if queued && status == "todo" { vec![part("queued", bold(CYAN))] } else { right };
                    Line {
                        segments: vec![segment(0, glyph, glyph_style), segment(2, &mark, mark_style),
                                       segment(start, &label, faded.unwrap_or(bold(MAGENTA))),
                                       segment(start + 1 + width_of(&label) as u16, &issue.title, faded.unwrap_or(style(FG_DARK)))],
                        right, item: Some(index), accent: None,
                    }
                }
            };
            lines.push(line);
        }
        self.todo_entries = entries;
        let panel = &mut self.panels[3];
        panel.items = self.todo_entries.len();
        panel.clamp();
        panel.lines = lines;
    }

    fn todo_subtitle(&self) -> String {
        let doing = self.todo_entries.iter().filter(|e| e.status() == "doing").count();
        let waiting = self.todo_entries.iter().filter(|e| e.status() == "todo").count();
        let queued = self.todo_entries.iter().filter(|e| e.status() == "todo" && self.todos.is_queued(&e.key())).count();
        let scope = if self.todo_scope.is_empty() { "all projects" } else { self.todo_scope.as_str() };
        let queue = if queued > 0 { format!(" · {queued} queued") } else { String::new() };
        format!("{scope} · {doing} in progress · {waiting} to do{queue}")
    }

    pub(crate) fn handle_todo(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let entry = self.todo_entries.get(self.panels[3].selected).cloned();
        let viewed = current_session();
        match key {
            Key::Char('t') => {
                let viewed = self.sessions.iter().find(|s| s.session == viewed && !s.cwd.is_empty()).cloned();
                let directory = match &viewed {
                    Some(session) => session.cwd.clone(),
                    None => screen.outside(&mut || pick_project()).unwrap_or_default(),
                };
                if directory.is_empty() {
                    return String::new();
                }
                let text = input_line(screen, &format!("to do in {} (Enter adds, empty cancels)> ", project_name(&directory)));
                if text.is_empty() {
                    return String::new();
                }
                let project = viewed.map(|s| s.project).unwrap_or_else(|| project_name(&directory));
                self.todos.add(&project, &directory, &text);
                format!("added to {}", project_name(&directory))
            }
            Key::Char('x') => match entry {
                Some(Entry::Typed(item)) => {
                    self.todos.remove(&item.id());
                    "removed".into()
                }
                _ => String::new(),
            },
            Key::Char(pressed @ ('i' | 'd')) => match entry {
                Some(entry) => {
                    let target = if pressed == 'i' { "doing" } else { "done" };
                    let status = if self.todos.status_of(&entry.key()) == target { "todo" } else { target };
                    self.todos.set_status(&entry.key(), status, "");
                    match status { "doing" => "in progress", "done" => "done", _ => "back to do" }.into()
                }
                None => String::new(),
            },
            Key::Char(pressed @ ('+' | '-')) => match entry {
                Some(Entry::Typed(item)) => {
                    self.todos.change_priority(&item.id(), if pressed == '+' { 1 } else { -1 });
                    self.build_todo();
                    if let Some(index) = self.todo_entries.iter().position(|e| e.key() == item.id()) {
                        self.panels[3].selected = index;
                    }
                    String::new()
                }
                Some(Entry::Issue { .. }) => "an issue's priority comes from its labels on GitHub".into(),
                None => String::new(),
            },
            Key::Enter => match entry {
                Some(entry) => self.hand_to_claude(&entry),
                None => String::new(),
            },
            Key::Char('Q') => match entry {
                Some(entry) if entry.status() == "todo" => {
                    if self.todos.toggle_queued(&entry.key()) { "queued: an idle session of its project takes it up".into() } else { "out of the queue".into() }
                }
                Some(_) => "only items still to do can be queued".into(),
                None => String::new(),
            },
            _ => String::new(),
        }
    }

    /// The item to the project's running session, or to a new one; then it is in progress.
    pub fn hand_to_claude(&mut self, entry: &Entry) -> String {
        let (directory, text, repository) = match entry {
            Entry::Typed(item) => (item.cwd(), item.text(), String::new()),
            Entry::Issue { issue, .. } => (self.reviews.checkouts.get(&issue.full_repository).cloned().unwrap_or_default(), todo::issue_prompt(issue), issue.repository.clone()),
        };
        if directory.is_empty() {
            return format!("no local checkout of {}: start a session there first", if repository.is_empty() { "?" } else { &repository });
        }
        let key = entry.key();
        let text = todo::with_todo_note(&text, &key);
        let running = self.sessions.iter().find(|s| s.cwd == directory && s.state != "unknown").cloned();
        let outcome = match &running {
            Some(session) => {
                let error = manager(&["say", &session.session, &text]);
                if !error.is_empty() {
                    return error;
                }
                format!("sent to {}", session.project)
            }
            None => {
                manager_in_background(&["new", &directory, &text]);
                format!("starting a session in {}", project_name(&directory))
            }
        };
        self.todos.set_status(&key, "doing", running.as_ref().map(|s| s.session.as_str()).unwrap_or(""));
        outcome
    }

    // ---- The whole column ----

    fn panel_hints(&self, index: usize) -> Vec<(&'static str, &'static str)> {
        match index {
            0 => vec![("⏎", "details"), ("r", "refetch"), ("o", "status page")],
            1 => self.sessions_hints(),
            2 => vec![("⏎", "open"), ("c", "Claude review"), ("P", "list"), ("r", "refresh")],
            _ => vec![("⏎", "to Claude"), ("Q", "queue"), ("t", "add"), ("i", "in progress"), ("d", "done"), ("+", "priority"), ("x", "remove")],
        }
    }

    fn panel_help(index: usize) -> Vec<(&'static str, &'static str)> {
        match index {
            0 => vec![("⏎", "the full usage report in the session pane"), ("r", "fetch the usage again now"),
                      ("o", "open status.claude.com: Anthropic's incidents and each product's state")],
            1 => Self::sessions_help(),
            2 => vec![("⏎", "open the pull request in the pull requests window"),
                      ("c", "a Claude session reviewing it, in its own clone of the repository"),
                      ("P", "the pull requests window on the viewed session's repository (or your review requests)"), ("r", "refresh now"),
                      ("✓ ● ✗", "CI passing, running, failing"), ("approved", "review state: approved, changes (requested), review (required)")],
            _ => vec![("⏎", "hand the item to Claude: the project's running session, or a new one"),
                      ("Q", "queue it (again: unqueue): a session of its project idle for 5 min at its prompt takes the first queued item, by priority, one at a time"),
                      ("t", "add an item for the viewed session's project (or pick a project)"),
                      ("i", "mark in progress (Enter does it too); i again: back to to do"),
                      ("d", "mark done, or back to to do; done items stay 7 days, dimmed at the bottom"),
                      ("+", "raise a typed item's priority: ! then !! up to !!!!"), ("-", "lower it"), ("x", "remove a typed item"),
                      ("! ↓", "raised and low priority; an issue's comes from its labels (P0/P1/urgent/high, P3/P4/low)"),
                      ("• ◐ ✓", "to do · in progress · done; ◇ an open GitHub issue still to do"),
                      ("", "Claude marks items itself with claude-todo: start, done, add")],
        }
    }

    pub(crate) fn subtitle(&self, index: usize) -> String {
        match index {
            0 => self.status_subtitle(),
            1 => if self.focus_waiting { "waiting on you · f shows all".into() } else { String::new() },
            2 => self.reviews_subtitle(),
            _ => self.todo_subtitle(),
        }
    }

    /// Fixed panels keep their size; of the flexible ones, the focused takes half the rest.
    pub fn layout(&self, height: u16) -> Vec<u16> {
        let status_height = self.panels[0].lines.len() as i64 + 2;
        let flexible = [1usize, 2, 3];
        let available = height as i64 - HEADER_LINES as i64 - 1 - status_height;
        let expanded = if flexible.contains(&self.focus) { self.focus } else { flexible[0] };
        let minimum = MIN_PANEL_LINES as i64;
        let others = minimum.max((available - minimum.max(available / 2)) / (flexible.len() as i64 - 1).max(1));
        let mut heights = vec![status_height as u16, 0, 0, 0];
        for index in flexible {
            heights[index] = others as u16;
        }
        heights[expanded] = minimum.max(available - others * (flexible.len() as i64 - 1)) as u16;
        heights
    }

    pub fn render(&mut self, buffer: &mut Buffer) {
        self.hit_lines.clear();
        self.hit_actions.clear();
        let (width, height) = (buffer.area.width, buffer.area.height);
        self.width = width;
        self.draw_header(buffer, width);
        let mut top = HEADER_LINES;
        for (index, panel_height) in self.layout(height).into_iter().enumerate() {
            self.draw_panel(buffer, index, top, panel_height, width);
            top += panel_height;
        }
        self.draw_footer(buffer, height.saturating_sub(1), width);
        if let Some(menu) = self.menu.as_mut() {
            menu.draw(buffer);
        }
        if self.help_open {
            self.draw_help(buffer);
        }
    }

    fn draw_panel(&mut self, buffer: &mut Buffer, index: usize, top: u16, height: u16, width: u16) {
        if height < 2 || width < 4 {
            return;
        }
        let focused = index == self.focus;
        let border = if focused { bold(BLUE) } else { style(BG_HIGHLIGHT) };
        let inner_height = (height - 2) as usize;
        let subtitle_text = self.subtitle(index);
        let panel = &mut self.panels[index];
        panel.keep_selected_visible(inner_height);
        let inner = (width - 3) as usize;
        put(buffer, top, 0, &format!("┌{}┐", "─".repeat(inner)), border);
        let label = format!("[{}] {}", PANEL_KEYS[index], PANEL_NAMES[index]);
        put(buffer, top, 2, &label, bold(if focused { BLUE } else { FG_DARK }));
        let subtitle = fit(&subtitle_text, (width as usize).saturating_sub(8 + width_of(&label)));
        if !subtitle.is_empty() {
            put(buffer, top, 2 + width_of(&label) as u16, &format!(" · {subtitle} "), dim(FG_DARK));
        }
        for offset in 0..inner_height {
            let row = top + 1 + offset as u16;
            put(buffer, row, 0, "│", border);
            put(buffer, row, width - 2, "│", border);
            let line_index = panel.scroll + offset;
            let Some(line) = panel.lines.get(line_index) else { continue };
            let highlighted = focused && line.item.is_some() && line.item == Some(panel.selected);
            let recolor = |style: Style, fg: u8| -> Style { if highlighted { style.fg(color(fg)).bg(color(BG_HIGHLIGHT)) } else { style } };
            if highlighted {
                put(buffer, row, 1, &" ".repeat(inner), on(FG, BG_HIGHLIGHT));
            }
            if let Some(accent) = line.accent {
                put(buffer, row, 1, "▌", if highlighted { accent.fg(color(BLUE)).bg(color(BG_HIGHLIGHT)) } else { accent });
            }
            let right_width: usize = if line.right.is_empty() { 0 } else { line.right.iter().map(|p| width_of(&p.text)).sum::<usize>() + 2 * (line.right.len() - 1) };
            for (column, text, tint) in &line.segments {
                let room = (width as i64 - 4 - *column as i64 - if right_width > 0 { right_width as i64 + 2 } else { 0 }).max(0) as usize;
                put(buffer, row, 2 + column, &fit(text, room), recolor(*tint, FG));
            }
            let mut column = (width as i64 - 3 - right_width as i64).max(2) as u16;
            for part in &line.right {
                put(buffer, row, column, &part.text, recolor(part.style, FG_DARK));
                let end = column + width_of(&part.text) as u16;
                if let Some(action) = part.action {
                    self.hit_actions.insert(row, (column, end, action));
                }
                column = end + 2;
            }
            self.hit_lines.insert(row, (index, line.item));
        }
        put(buffer, top + height - 1, 0, &format!("└{}┘", "─".repeat(inner)), border);
        let hidden_below = panel.lines.len() as i64 - panel.scroll as i64 - inner_height as i64;
        if hidden_below > 0 {
            let note = format!(" ▼ {hidden_below} ");
            put(buffer, top + height - 1, width - 3 - width_of(&note) as u16, &note, dim(FG_DARK));
        }
        if panel.scroll > 0 {
            let note = format!(" ▲ {} ", panel.scroll);
            put(buffer, top, width - 3 - width_of(&note) as u16, &note, dim(FG_DARK));
        }
    }

    /// Title, the needs-you count and the clock (the bar is hidden on this workspace).
    fn draw_header(&self, buffer: &mut Buffer, width: u16) {
        let title = "Claude sessions";
        put(buffer, 0, 2, title, bold(BLUE));
        let needing = self.sessions.iter().filter(|s| s.state == "needs-you").count();
        let mut column = 2 + title.len() as u16 + 2;
        if needing > 0 {
            let text = format!("{needing} need you");
            put(buffer, 0, column, &text, bold(RED));
            column += text.len() as u16 + 2;
        }
        if !self.held.is_empty() {
            put(buffer, 0, column, &format!("⛔ {}", self.held.len()), bold(YELLOW));
        }
        let stamp = chrono::Local::now().format("%H:%M  %a %d %b").to_string();
        put(buffer, 0, width.saturating_sub(2 + stamp.len() as u16).max(2), &stamp, style(FG_DARK));
        let cost = self.costs.totals().today;
        if cost > 0.0 {
            let amount = format!("≈ {}", money(cost));
            let label = " today";
            let start = width as i64 - 2 - stamp.len() as i64 - 3 - width_of(&amount) as i64 - label.len() as i64;
            if start > 2 + title.len() as i64 + if needing > 0 { 12 } else { 0 } + 2 {
                put(buffer, 0, start as u16, &amount, bold(MAGENTA));
                put(buffer, 0, start as u16 + width_of(&amount) as u16, label, dim(FG_DARK));
            }
        }
    }

    fn draw_footer(&self, buffer: &mut Buffer, row: u16, width: u16) {
        if self.filtering {
            put(buffer, row, 2, "/", bold(BLUE));
            put(buffer, row, 4, &format!("{}▏", self.query), style(FG));
            return;
        }
        if !self.message.is_empty() {
            put(buffer, row, 2, &self.message, style(YELLOW));
            return;
        }
        let mut column = 2u16;
        for (key, label) in self.panel_hints(self.focus).into_iter().chain(GLOBAL_HINTS) {
            if column as usize + width_of(key) + width_of(label) + 3 > width as usize {
                break;
            }
            put(buffer, row, column, key, bold(BLUE));
            column += width_of(key) as u16 + 1;
            put(buffer, row, column, label, dim(FG_DARK));
            column += width_of(label) as u16 + 2;
        }
    }

    fn draw_help(&self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        let mut rows: Vec<(String, Option<String>)> = Vec::new();
        let mut sections: Vec<(String, Vec<(&str, &str)>)> = vec![("Everywhere".into(), GLOBAL_HELP.to_vec())];
        for index in 0..4 {
            sections.push((format!("[{}] {}", PANEL_KEYS[index], PANEL_NAMES[index]), Self::panel_help(index)));
        }
        sections.push(("Glyphs".into(), LEGEND.to_vec()));
        for (title, entries) in sections {
            rows.push((title, None));
            rows.extend(entries.into_iter().map(|(key, label)| (key.to_string(), Some(label.to_string()))));
            rows.push((String::new(), Some(String::new())));
        }
        let box_height = height.saturating_sub(HEADER_LINES + 1);
        for line in 0..box_height {
            put(buffer, HEADER_LINES + line, 0, &" ".repeat(width.saturating_sub(1) as usize), on(FG, BG_HIGHLIGHT));
        }
        for (index, (key, label)) in rows.iter().take(box_height.saturating_sub(2) as usize).enumerate() {
            let row = HEADER_LINES + 1 + index as u16;
            match label {
                None => put(buffer, row, 2, key, on(MAGENTA, BG_HIGHLIGHT).add_modifier(Modifier::BOLD)),
                Some(label) => {
                    put(buffer, row, 2, key, on(YELLOW, BG_HIGHLIGHT).add_modifier(Modifier::BOLD));
                    put(buffer, row, 11, label, on(FG_DARK, BG_HIGHLIGHT));
                }
            }
        }
        put(buffer, HEADER_LINES + box_height.saturating_sub(1), 2, "any key closes", on(FG_DARK, BG_HIGHLIGHT).add_modifier(Modifier::DIM));
    }

    /// Asks a y/n question in the footer; true on y.
    pub fn confirm(&mut self, screen: &mut dyn Screen, question: &str) -> bool {
        self.message = format!("{question}  y/n");
        screen.draw(self);
        let answer = screen.next_key(None);
        self.message.clear();
        answer == Some(Key::Char('y'))
    }

    fn handle_click(&mut self, x: u16, y: u16, double: bool, screen: &mut dyn Screen) {
        let Some((panel_index, item)) = self.hit_lines.get(&y).copied() else { return };
        self.focus_panel(panel_index);
        let Some(item) = item else { return };
        self.panels[panel_index].selected = item;
        if let Some((start, end, "effort")) = self.hit_actions.get(&y).copied() {
            if (start..end).contains(&x) {
                if let Some(session) = self.selected_session() {
                    self.message = self.open_effort_menu(session, Some(y));
                }
                return;
            }
        }
        if double {
            self.message = self.handle_panel(Key::Enter, screen);
        }
    }

    fn handle_panel(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        match self.focus {
            0 => self.handle_status(key),
            1 => self.handle_sessions(key, screen),
            2 => self.handle_reviews(key),
            _ => self.handle_todo(key, screen),
        }
    }

    pub fn handle(&mut self, key: Key, screen: &mut dyn Screen) {
        self.message.clear();
        if self.menu.is_some() {
            self.message = self.handle_menu(key, screen);
            return;
        }
        if self.help_open {
            self.help_open = false;
            return;
        }
        if let Key::F(number @ 1..=3) = key {
            self.focus_panel(number as usize); // F1..F3 from the tmux Ctrl+digit binds, even mid-filter
            self.filtering = false;
            return;
        }
        if self.filtering {
            match key {
                Key::Esc => {
                    self.query.clear();
                    self.filtering = false;
                }
                Key::Enter => self.filtering = false,
                Key::Backspace => { self.query.pop(); }
                Key::Char(character) => self.query.push(character),
                _ => {}
            }
            return;
        }
        match key {
            Key::Click { x, y, double } => self.handle_click(x, y, double, screen),
            Key::Char(digit @ '0'..='3') => self.focus_panel(digit as usize - '0' as usize),
            Key::Tab => self.focus_panel((self.focus + 1) % 4),
            Key::BackTab => self.focus_panel((self.focus + 3) % 4),
            Key::Char('j') | Key::Down => self.panels[self.focus].move_by(1),
            Key::Char('k') | Key::Up => self.panels[self.focus].move_by(-1),
            // From any panel, as ← comes back from the session (tmux.conf).
            Key::Char('l') | Key::Right => { let _ = Command::new("claude-sessions").arg("focus-viewer").status(); }
            Key::Char('/') => self.filtering = true,
            Key::Esc => self.query.clear(),
            Key::Char('?') => self.help_open = true,
            Key::Char(':') => {
                let entries = self.palette_entries();
                if let Some(choice) = screen.outside(&mut || pick_action(&entries)) {
                    if let Some((panel, key)) = choice.split_once('\t') {
                        if let Ok(panel) = panel.parse::<usize>() {
                            self.focus_panel(panel);
                        }
                        let key = if key == "⏎" { Key::Enter } else { key.chars().next().map(Key::Char).unwrap_or(Key::Esc) };
                        self.handle(key, screen);
                    }
                }
            }
            Key::Char('a') => {
                if let Some(directory) = screen.outside(&mut || pick_project()).filter(|d| !d.is_empty()) {
                    self.message = manager(&["new", &directory]);
                }
            }
            Key::Char('D') => self.message = manager(&["digest"]),
            Key::Char('K') => manager_in_background(&["clones"]),
            Key::Char('H') => {
                let past = self.past_sessions();
                if let Some(index) = screen.outside(&mut || pick_past_session(&past)).and_then(|chosen| chosen.parse::<usize>().ok()) {
                    let chosen = &past[index].0;
                    self.message = manager(&["resume", &chosen.cwd, &chosen.session]);
                }
            }
            Key::Char('A') => {
                let directory = screen.outside(&mut || pick_project()).unwrap_or_default();
                if !directory.is_empty() {
                    let branch = input_line(screen, "branch for the parallel session> ");
                    if !branch.is_empty() {
                        manager_in_background(&["parallel", &directory, &branch]);
                        self.message = format!("cloning {} for {branch} …", project_name(&directory));
                    }
                }
            }
            Key::Char('V') => self.message = self.start_design_session(screen),
            Key::Char('P') => self.message = manager(&["prs"]),
            Key::Char('W') => self.message = open_in_scratchpad("claude", ""),
            Key::Char('G') => self.message = open_in_scratchpad("greyproxy", ""),
            Key::Char('q') | Key::Ctrl('c') => {
                if self.confirm(screen, "close the manager window? sessions keep running") {
                    let _ = Command::new("tmux").arg("detach-client").status();
                }
            }
            _ => self.message = self.handle_panel(key, screen),
        }
    }

    /// V: the canvas link, then the project; the session links its card and watches the comments.
    pub(crate) fn start_design_session(&mut self, screen: &mut dyn Screen) -> String {
        let typed = input_line(screen, "Claude Design canvas (its claude.ai link, empty cancels)> ");
        if typed.is_empty() {
            return String::new();
        }
        let Some(url) = sessions::canvas_address(&typed) else {
            return "not a canvas link: claude.ai/artifact/<id>, or a chat link with ?artifact=<id>".into();
        };
        let directory = screen.outside(&mut || pick_project()).unwrap_or_default();
        if directory.is_empty() {
            return String::new();
        }
        manager_in_background(&["new", &directory, &sessions::design_prompt(&url)]);
        format!("starting a design session in {} …", project_name(&directory))
    }

    /// Every single-key action from the help tables: "panel\tkey\tlabel" lines for fzf
    /// ("-" for anywhere).
    pub(crate) fn palette_entries(&self) -> Vec<String> {
        let mut entries: Vec<String> = GLOBAL_HELP.iter()
            .filter(|(key, _)| key.chars().count() == 1 && !":?".contains(*key))
            .map(|(key, label)| format!("-\t{key}\t{:<13} {key:<2} {label}", "everywhere"))
            .collect();
        for index in 0..4 {
            for (key, label) in Self::panel_help(index) {
                if key.chars().count() == 1 || key == "⏎" {
                    entries.push(format!("{index}\t{key}\t{:<13} {key:<2} {label}", PANEL_NAMES[index]));
                }
            }
        }
        entries
    }

    /// Past sessions not running now, newest first, with their titles.
    fn past_sessions(&mut self) -> Vec<(PastSession, String)> {
        self.history.refresh();
        let running: Vec<&str> = self.sessions.iter().map(|s| s.session.as_str()).collect();
        self.history.entries.iter()
            .filter(|entry| !running.contains(&entry.session.as_str()))
            .map(|entry| (entry.clone(), self.history.title(&entry.session)))
            .collect()
    }
}

fn claude_status_lines(claude_status: &ClaudeStatus, label_width: u16) -> Vec<Line> {
    let (current, error) = claude_status.snapshot();
    let Some(current) = current else {
        let text = if !error.is_empty() { error } else if paths::claude_status_url().is_empty() { "not checked".into() } else { "checking status.claude.com …".into() };
        return vec![Line::new(vec![segment(0, CLAUDE_LABEL, style(FG)), segment(label_width + 2, text, dim(FG_DARK))])];
    };
    let tint = bold(claude_status_color(&current.indicator));
    let mut first = Line::new(vec![segment(0, CLAUDE_LABEL, style(FG)), segment(label_width + 2, format!("● {}", current.description), tint)]);
    if !error.is_empty() {
        first.right = vec![part("stale", style(YELLOW))];
    }
    let mut lines = vec![first];
    for (name, state) in current.degraded.iter().take(3) {
        lines.push(Line::new(vec![segment(label_width + 4, format!("{name}: {state}"), tint)]));
    }
    lines
}

/// Ends a session gracefully through the manager, or closes its terminal window when it never
/// had a tmux pane, or else tells the daemon it is gone.
fn kill_session(session: &Session) -> String {
    if !session.tmux_pane.is_empty() {
        manager_in_background(&["exit", &session.session]);
        return format!("exiting {} …", session.project);
    }
    if !session.kitty_pid.is_empty() {
        util::refresh_hyprland_instance();
        let close = format!("hl.dsp.window.close({{ window = \"pid:{}\" }})", session.kitty_pid);
        let _ = Command::new("hyprctl").args(["dispatch", &close]).output();
        return format!("no tmux session for {}: closed its terminal window instead", session.project);
    }
    // Nothing to kill: if its hook is still alive it comes back, the sign it lives elsewhere.
    let report = serde_json::json!({"session": session.session, "state": "gone", "project": session.project});
    let _ = Command::new("osd-show").arg(format!("session {report}")).status();
    format!("nothing to kill for {}: removed it from the list", session.project)
}

/// hyprctl eval of OPEN_IN_SCRATCHPAD (scratchpads.lua): a cockpit web window, on a page.
pub fn scratchpad_call(name: &str, page: &str) -> Result<Vec<String>, String> {
    let address = regex::Regex::new(r"^https?://[\x21-\x7e]+$").unwrap();
    // The page reaches a shell through Hyprland's exec (scratchpads.lua): no quote, backslash, $
    // or backtick, which Lua's %q does not neutralise for sh.
    if !page.is_empty() && (!address.is_match(page) || page.contains(['"', '\\', '$', '`'])) {
        return Err(format!("not a page address: {page:?}"));
    }
    let arguments = if page.is_empty() { format!("\"{name}\"") } else { format!("\"{name}\", \"{page}\"") };
    Ok(vec!["hyprctl".into(), "eval".into(), format!("OPEN_IN_SCRATCHPAD({arguments})")])
}

fn open_in_scratchpad(name: &str, page: &str) -> String {
    let call = match scratchpad_call(name, page) {
        Ok(call) => call,
        Err(error) => return error,
    };
    util::refresh_hyprland_instance();
    match Command::new(&call[0]).args(&call[1..]).output() {
        Ok(output) if output.status.success() => String::new(),
        Ok(output) => {
            let text = if output.stderr.is_empty() { output.stdout } else { output.stderr };
            format!("hyprctl: {}", String::from_utf8_lossy(&text).trim().chars().take(60).collect::<String>())
        }
        Err(error) => format!("hyprctl: {error}"),
    }
}

/// fzf over lines; the chosen line, or None.
fn fzf(prompt: &str, lines: &[String], extra: &[&str]) -> Option<String> {
    let mut child = Command::new("fzf").args(["--prompt", prompt, "--height", "100%", "--no-sort"]).args(extra)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(lines.join("\n").as_bytes());
    }
    let output = child.wait_with_output().ok()?;
    let chosen = String::from_utf8_lossy(&output.stdout).trim_end_matches('\n').to_string();
    (output.status.success() && !chosen.is_empty()).then_some(chosen)
}

fn pick_project() -> Option<String> {
    fzf("project> ", &sessions::project_choices(), &[])
}

/// "panel\tkey" of the chosen action.
fn pick_action(entries: &[String]) -> Option<String> {
    let chosen = fzf("action> ", entries, &["--delimiter", "\t", "--with-nth", "3.."])?;
    let mut fields = chosen.splitn(3, '\t');
    Some(format!("{}\t{}", fields.next()?, fields.next()?))
}

/// The index of the chosen past session.
fn pick_past_session(past: &[(PastSession, String)]) -> Option<String> {
    let lines: Vec<String> = past.iter().enumerate().map(|(index, (entry, title))| {
        let when = chrono::DateTime::from_timestamp(entry.since, 0).map(|t| t.with_timezone(&chrono::Local).format("%d %b %H:%M").to_string()).unwrap_or_default();
        format!("{index}\t{when}  {:<18} {}", entry.project, if title.is_empty() { &entry.prompt } else { title })
    }).collect();
    let chosen = fzf("resume> ", &lines, &["--delimiter", "\t", "--with-nth", "2.."])?;
    chosen.split('\t').next().map(str::to_string)
}

/// A line typed in the plain terminal, outside the column's screen.
fn input_line(screen: &mut dyn Screen, prompt: &str) -> String {
    screen.outside(&mut || {
        print!("\n  {prompt}");
        let _ = io::stdout().flush();
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line).ok()?;
        Some(line.trim().to_string())
    }).unwrap_or_default()
}

/// Reads the next key or click from the terminal, waiting up to `timeout` (forever for None).
pub fn read_key(timeout: Option<Duration>, last_click: &mut Option<(Instant, u16)>) -> Option<Key> {
    let deadline = timeout.map(|t| Instant::now() + t);
    loop {
        let wait = match deadline {
            Some(deadline) => deadline.saturating_duration_since(Instant::now()),
            None => Duration::from_secs(3600),
        };
        if !event::poll(wait).ok()? {
            if deadline.is_some() {
                return None;
            }
            continue;
        }
        match event::read().ok()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let control = key.modifiers.contains(KeyModifiers::CONTROL);
                return Some(match key.code {
                    KeyCode::Char(c) if control => Key::Ctrl(c.to_ascii_lowercase()),
                    KeyCode::Char(c) => Key::Char(c),
                    KeyCode::Enter => Key::Enter,
                    KeyCode::Tab => Key::Tab,
                    KeyCode::BackTab => Key::BackTab,
                    KeyCode::Esc => Key::Esc,
                    KeyCode::Backspace => Key::Backspace,
                    KeyCode::Up => Key::Up,
                    KeyCode::Down => Key::Down,
                    KeyCode::Left => Key::Left,
                    KeyCode::Right => Key::Right,
                    KeyCode::F(number) => Key::F(number),
                    _ => continue,
                });
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                let double = last_click.is_some_and(|(at, row)| row == mouse.row && at.elapsed() < DOUBLE_CLICK);
                *last_click = Some((Instant::now(), mouse.row));
                return Some(Key::Click { x: mouse.column, y: mouse.row, double });
            }
            Event::Resize(..) => return None,
            _ => continue,
        }
    }
}

pub const POLL_INTERVAL: Duration = POLL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_cuts_with_an_ellipsis_by_cell_width() {
        assert_eq!(fit("Checkout totals", 8), "Checkou…");
        assert_eq!(fit("short", 8), "short");
        assert_eq!(fit("x", 1), "");
    }

    #[test]
    fn a_page_address_is_checked_before_it_reaches_lua() {
        assert!(scratchpad_call("prs", "https://github.com/o/r/pull/7").is_ok());
        assert!(scratchpad_call("prs", "https://x\"); os.execute(\"rm").is_err());
    }
}
