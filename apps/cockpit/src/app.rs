//! The column itself: four stacked panels (Status, Sessions, Pull requests, Todo), the header and
//! the footer, the effort and settings menus, the help, and every key. The focused panel takes half the column;
//! Status keeps its size.

use crate::agents::{self, Feature};
use crate::costs::Costs;
use crate::errors::ErrorReporter;
use crate::paths;
use crate::reviews::{self, Review, Reviews};
use crate::sessions::{self, CONTEXT_COMPACT_AT, History, PastSession, Restorable, Session, ViewState};
use crate::settings::{self, LAUNCH_SETTINGS, SETTINGS};
use crate::status::{self, ClaudeStatus};
use crate::todo::{self, Entry, TodoList};
use crate::usage::{self, BudgetAlerts, Refresher, Window};
use crate::prompts;
use crate::{approval, remote};
use crate::util::{self, age, current_session, manager, manager_in_background, money, now, project_name};
use serde_json::Value;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::fs;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// kitty's 16-colour palette is Tokyo Night, so plain colour indexes match the rest of the desktop.
pub const BLACK: u8 = 0;
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
/// How long a session that exited cleanly may stay in the snapshot before its card shows again.
const EXIT_SETTLE_S: f64 = 15.0;
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
const TIME_LABEL: &str = "Time with Claude";
/// The Todo panel's tabs: the statuses each one shows. Open merges everything not done yet.
const TODO_TABS: [(&str, &[&str]); 5] = [("Open", &[todo::PROPOSED, "todo", "doing"]), ("Proposed", &[todo::PROPOSED]), ("To do", &["todo"]),
                                         ("In progress", &["doing"]), ("Done", &["done"])];
const TODO_TAB_OPEN: usize = 0;
const TODO_TAB_PROPOSED: usize = 1;
const TODO_TAB_TO_DO: usize = 2;
/// What follows this line in the todo editor is help, left out of the item.
const EDITOR_HELP_MARK: &str = "#── everything from this line on is left out ──";
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const PANEL_NAMES: [&str; 4] = ["Status", "Sessions", "Pull requests", "Todo"];
const PANEL_KEYS: [&str; 4] = ["0", "1", "2", "3"];
const GLOBAL_HINTS: [(&str, &str); 5] = [("⇥", "panel"), (":", "actions"), ("a", "new"), ("?", "help"), ("q", "close")];
/// ⌘ in key hints: the physical Command key, which keyd turns into Ctrl (system/etc/keyd).
const GLOBAL_HELP: [(&str, &str); 14] = [
    ("0-3", "focus a panel"), ("H", "resume a past session, searched by project and prompt"),
    ("⌘1 ⌘2 ⌘3", "focus a panel from inside the session (tmux)"), ("⇥ ⇧⇥", "next / previous panel"), ("j/k ↑↓", "move inside the panel"),
    (":", "every action, searchable; includes new-with-agent to override the default"), ("a", "new session in a project using the default agent from Settings (S)"),
    ("O", "new opencode session in a project, overriding the default agent: same sandbox, card and keys, but what only Claude Code does (effort, $, remote, /compact) is done in its pane"),
    ("S", "settings: default agent, OpenCode model and model-specific effort; on/off choices; and the phone page, pairing (QR code), forgetting a phone (x)"),
    ("P W G", "windows on top: pull requests, Claude on the web, greyproxy (also Super+Ctrl+P/A/G)"),
    ("≈ $", "today's Claude Code and opencode replies on this machine at API prices (the plan is flat); week and month in Status"),
    ("⌘⇥", "cursor between the column and the session (tmux)"),
    ("l →  ←", "into the session from any panel; ← from the session comes back"),
    ("q ⌘C", "close the manager window; sessions keep running"),
];
/// Rare actions with no key, only in the : list: the panel they run in ("-" for none), a name, a label.
const PALETTE_ONLY: [(&str, &str, &str); 8] = [
    ("-", "new-with-agent", "new session with an explicit agent: pick a project, then Claude Code or opencode"),
    ("-", "digest", "the digest: yesterday, today, plan usage, who waits, pull requests (also once a day by itself)"),
    ("-", "design", "a design session: a Claude Design canvas, worked on by a session in a project (link, then project)"),
    ("-", "clones", "the clones of + and reviews: delete those with no session, no uncommitted change and nothing unpushed (also once a day by itself)"),
    ("0", "status-page", "open status.claude.com: Anthropic's incidents and each product's state"),
    ("1", "remote", "toggle Remote Control (phone / claude.ai/code) for the selected session"),
    ("1", "drop-queued", "drop the messages queued for the selected session"),
    ("-", "pair-phone", "a QR code that pairs a phone with the phone page (claude-remote), once, within 10 minutes (also in S)"),
];
const LEGEND: [(&str, &str); 24] = [
    ("●", "working"), ("○", "waiting for you"), ("!", "needs you: permission or question"), ("·", "running, state unknown"),
    ("▌", "shown in the session pane; red means the session needs you"), ("⚲", "pinned"), ("⇅", "remote controlled"), ("◇ opencode", "run by opencode instead of Claude Code (O starts one)"),
    ("◆ figcraft", "connected to FigCraft, which is open for this session's project (craft-open)"),
    ("◷ 2 agents + background", "background agents, and other background work (shells, monitors), still running when its turn ended"),
    ("ask-to-run: approval / running / input", "this session's command awaits approval, runs outside the sandbox, or needs your input; clears when finished"),
    ("! Needs you", "red marker and rail: this session needs approval, input, or an answer; cards keep their place"),
    ("✉ 2 queued", "messages waiting for the end of its turn, sent one per turn (m, Alt+Q; M drops them)"),
    ("↑2 unpushed", "commits on no remote yet: U pushes the branch"),
    ("⏱ 1h20 today · 34h", "time spent with Claude (and opencode) on the project, today and in all: the gaps under 5 minutes between its sessions' replies"),
    ("↑2 unapplied", "a dotfiles clone's commits ~/dotfiles lacks: U shows them and applies them after a yes"),
    ("↑2 unpushed (dotfiles)", "on a dotfiles card, commits ~/dotfiles holds and GitHub lacks: U pushes them after a yes"),
    ("▭ terminal", "started with \"this terminal\": no ⌘R, restore, queue, error hand-off or ask-to-run"),
    ("✎ designer ⌨ developer", "its role in a project designed in FigCraft (claude-design role): design to-dos go to the designer"),
    ("✎ design", "a to-do for the designer session: a gap a developer found in the design (claude-todo --design)"),
    ("⛔ opusline", "a session of that project was blocked by the setup (claude-todo blocked): queued for dotfiles"),
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
        // A collapsed panel shows no line: scrolling it would flip between 0 and 1 at every frame.
        if inner_height == 0 {
            self.scroll = 0;
            return;
        }
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
    /// tmux moved its focus into the column or out of it (focus-events).
    FocusGained,
    FocusLost,
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

/// The settings and new-session defaults in the column's top right corner (S), and below them the phone
/// page (claude-remote): whether its server answers and the tailnet publishes it, a row that pairs
/// a phone, and the paired phones.
pub struct SettingsMenu {
    pub(crate) selected: usize,
    rows: HashMap<u16, usize>,
    area: (u16, u16, u16, u16), // top, left, bottom, right
    pub(crate) phones: Vec<Value>,
    phone_health: Arc<Mutex<Option<PhoneHealth>>>,
    catalog: Option<ModelCatalog>,
    scroll: usize,
}

/// Whether the phone page's server answers, and whether tailscale serve publishes it (None: Tailscale
/// did not say); checked once the menu opens.
type PhoneHealth = (bool, Option<bool>);
type ModelCatalog = Arc<Mutex<Option<Result<Vec<settings::ModelChoice>, String>>>>;

/// What a row of the settings menu does.
#[derive(Debug, PartialEq)]
pub(crate) enum SettingsRow {
    Setting(usize),
    Launch(usize),
    PairPhone,
    Phone(usize),
}

enum SettingsLine {
    Space,
    Section(&'static str),
    Health,
    Choice(usize),
}

impl SettingsMenu {
    fn new() -> Self {
        let phone_health = Arc::new(Mutex::new(None));
        let shared = phone_health.clone();
        // Half a second at worst, off the key's path: the menu shows "checking" meanwhile.
        std::thread::spawn(move || {
            let health = (remote::server_answers(), remote::is_published());
            *shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(health);
        });
        Self { selected: 0, rows: HashMap::new(), area: (0, 0, 0, 0), phones: remote::phones(), phone_health, catalog: None, scroll: 0 }
    }

    fn row_count(&self) -> usize {
        SETTINGS.len() + LAUNCH_SETTINGS.len() + 1 + self.phones.len()
    }

    pub(crate) fn row(&self, index: usize) -> SettingsRow {
        let phone_start = LAUNCH_SETTINGS.len() + SETTINGS.len() - 1;
        match index {
            index if index < LAUNCH_SETTINGS.len() => SettingsRow::Launch(index),
            index if index < phone_start => SettingsRow::Setting(index - LAUNCH_SETTINGS.len() + 1),
            index if index == phone_start => SettingsRow::Setting(0),
            index if index == phone_start + 1 => SettingsRow::PairPhone,
            index => SettingsRow::Phone(index - phone_start - 2),
        }
    }

    fn health_line(&self) -> (String, u8) {
        match *self.phone_health.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) {
            None => ("server … checking".into(), FG_DARK),
            Some((false, _)) => ("server ● down".into(), RED),
            Some((true, Some(false))) => ("server ● up · not on tailnet".into(), YELLOW),
            Some((true, None)) => ("server ● up · tailnet ?".into(), YELLOW),
            Some((true, Some(true))) => ("server ● up · tailnet ● on".into(), GREEN),
        }
    }

    fn draw(&mut self, buffer: &mut Buffer) {
        let width = buffer.area.width;
        let top = HEADER_LINES;
        self.rows.clear();
        if width < 16 || buffer.area.height < top + 6 { return; }
        let mut lines = Vec::new();
        for index in 0..self.row_count() {
            match self.row(index) {
                SettingsRow::Launch(0) => lines.push(SettingsLine::Section("Sessions")),
                SettingsRow::Setting(1) => {
                    lines.push(SettingsLine::Space);
                    lines.push(SettingsLine::Section("Automation"));
                }
                SettingsRow::Setting(0) => {
                    lines.push(SettingsLine::Space);
                    lines.push(SettingsLine::Section("Phone"));
                    lines.push(SettingsLine::Health);
                }
                _ => {}
            }
            lines.push(SettingsLine::Choice(index));
        }
        let box_height = (lines.len() as u16 + 4).min(buffer.area.height - top - 1);
        let body_height = (box_height - 4) as usize;
        let selected_line = lines.iter().position(|line| matches!(line, SettingsLine::Choice(index) if *index == self.selected)).unwrap_or(0);
        self.scroll = self.scroll.min(lines.len().saturating_sub(body_height));
        if selected_line < self.scroll { self.scroll = selected_line; }
        if selected_line >= self.scroll + body_height { self.scroll = selected_line + 1 - body_height; }
        let menu_width = width.saturating_sub(4).min(54);
        let left = width.saturating_sub(3 + menu_width).max(1);
        let right = left + menu_width - 1;
        self.area = (top, left, top + box_height - 1, right);
        let frame = on(FG_DARK, BLACK);
        let ground = on(FG, BLACK);
        let dim = on(FG_DARK, BLACK);
        let heading = on(BLUE, BLACK).add_modifier(Modifier::BOLD);
        let inner = menu_width.saturating_sub(2) as usize;
        let value_left = left + if menu_width >= 44 { 26 } else { menu_width / 2 };
        let label_width = value_left.saturating_sub(left + 5) as usize;
        let value_width = right.saturating_sub(value_left + 3) as usize;
        let blank = |buffer: &mut Buffer, row: u16| put(buffer, row, left, &format!("│{}│", " ".repeat(inner)), frame);
        put(buffer, top, left, &format!("╭{}╮", "─".repeat(inner)), frame);
        put(buffer, top, left + 2, " Settings ", heading);
        let mut row = top + 1;
        for line in lines.iter().skip(self.scroll).take(body_height) {
            blank(buffer, row);
            let index = match line {
                SettingsLine::Section(label) => {
                    put(buffer, row, left + 2, &format!("{label} {}", "─".repeat(inner.saturating_sub(label.len() + 4))), heading);
                    row += 1;
                    continue;
                }
                SettingsLine::Space => { row += 1; continue; }
                SettingsLine::Health => {
                    let (health, color) = self.health_line();
                    put(buffer, row, left + 4, &fit(&health, inner - 4), on(color, BLACK));
                    row += 1;
                    continue;
                }
                SettingsLine::Choice(index) => *index,
            };
            let chosen = index == self.selected;
            let background = if chosen { BG_HIGHLIGHT } else { BLACK };
            let text_style = if chosen { on(FG, background).add_modifier(Modifier::BOLD) } else { ground };
            put(buffer, row, left + 1, &" ".repeat(inner), on(FG, background));
            if chosen { put(buffer, row, left + 2, "›", on(BLUE, background).add_modifier(Modifier::BOLD)); }
            match self.row(index) {
                SettingsRow::Setting(setting) => {
                    let (name, label) = SETTINGS[setting];
                    let is_on = settings::is_on(name);
                    put(buffer, row, left + 4, &fit(label, label_width), text_style);
                    let state_color = if is_on { GREEN } else if chosen { FG } else { FG_DARK };
                    let state_style = on(state_color, background).add_modifier(if is_on || chosen { Modifier::BOLD } else { Modifier::empty() });
                    put(buffer, row, value_left, if is_on { "on" } else { "off" }, state_style);
                }
                SettingsRow::Launch(setting) => {
                    let (name, label) = LAUNCH_SETTINGS[setting];
                    let (value, color) = match settings::launch_value(name) {
                        Ok(value) if value.is_empty() => ("OpenCode default".into(), FG_DARK),
                        Ok(value) => (value, CYAN),
                        Err(error) => (error, RED),
                    };
                    put(buffer, row, left + 4, &fit(label, label_width), text_style);
                    put(buffer, row, value_left, &fit(&value, value_width), on(color, background).add_modifier(Modifier::BOLD));
                    put(buffer, row, right - 2, "▾", on(FG_DARK, background));
                }
                SettingsRow::PairPhone => put(buffer, row, left + 4, &fit("Pair a phone (QR code)", inner - 4), text_style),
                SettingsRow::Phone(phone) => {
                    let phone = &self.phones[phone];
                    let seen = format!("seen {}", remote::seen_ago(phone.get("last_seen").and_then(Value::as_i64)));
                    let seen_width = seen.width().min(inner / 2);
                    put(buffer, row, left + 4, &fit(phone["name"].as_str().unwrap_or(""), inner.saturating_sub(seen_width + 5)), text_style);
                    put(buffer, row, right - seen_width as u16 - 1, &fit(&seen, seen_width), on(FG_DARK, background));
                }
            }
            self.rows.insert(row, index);
            row += 1;
        }
        while row < top + box_height - 3 { blank(buffer, row); row += 1; }
        blank(buffer, row);
        put(buffer, row, left + 2, &fit("↑↓ move   esc close", inner - 2), dim);
        row += 1;
        blank(buffer, row);
        let keys = match self.row(self.selected) {
            SettingsRow::Phone(_) => "x forget phone",
            SettingsRow::Launch(1 | 2) => "⏎ choose   d reset to default",
            SettingsRow::Launch(_) => "⏎ choose agent",
            SettingsRow::PairPhone => "⏎ pair phone",
            SettingsRow::Setting(_) => "⏎ toggle",
        };
        put(buffer, row, left + 2, &fit(keys, inner - 2), ground);
        put(buffer, row + 1, left, &format!("╰{}╯", "─".repeat(inner)), frame);
    }

    fn contains(&self, x: u16, y: u16) -> bool {
        let (top, left, bottom, right) = self.area;
        (top..=bottom).contains(&y) && (left..=right).contains(&x)
    }
}

/// A phone being paired: the one-time link's QR code over the column, until a key; it says so once
/// the phone opened it.
pub(crate) struct Pairing {
    pub(crate) link: String,
    pub(crate) digits: String,
    modules: Option<Vec<Vec<bool>>>,
    phones_before: usize,
    pub(crate) paired: Option<String>,
}

impl Pairing {
    fn draw(&self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        let ground = on(FG, BG_HIGHLIGHT);
        for line in HEADER_LINES..height.saturating_sub(1) {
            put(buffer, line, 0, &" ".repeat(width.saturating_sub(1) as usize), ground);
        }
        let mut row = HEADER_LINES + 1;
        if let Some(name) = &self.paired {
            put(buffer, row, 2, &format!("{name} is paired ✓"), on(GREEN, BG_HIGHLIGHT).add_modifier(Modifier::BOLD));
            put(buffer, row + 2, 2, "any key closes", on(FG_DARK, BG_HIGHLIGHT).add_modifier(Modifier::DIM));
            return;
        }
        put(buffer, row, 2, "Pair a phone", on(MAGENTA, BG_HIGHLIGHT).add_modifier(Modifier::BOLD));
        row += 2;
        // A scanned link opens in Safari, never in the Home Screen app: that app takes the digits.
        put(buffer, row, 2, "Home Screen app: type", on(FG, BG_HIGHLIGHT));
        put(buffer, row, 24, &format!("{} {}", &self.digits[..3], &self.digits[3..]), on(YELLOW, BG_HIGHLIGHT).add_modifier(Modifier::BOLD));
        put(buffer, row, 33, "(2 minutes, 5 tries)", on(FG_DARK, BG_HIGHLIGHT));
        row += 1;
        put(buffer, row, 2, "Browser: scan within 10 minutes, once", on(FG, BG_HIGHLIGHT));
        row += 2;
        let modules = self.modules.as_ref().filter(|modules| {
            modules[0].len() as u16 + 4 <= width && (modules.len() as u16).div_ceil(2) + row + 4 <= height
        });
        match modules {
            // Two module rows per terminal row (▀: the top one in the foreground), in true black and
            // white whatever the theme, for the phone's camera.
            Some(modules) => {
                let shade = |is_dark: bool| if is_dark { Color::Rgb(0, 0, 0) } else { Color::Rgb(255, 255, 255) };
                for pair in modules.chunks(2) {
                    for (column, &is_dark) in pair[0].iter().enumerate() {
                        let below = pair.get(1).is_some_and(|next| next[column]);
                        put(buffer, row, 2 + column as u16, "▀", Style::default().fg(shade(is_dark)).bg(shade(below)));
                    }
                    row += 1;
                }
            }
            None => {
                put(buffer, row, 2, "No room for the QR code here (or qrencode is missing): open the link on the phone.", on(YELLOW, BG_HIGHLIGHT));
                row += 1;
            }
        }
        row += 1;
        for line in approval::wrap(&self.link, width.saturating_sub(4) as usize) {
            put(buffer, row, 2, &line, on(FG_DARK, BG_HIGHLIGHT));
            row += 1;
        }
        put(buffer, row + 1, 2, "any key closes (the link keeps working until it is used or expires)", on(FG_DARK, BG_HIGHLIGHT).add_modifier(Modifier::DIM));
    }
}

/// What the terminal does for the app: draw it, read keys, and lend the terminal to fzf or a prompt.
pub trait Screen {
    fn draw(&mut self, app: &mut App);
    fn next_key(&mut self, timeout: Option<Duration>) -> Option<Key>;
    fn outside(&mut self, action: &mut dyn FnMut() -> Option<String>) -> Option<String>;
}

/// A session told to exit, and the `claude-sessions exit` doing it.
pub(crate) struct Exiting {
    pub(crate) session: String,
    project: String,
    child: std::process::Child,
    ended_at: Option<f64>,
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
    pub help_open: bool,
    /// Whether tmux's focus is in the column: out of it, nothing there is drawn as focused.
    pub has_pane_focus: bool,
    pub message: String,
    /// Sessions told to exit (x): off the list at once, back with a warning if the exit fails.
    pub(crate) exiting: Vec<Exiting>,
    pub(crate) labelled: (String, String),
    pub(crate) menu: Option<EffortMenu>,
    pub(crate) settings_menu: Option<SettingsMenu>,
    pub(crate) pairing: Option<Pairing>,
    pub restore: Vec<Restorable>,
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
    quota_rows: Vec<usage::QuotaRow>,
    pub(crate) review_rows: Vec<Review>,
    pub todo_entries: Vec<Entry>,
    pub(crate) todo_scope: String,
    /// The session in the session pane as of the last tick: its change wakes the column at once.
    shown_session: String,
    /// The Todo tab shown (TODO_TABS), and how many entries each holds.
    pub(crate) todo_tab: usize,
    /// session -> the Todo tab it last showed: a session not in it shows Open.
    todo_tabs: HashMap<String, usize>,
    /// session -> when the column last sent it a queued prompt: the next one waits for a turn that
    /// ended after that.
    pub(crate) prompts_sent: HashMap<String, i64>,
    /// craft app -> the checkout folders its running instances serve (sessions::craft_roots).
    pub(crate) craft_roots: HashMap<String, HashSet<String>>,
    pub(crate) todo_counts: [usize; 5],
    pub(crate) hit_lines: HashMap<u16, (usize, Option<usize>)>,
    pub(crate) hit_actions: HashMap<u16, (u16, u16, &'static str)>,
    /// The session cards' data as the last tick read it (gather_sessions); a key press redraws from
    /// it instead of reading tmux and the files again.
    gathered: Option<Gathered>,
    /// claude-sessions commands a key started (open, view, digest…), not waited for: the column stays
    /// responsive while they run; an error shows once one ends (collect_manager).
    pending: Vec<(String, std::process::Child)>,
    /// The plan's name (usage::plan, from the credentials file), read once a tick.
    plan: String,
    /// When craft_roots was last asked of pgrep (CRAFT_CHECK_S).
    craft_checked: f64,
    /// Commits ~/dotfiles holds and GitHub lacks (sessions::dotfiles_unpushed), and when counted.
    pub(crate) dotfiles_unpushed: usize,
    dotfiles_checked: f64,
}

/// pgrep for the craft apps' project folders forked on every tick (4 a second); which project
/// FigCraft serves changes when Theo opens it.
const CRAFT_CHECK_S: f64 = 5.0;

/// What the session cards are drawn from (App::gather_sessions).
struct Gathered {
    running: Vec<Session>,
    paused: usize,
    dotfiles_unpushed: usize,
    start_files: Vec<(&'static str, f64)>,
    craft_roots: HashMap<String, HashSet<String>>,
}

impl App {
    pub fn new() -> Self {
        Self {
            history: History::default(), view: ViewState::load(), usage: Refresher::new(), alerts: BudgetAlerts::new(util::send_notice),
            costs: Costs::default(), reviews: Reviews::default(), todos: TodoList::load(), errors: ErrorReporter::new(),
            claude_status: ClaudeStatus::default(), panels: Default::default(), focus: 1,
            help_open: false, has_pane_focus: true, message: String::new(), exiting: Vec::new(), labelled: Default::default(), menu: None, settings_menu: None, pairing: None, restore: Vec::new(),
            held: Vec::new(), held_checked: 0.0, queue_checked: 0.0, gaming: util::gaming, restore_checked: 0.0, start_files: Vec::new(),
            width: DEFAULT_COLUMN_WIDTH, sessions: Vec::new(), windows: Vec::new(), quota_rows: Vec::new(), review_rows: Vec::new(), todo_entries: Vec::new(),
            todo_scope: String::new(), shown_session: String::new(), todo_tab: TODO_TAB_OPEN, todo_tabs: HashMap::new(), todo_counts: [0; 5], prompts_sent: HashMap::new(), craft_roots: HashMap::new(), hit_lines: HashMap::new(), hit_actions: HashMap::new(),
            gathered: None, pending: Vec::new(), plan: String::new(), craft_checked: 0.0, dotfiles_unpushed: 0, dotfiles_checked: 0.0,
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
        self.shown_session = current_session();
        self.refresh();
        self.todos.apply_updates();
        self.errors.tick(sessions::reported);
        self.claude_status.tick();
        self.usage.tick(false);
        self.alerts.check(&self.windows);
        self.costs.tick();
        self.reviews.tick();
        self.watch_pairing();
        self.redraw();
        self.push_viewed_state();
        self.dispatch_queue();
        self.dispatch_prompts();
    }

    /// A claude-sessions command a key asked for, started and not waited for (it takes 40 to 60 ms,
    /// up to 500): the column keeps answering keys meanwhile (security audit 2026-10-19,
    /// performance 2). Its error, if any, shows once it ends (collect_manager).
    pub(crate) fn manager_async(&mut self, arguments: &[&str]) -> String {
        let started = Command::new("claude-sessions").args(arguments).env("CLAUDE_SESSIONS_LOG_STDERR", "1")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
        match started {
            Ok(child) => {
                self.pending.push((command_name(arguments).to_string(), child));
                String::new()
            }
            Err(error) => format!("claude-sessions {}: {error}", command_name(arguments)),
        }
    }

    /// The commands manager_async started that have ended; a failure's last error line becomes the
    /// message. True when one ended (the column redraws).
    pub fn collect_manager(&mut self) -> bool {
        use std::io::Read;
        let mut has_ended = false;
        for (command, mut child) in std::mem::take(&mut self.pending) {
            match child.try_wait() {
                Ok(None) => self.pending.push((command, child)),
                Ok(Some(status)) => {
                    has_ended = true;
                    if status.success() {
                        continue;
                    }
                    let mut errors = String::new();
                    if let Some(mut stderr) = child.stderr.take() {
                        let _ = stderr.read_to_string(&mut errors);
                    }
                    let last = errors.trim().lines().last().unwrap_or("").to_string();
                    util::log_event("ERROR", &format!("claude-sessions {command}: {}", if last.is_empty() { "failed" } else { &last }));
                    self.message = if last.is_empty() { format!("claude-sessions {command} failed") } else { last };
                }
                Err(error) => {
                    has_ended = true;
                    self.message = format!("claude-sessions {command}: {error}");
                }
            }
        }
        has_ended
    }

    /// A session craft-open asked to restart (its figcraft-app server had exited at its start) is
    /// restarted once its turn ends, in a tmux session of its own; its queued prompt then waits for
    /// the restarted session. A terminal session is its terminal's: it is only told in the log.
    fn dispatch_restarts(&mut self) {
        let ready: Vec<Session> = self.sessions.iter()
            .filter(|session| !session.is_external() && session.state == "waiting" && prompts::restart_wanted(&session.session)).cloned().collect();
        for session in ready {
            prompts::restart_done(&session.session);
            if session.terminal {
                util::log_event("WARN", &format!("{} asked for a restart to load figcraft-app, but runs in its own terminal: restart it there", session.project));
                continue;
            }
            manager_in_background(&["restart", &session.session, "", "detached"]);
            self.prompts_sent.insert(session.session.clone(), util::now() as i64);
            util::log_event("INFO", &format!("{} restarted to load figcraft-app", session.project));
        }
    }

    /// Each session whose turn has ended gets its oldest queued prompt (prompts.rs), one per turn:
    /// the next waits until the session worked and stopped again.
    pub fn dispatch_prompts(&mut self) {
        self.dispatch_restarts();
        let ready: Vec<Session> = self.sessions.iter()
            .filter(|session| !session.is_external() && session.state == "waiting" && session.since > self.prompts_sent.get(&session.session).copied().unwrap_or(0))
            .cloned().collect();
        for session in ready {
            let Some((text, path)) = prompts::oldest(&session.session) else { continue };
            let error = manager(&["say", &session.session, &text]);
            if !error.is_empty() {
                util::log_event("WARN", &format!("queued prompt for {} not sent yet: {error}", session.project));
                continue;
            }
            let _ = std::fs::remove_file(&path);
            self.prompts_sent.insert(session.session.clone(), util::now() as i64);
            util::log_event("INFO", &format!("queued prompt sent to {} at the end of its turn", session.project));
            crate::audit::record("column", "prompt.sent", "ok", serde_json::json!({"session": session.session, "project": session.project,
                                                                     "text": crate::audit::fingerprint(&text)}));
        }
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
        if !settings::is_on("todo-queue") {
            return;
        }
        // No new work while Theo plays: it would compete with the game (game-performance lowers
        // the sessions' priority, it does not stop them).
        if (self.gaming)() {
            return;
        }
        let viewed = current_session();
        let watched = !viewed.is_empty() && util::claude_workspace_active();
        let idle: Vec<Session> = self.sessions.iter()
            .filter(|session| !session.is_external() && session.state == "waiting" && !session.terminal && !session.cwd.is_empty() && now as i64 - session.since >= QUEUE_IDLE_S)
            .filter(|session| !(watched && session.session == viewed) && !self.todos.busy(&session.session))
            .cloned().collect();
        for session in idle {
            let Some(entry) = self.next_queued(&session) else { continue };
            let (key, text) = match &entry {
                Entry::Typed(item) => (item.id(), todo::prompt_of(&entry)),
                Entry::Issue { issue, .. } => (issue.key(), todo::prompt_of(&entry)),
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
        // Derive the project from its checkout, not the session's label. Parallel clones share
        // their source project's queue, just as they share its TODO panel.
        let project = util::project_name(&util::repository_root(&session.cwd));
        let mut candidates: Vec<Entry> = self.todos.items.iter()
            .filter(|item| util::project_name(&util::repository_root(&item.cwd())) == project && item.status() == "todo" && self.todos.is_queued(&item.id()))
            .filter(|item| item.is_design() == (session.role == "designer"))
            .map(|item| Entry::Typed(item.clone())).collect();
        for issue in self.reviews.issues() {
            let key = issue.key();
            let checkout = self.reviews.checkouts.get(&issue.full_repository);
            if self.todos.is_queued(&key) && self.todos.status_of(&key) == "todo"
                && checkout.is_some_and(|checkout| util::project_name(&util::repository_root(checkout)) == project) {
                let priority = todo::issue_priority(&issue.labels);
                candidates.push(Entry::Issue { issue, status: "todo".into(), priority });
            }
        }
        candidates.sort_by(todo::queue_order);
        candidates.into_iter().next()
    }

    /// Everything read again (sessions, usage, plan), then drawn.
    pub fn build(&mut self) {
        self.refresh();
        self.redraw();
    }

    /// What the panels are drawn from, read again: the session cards' data (gather_sessions, on the
    /// next draw), the usage windows and the plan. Once a tick, never on a key press.
    fn refresh(&mut self) {
        self.gathered = None;
        self.windows = usage::windows();
        self.quota_rows = usage::quota_rows();
        self.plan = usage::plan();
    }

    /// The panels drawn from what the last refresh read: a key press costs no tmux, pgrep or file
    /// read (15 to 40 ms each time it did; security audit 2026-10-19, performance 1).
    pub fn redraw(&mut self) {
        self.build_status();
        self.build_sessions();
        self.build_reviews();
        self.build_todo();
    }

    /// A duration as the column shows it: "42m", "3h05", then "34h".
    pub(crate) fn duration_text(seconds: f64) -> String {
        let minutes = (seconds / 60.0).round() as i64;
        match minutes {
            0..=59 => format!("{minutes}m"),
            60..=599 => format!("{}h{:02}", minutes / 60, minutes % 60),
            _ => format!("{}h", minutes / 60),
        }
    }

    /// Time spent with Claude on a project (Costs::time_spent), "⏱ 34h" or "⏱ 1h20 today · 34h"; None
    /// under a minute in all.
    pub(crate) fn time_spent_text((today, all_time): (f64, f64)) -> Option<String> {
        if all_time < 60.0 {
            return None;
        }
        let short = Self::duration_text;
        Some(if today >= 60.0 { format!("⏱ {} today · {}", short(today), short(all_time)) } else { format!("⏱ {}", short(all_time)) })
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
        // Every row's label shares the column, the cost and Claude rows too: sized on the usage rows
        // alone, it shrank to nothing before the first usage fetch and the values covered the labels.
        let label_width = self.quota_rows.iter().map(|row| width_of(&row.label)).chain([width_of(COSTS_LABEL), width_of(CLAUDE_LABEL), width_of(TIME_LABEL)]).max().unwrap_or(0) as u16;
        let mut lines = Vec::new();
        for (index, quota) in self.quota_rows.iter().enumerate() {
            let tint = bold(match quota.state.as_str() {
                "Available" if quota.remaining.is_some_and(|remaining| remaining <= 50) => YELLOW,
                "Available" => GREEN,
                "Stale" => YELLOW,
                "Low" => RED,
                "Exhausted" | "Blocked" => RED,
                _ => FG_DARK,
            });
            let summary = quota.summary();
            let room = self.width.saturating_sub(4) as usize;
            let quota_label_width = (label_width as usize).min(room.saturating_sub(width_of(&summary) + 2));
            let label = fit(&quota.label, quota_label_width);
            let end = quota_label_width + 2 + width_of(&summary);
            let show_reset = !quota.resets.is_empty() && end + 2 + width_of(&quota.resets) <= room;
            lines.push(Line {
                segments: vec![segment(0, label, style(FG)), segment(quota_label_width as u16 + 2, summary, tint)],
                right: if show_reset { vec![part(&quota.resets, dim(FG_DARK))] } else { Vec::new() },
                item: Some(index), accent: None,
            });
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
        // All sessions together, never the sum of the projects' times: parallel sessions overlap.
        let together = self.costs.time_together();
        if together.month >= 60.0 {
            let mut segments = vec![segment(0, TIME_LABEL, style(FG))];
            let mut column = label_width + 2;
            for (label, seconds) in [("today", together.today), ("week", together.week), ("month", together.month)] {
                let amount = Self::duration_text(seconds);
                segments.push(segment(column, label, dim(FG_DARK)));
                segments.push(segment(column + label.len() as u16 + 1, &amount, bold(CYAN)));
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
        panel.items = self.quota_rows.len();
        panel.clamp();
        panel.lines = lines;
    }

    fn status_subtitle(&self) -> String {
        let plan = self.plan.clone();
        let all_time = self.costs.all_time();
        if all_time > 0.0 { format!("{plan} · all time {}", money(all_time)).trim_start_matches([' ', '·']).to_string() } else { plan }
    }

    // ---- Sessions ----

    /// x on a card: a session in tmux leaves the list at once while `claude-sessions exit` runs.
    fn exit_session(&mut self, session: &Session) -> String {
        if session.tmux_pane.is_empty() {
            return kill_session(session);
        }
        match util::manager_child(&["exit", &session.session]) {
            Ok(child) => {
                self.exiting.push(Exiting { session: session.session.clone(), project: session.project.clone(), child, ended_at: None });
                format!("exiting {} …", session.project)
            }
            Err(error) => format!("⚠ cannot exit {}: {error}", session.project),
        }
    }

    /// Drops the exits that are over: the session is gone, or it failed and its card comes back
    /// with the reason in the footer. A clean exit may still be in the snapshot for a moment, until
    /// its hook says it ended; after EXIT_SETTLE_S it shows again rather than vanish unexplained.
    fn settle_exits(&mut self, running: &[Session]) {
        let now = now();
        let mut warnings = Vec::new();
        self.exiting.retain_mut(|exit| {
            if !running.iter().any(|s| s.session == exit.session) {
                return false;
            }
            if exit.ended_at.is_none() {
                match exit.child.try_wait() {
                    Ok(None) => return true,
                    Ok(Some(status)) if status.success() => exit.ended_at = Some(now),
                    Ok(Some(status)) => {
                        warnings.push(format!("⚠ {} did not exit ({status}): see ~/.local/state/claude-sessions/log", exit.project));
                        return false;
                    }
                    Err(error) => {
                        warnings.push(format!("⚠ {} exit unknown: {error}", exit.project));
                        return false;
                    }
                }
            }
            if exit.ended_at.is_some_and(|ended| now - ended > EXIT_SETTLE_S) {
                warnings.push(format!("⚠ {} still reports after its exit: it may live on elsewhere", exit.project));
                return false;
            }
            true
        });
        if let Some(warning) = warnings.pop() {
            self.message = warning;
        }
    }

    /// What the session cards are drawn from, read from tmux, pgrep and the per-session files: once
    /// per tick, never on a key press (security audit 2026-10-19, performance 1).
    fn gather_sessions(&mut self) -> Gathered {
        let mut running: Vec<Session> = sessions::running();
        let known: Vec<String> = running.iter().filter(|s| s.state != "unknown").map(|s| s.session.clone()).collect();
        self.history.load_titles(&known, TITLES_PER_TICK);
        for session in &mut running {
            if session.state != "unknown" {
                session.facts = sessions::statusline_facts(&session.session);
                session.title = self.history.title(&session.session);
            }
            session.design = sessions::design_of(&session.session);
            session.role = sessions::role_of(&session.session);
        }
        let has_dotfiles_card = running.iter().any(|s| sessions::is_dotfiles_clone(&s.cwd));
        Gathered {
            running,
            paused: std::fs::read_to_string(paths::game_frozen()).map(|list| list.lines().filter(|line| !line.trim().is_empty()).count()).unwrap_or(0),
            start_files: sessions::start_file_mtimes(),
            // What ~/dotfiles holds and GitHub lacks: U on a dotfiles card pushes it (apply_run). Counted
            // here, out of the sandbox, in the live checkout no session writes; only with a dotfiles
            // card on screen, and like craft_roots at most every CRAFT_CHECK_S.
            dotfiles_unpushed: if !has_dotfiles_card {
                0
            } else if now() - self.dotfiles_checked >= CRAFT_CHECK_S {
                self.dotfiles_checked = now();
                sessions::dotfiles_unpushed()
            } else {
                self.dotfiles_unpushed
            },
            craft_roots: if now() - self.craft_checked >= CRAFT_CHECK_S {
                self.craft_checked = now();
                ["figcraft"].iter().filter_map(|app| Some((app.to_string(), sessions::craft_roots(app)?))).collect()
            } else {
                self.craft_roots.clone()
            },
        }
    }

    fn build_sessions(&mut self) {
        let gathered = match self.gathered.take() {
            Some(gathered) => gathered,
            None => self.gather_sessions(),
        };
        let mut running: Vec<Session> = gathered.running.clone();
        self.settle_exits(&running);
        running.retain(|s| !s.is_external() && !self.exiting.iter().any(|exit| exit.session == s.session));
        for session in &mut running {
            session.pinned = self.view.pinned.contains(&session.session);
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
            let held = sessions::greyproxy_held(&paths::greyproxy_db());
            for host in held.iter().filter(|host| !self.held.contains(host)) {
                crate::audit::record("column", "greyproxy.held", "waiting", serde_json::json!({"destination": host}));
            }
            self.held = held;
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
        let paused = gathered.paused;
        if paused > 0 {
            lines.push(Line::new(vec![segment(0, format!("⏸ {paused} session{} paused while you game", if paused > 1 { "s" } else { "" }), bold(BLUE))]));
            lines.push(Line::new(vec![segment(2, "they resume where they stopped when the game ends", dim(FG_DARK))]));
            lines.push(Line::default());
        }
        self.start_files = gathered.start_files.clone();
        self.dotfiles_unpushed = gathered.dotfiles_unpushed;
        self.craft_roots = gathered.craft_roots.clone();
        self.gathered = Some(gathered);
        let outdated: Vec<&Session> = running.iter().filter(|s| !s.is_external() && !sessions::changed_since_start(s, &self.start_files).is_empty()).collect();
        if !outdated.is_empty() {
            let mut changes: Vec<&str> = outdated.iter().flat_map(|s| sessions::changed_since_start(s, &self.start_files)).collect();
            changes.sort();
            changes.dedup();
            let count = format!("{} session{}", outdated.len(), if outdated.len() > 1 { "s" } else { "" });
            lines.push(Line::new(vec![segment(0, format!("⟳ {count} started before a change to {}", changes.join(", ")), bold(YELLOW))]));
            lines.push(Line::new(vec![segment(2, "⌘R restarts the ones waiting at their prompt", dim(FG_DARK))]));
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
            let needs_attention = session.needs_attention();
            let tint = bold(if needs_attention { RED } else { state_color(&session.state) });
            let accent = (needs_attention || session.session == current).then_some(tint);
            let glyph = if needs_attention { "!" } else { sessions::glyph(&session.state) };
            let branch = util::git_branch(&session.cwd);
            let grouped = session.group_size > 1;
            // Sessions of one repository share a heading; task titles identify their cards,
            // with branch names kept below so short replies cannot replace their identity.
            let indent: u16 = if grouped { 2 } else { 0 };
            let rail = if grouped { vec![segment(0, "│", dim(project_color(&session.project)))] } else { vec![] };
            let time_spent = Self::time_spent_text(self.costs.time_spent(&session.project));
            if grouped && session.group_position == 0 {
                let mut heading = Line::new(vec![segment(0, &session.project, bold(project_color(&session.project))),
                                                 segment(width_of(&session.project) as u16 + 1, format!("· {} sessions", session.group_size), dim(FG_DARK))]);
                heading.right = time_spent.iter().map(|text| part(text, dim(FG_DARK))).collect();
                lines.push(heading);
            }
            let task_title = branch.strip_prefix("todo-")
                .and_then(|id| self.todos.items.iter().find(|item| item.id() == id && item.project() == session.project))
                .map(|item| util::collapse(&item.text()))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| util::collapse(&session.title));
            let segments = if grouped {
                let name = if !task_title.is_empty() { task_title.clone() }
                    else if branch.is_empty() { session.project.clone() } else { branch.clone() };
                [rail.clone(), vec![segment(2, glyph, tint), segment(4, name, bold(FG))]].concat()
            } else {
                let mut segments = vec![segment(0, glyph, tint), segment(2, &session.project, bold(project_color(&session.project)))];
                if !branch.is_empty() {
                    segments.push(segment(4 + width_of(&session.project) as u16, format!("⎇ {branch}"), dim(FG_DARK)));
                }
                segments
            };
            let facts = &session.facts;
            let mut badges = Vec::new();
            if needs_attention {
                badges.push(part("Needs you", bold(RED)));
            }
            if !session.ask_to_run.is_empty() {
                let tint = if session.ask_to_run == "running" { BLUE } else { RED };
                badges.push(part(format!("ask-to-run: {}", session.ask_to_run), bold(tint)));
            }
            if session.is_external() {
                badges.push(part("external", bold(CYAN)));
            }
            if !session.is_external() && facts.context.is_some_and(|context| context >= CONTEXT_COMPACT_AT) {
                badges.push(part("/compact", bold(YELLOW)));
            }
            if session.update {
                badges.push(part("⬆ update", bold(GREEN)));
            }
            if !session.is_external() && !sessions::changed_since_start(session, &self.start_files).is_empty() {
                badges.push(part("⟳ restart", bold(YELLOW)));
            }
            if facts.remote {
                badges.push(part("⇅ remote", bold(CYAN)));
            }
            if session.terminal {
                badges.push(part("▭ terminal", bold(MAGENTA)));
            }
            if session.credits {
                badges.push(part("$ credits", bold(YELLOW)));
            }
            if let Some(badge) = session.agent().badge {
                badges.push(part(badge, bold(BLUE)));
            }
            match session.role.as_str() {
                "designer" => badges.push(part("✎ designer", bold(MAGENTA))),
                "developer" => badges.push(part("⌨ developer", style(CYAN))),
                _ => {}
            }
            // Every session has the craft servers (craft-open): the badge marks only the sessions of
            // the checkouts the running instances serve, the ones that can work with them.
            for app in session.apps.split_whitespace() {
                let Some((name, "on")) = app.split_once(':') else { continue };
                let serves_it = self.craft_roots.get(name).is_none_or(|roots| roots.contains(&util::repository_root(&session.cwd)));
                if serves_it {
                    badges.push(part(format!("◆ {name}"), bold(GREEN)));
                }
            }
            if let Some(background) = sessions::background_badge(session) {
                badges.push(part(background, bold(BLUE)));
            }
            let queued = prompts::count(&session.session);
            if queued > 0 {
                badges.push(part(format!("✉ {queued} queued"), bold(CYAN)));
            }
            if session.unpushed > 0 {
                let count = if sessions::is_dotfiles_clone(&session.cwd) { "unapplied" } else { "unpushed" };
                badges.push(part(format!("↑{} {count}", session.unpushed), bold(YELLOW)));
            }
            if sessions::is_dotfiles_clone(&session.cwd) && self.dotfiles_unpushed > 0 {
                badges.push(part(format!("↑{} unpushed", self.dotfiles_unpushed), bold(YELLOW)));
            }
            if session.pinned {
                badges.push(part("⚲", style(YELLOW)));
            }
            // A project's time once: on its heading when it has several sessions, else on its card.
            if !grouped {
                if let Some(text) = &time_spent {
                    badges.push(part(text, dim(FG_DARK)));
                }
            }
            badges.push(part(age(session.since), tint));
            lines.push(Line { segments, right: badges, item: Some(index), accent });
            let title = if grouped && !task_title.is_empty() { branch.clone() }
                else if !task_title.is_empty() { task_title.clone() } else { sessions::visible_task(session) };
            let mut right = Vec::new();
            if session.state != "unknown" {
                right.push(part(&facts.model, dim(FG_DARK)));
                if !session.is_external() {
                    let effort = if facts.effort.is_empty() { "effort" } else { facts.effort.as_str() };
                    right.push(Part { text: format!("{effort} ▾"), style: style(CYAN), action: Some("effort") });
                }
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
            let empty = "No running session.";
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

    /// The selected session's keys, most pressing first: what its card asks for (an approval, a
    /// restart, /compact, a push…), then the everyday keys. The footer cuts the end of the list
    /// when the column is narrow, so the order is what keeps the useful keys on screen.
    pub(crate) fn sessions_hints(&self) -> Vec<(&'static str, &'static str)> {
        let selected = self.selected_session().unwrap_or_default();
        let mut hints = Vec::new();
        if selected.update {
            hints.push(("r", "restart to apply the update"));
        } else if !sessions::changed_since_start(&selected, &self.start_files).is_empty() {
            hints.push(("r", "restart to pick up the change"));
        }
        if selected.facts.context.is_some_and(|c| c >= CONTEXT_COMPACT_AT) {
            hints.push(("C", "/compact"));
        }
        if selected.design.is_some() {
            hints.push(("o", "canvas"));
        }
        if sessions::is_dotfiles_clone(&selected.cwd) && (selected.unpushed > 0 || self.dotfiles_unpushed > 0) {
            hints.push(("U", if selected.unpushed > 0 { "apply" } else { "push" }));
        } else if selected.unpushed > 0 {
            hints.push(("U", "push"));
        }
        hints.extend([("⏎", "open"), ("n", "peek"), ("e", "effort"), ("g", "git"), ("+", "another"), ("x", "exit")]);
        if !selected.session.is_empty() && selected.is_external() {
            hints.retain(|(key, _)| !session_key_requires_terminal(if *key == "⏎" { Key::Enter } else { Key::Char(key.chars().next().unwrap()) }));
            if sessions::next_attention_index(&self.sessions, self.panels[1].selected).is_none() {
                hints.retain(|(key, _)| *key != "n");
            }
        }
        let mut seen = std::collections::HashSet::new();
        hints.retain(|(key, _)| seen.insert(*key));
        let agent = selected.agent();
        hints.retain(|(key, _)| key.chars().next().and_then(|key| key_feature(Key::Char(key))).is_none_or(|(feature, _)| agent.can(feature)));
        hints
    }

    fn sessions_help() -> Vec<(&'static str, &'static str)> {
        vec![("⏎", "show the session and move the cursor into it"), ("n", "peek at the next session needing you"),
             ("x", "exit the session: Ctrl-C twice, then killed if it lingers"), ("r", "restart in place: same conversation, new process"),
             ("⌘R", "restart the sessions started before a change they lack (badge ⟳ restart), when they wait at their prompt; busy ones are named and left alone"),
             ("p", "pin on top"),
             ("e", "effort level: pick one, the session restarts on it (or click the level on the card)"),
             ("$", "bill the session to the Console's API credits, or back to the subscription: it restarts on the same conversation (badge $ credits)"),
             ("g", "lazygit on the session's repository, in a popup (q closes it)"),
             ("U", "push the session's branch to origin with your SSH key, after a yes (lazygit's sandbox cannot)"),
             ("+", "another session on this repository, on its own branch in a separate clone, using the default agent"),
             ("C", "/compact in the session, when it waits (the /compact badge suggests it)"),
             ("B", "after a reboot: bring back the sessions that were running"),
             ("o", "open the Claude Design canvas a design session works on (◆ on its card)")]
    }

    pub(crate) fn handle_sessions(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let session = self.selected_session();
        if session.as_ref().is_some_and(|session| session.is_external()) && session_key_requires_terminal(key) {
            return "external session: no terminal target; use its own window".into();
        }
        if let (Some(s), Some((feature, what))) = (session.as_ref(), key_feature(key)) {
            let agent = s.agent();
            if !agent.can(feature) {
                return format!("{what} is not {}'s to do: {} runs {}, do it in its pane", agent.label, s.project, agent.name);
            }
        }
        let known = session.as_ref().filter(|s| s.state != "unknown").cloned();
        match key {
            Key::Enter => session.map(|s| self.manager_async(&["open", &s.session])).unwrap_or_default(),
            Key::Char('n') => match sessions::next_attention_index(&self.sessions, self.panels[1].selected) {
                None => "nothing needs you".into(),
                Some(index) => {
                    self.panels[1].selected = index;
                    let viewed = self.sessions[index].session.clone();
                    self.manager_async(&["view", &viewed])
                }
            },
            Key::Char('p') => {
                if let Some(s) = known {
                    self.view.toggle_pin(&s.session);
                }
                String::new()
            }
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
            Key::Char('$') => match known {
                Some(s) => {
                    let (billing, label) = if s.credits { ("subscription", "the subscription") } else { ("credits", "API credits") };
                    if !self.confirm(screen, &format!("restart {} on {label}? same conversation", s.project)) {
                        return String::new();
                    }
                    manager_in_background(&["billing", &s.session, billing]);
                    format!("restarting {} on {label} …", s.project)
                }
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
                    let agent = match settings::launch_agent() { Ok(agent) => agent, Err(error) => return error };
                    manager_in_background(&["--agent", &agent, "parallel", &s.cwd, &branch]);
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
            Key::Char('o') => match session.and_then(|s| s.design) {
                Some(design) => {
                    let _ = Command::new("xdg-open").arg(&design.url).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                    format!("opening {} …", design.title)
                }
                None => "no canvas: the design action in the : list starts a design session".into(),
            },
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
                Some(s) if self.confirm(screen, &format!("exit {}?", s.project)) => self.exit_session(&s),
                _ => String::new(),
            },
            _ => String::new(),
        }
    }

    /// Restart every session sitting at its prompt: one that works or asks for a permission would
    /// lose its turn, so it is skipped and named.
    pub(crate) fn restart_all(&mut self, screen: &mut dyn Screen) -> String {
        // Only the sessions started before a change they do not have yet (the "⟳ restart" badge);
        // a terminal session is its terminal's and is left alone.
        let stale: Vec<&Session> = self.sessions.iter()
            .filter(|s| !s.is_external() && !s.terminal && !sessions::changed_since_start(s, &self.start_files).is_empty()).collect();
        let ready: Vec<Session> = stale.iter().filter(|s| s.state == "waiting").map(|s| (*s).clone()).collect();
        let busy: Vec<Session> = stale.iter().filter(|s| s.state == "working" || s.state == "needs-you").map(|s| (*s).clone()).collect();
        if ready.is_empty() {
            return if busy.is_empty() { "no session needs a restart".into() } else { "nothing to restart now: the sessions that need it are busy".into() };
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
        if session.is_external() {
            return "external session: no terminal target; use its own window".into();
        }
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

    fn toggle_setting(&mut self, index: usize) -> String {
        let (name, label) = SETTINGS[index];
        let wanted = if settings::is_on(name) { "off" } else { "on" };
        let failure = manager(&["setting", name, wanted]);
        if failure.is_empty() { format!("{label}: {wanted}") } else { failure }
    }

    /// What Enter or a click does on a row of the settings menu.
    fn choose_settings_row(&mut self, index: usize, screen: &mut dyn Screen) -> String {
        let Some(menu) = self.settings_menu.as_ref() else { return String::new() };
        match menu.row(index) {
            SettingsRow::Setting(setting) => self.toggle_setting(setting),
            SettingsRow::Launch(setting) => self.choose_launch_setting(setting, screen),
            SettingsRow::PairPhone => {
                self.settings_menu = None;
                self.start_pairing()
            }
            SettingsRow::Phone(_) => String::new(),
        }
    }

    fn choose_launch_setting(&mut self, index: usize, screen: &mut dyn Screen) -> String {
        let (name, label) = LAUNCH_SETTINGS[index];
        if name == "default-agent" {
            let Some(agent) = screen.outside(&mut || pick_agent()) else { return String::new() };
            if agents::find(&agent).is_none() { return format!("invalid default-agent: {agent}"); }
            let error = manager(&["setting", name, &agent]);
            return if error.is_empty() { format!("{label}: {agent}") } else { error };
        }
        let selected_model = if name == "opencode-effort" {
            match settings::launch_value("opencode-model") { Ok(model) => model, Err(error) => return error }
        } else { String::new() };
        if name == "opencode-effort" && selected_model.is_empty() { return "pick an OpenCode model before its effort".into(); }
        let menu = self.settings_menu.as_ref().expect("the settings menu is open");
        let Some(catalog) = &menu.catalog else {
            let viewed = current_session();
            let directory = self.sessions.iter().find(|session| session.session == viewed).map(|session| session.cwd.clone())
                .or_else(|| self.selected_session().map(|session| session.cwd)).filter(|directory| !directory.is_empty());
            let Some(directory) = directory.or_else(|| screen.outside(&mut || pick_project()).filter(|directory| !directory.is_empty())) else { return String::new() };
            let catalog = Arc::new(Mutex::new(None));
            let shared = catalog.clone();
            std::thread::spawn(move || {
                let result = settings::load_models(&directory);
                *shared.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
            });
            self.settings_menu.as_mut().expect("the settings menu is open").catalog = Some(catalog);
            return "loading OpenCode models … press Enter again when ready".into();
        };
        let result = catalog.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        let models = match result {
            None => return "loading OpenCode models … press Enter again when ready".into(),
            Some(Err(error)) => return format!("cannot load OpenCode models: {error}"),
            Some(Ok(models)) => models,
        };
        let mut choices = vec!["OpenCode default".to_string()];
        if name == "opencode-model" {
            choices.extend(models.iter().map(settings::ModelChoice::reference));
        } else {
            let Some(model) = models.iter().find(|model| model.reference() == selected_model) else {
                return format!("OpenCode model unavailable in this project: {selected_model}");
            };
            choices.extend(model.variants.iter().map(|variant| variant.id.clone()));
        }
        let Some(choice) = screen.outside(&mut || fzf(&format!("{label}> "), &choices, &[])) else { return String::new() };
        if !choices.contains(&choice) { return format!("invalid {label}: {choice}"); }
        let value = if choice == "OpenCode default" { "" } else { &choice };
        let error = manager(&["setting", name, value]);
        if error.is_empty() { format!("{label}: {choice}") } else { error }
    }

    /// The QR code of a new one-time pairing link, over the column.
    fn start_pairing(&mut self) -> String {
        match remote::pairing_link("") {
            Ok((link, digits)) => {
                self.pairing = Some(Pairing { modules: remote::qr_modules(&link), link, digits, phones_before: remote::phones().len(), paired: None });
                String::new()
            }
            Err(error) => format!("cannot pair a phone: {error}"),
        }
    }

    fn forget_phone(&mut self, phone: usize, screen: &mut dyn Screen) -> String {
        let Some(chosen) = self.settings_menu.as_ref().and_then(|menu| menu.phones.get(phone)).cloned() else { return String::new() };
        let name = chosen["name"].as_str().unwrap_or("").to_string();
        if !self.confirm(screen, &format!("Forget {name}? It needs a new pairing to come back.")) {
            return String::new();
        }
        let outcome = match remote::unpair(chosen["id"].as_str().unwrap_or("")) {
            Ok(name) => format!("{name} forgotten"),
            Err(error) => error,
        };
        if let Some(menu) = self.settings_menu.as_mut() {
            menu.phones = remote::phones();
            menu.selected = menu.selected.min(menu.row_count() - 1);
        }
        outcome
    }

    /// Keys and clicks while the settings menu is open; anything else closes it.
    fn handle_settings_menu(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let Some(menu) = self.settings_menu.as_mut() else { return String::new() };
        match key {
            Key::Click { x, y, .. } => {
                if let Some(index) = menu.rows.get(&y).copied() {
                    menu.selected = index;
                    return self.choose_settings_row(index, screen);
                }
                if !menu.contains(x, y) {
                    self.settings_menu = None;
                }
            }
            Key::Char('j') | Key::Down => menu.selected = (menu.selected + 1).min(menu.row_count() - 1),
            Key::Char('k') | Key::Up => menu.selected = menu.selected.saturating_sub(1),
            Key::Enter | Key::Char(' ') => {
                let index = menu.selected;
                return self.choose_settings_row(index, screen);
            }
            Key::Char('x') => {
                if let SettingsRow::Phone(phone) = menu.row(menu.selected) {
                    return self.forget_phone(phone, screen);
                }
            }
            Key::Char('d') => {
                if let SettingsRow::Launch(setting @ (1 | 2)) = menu.row(menu.selected) {
                    let (name, label) = LAUNCH_SETTINGS[setting];
                    let error = manager(&["setting", name, ""]);
                    return if error.is_empty() { format!("{label}: OpenCode default") } else { error };
                }
            }
            _ => self.settings_menu = None,
        }
        String::new()
    }

    /// Once the phone opened the pairing link, the overlay says so.
    fn watch_pairing(&mut self) {
        let Some(pairing) = self.pairing.as_mut().filter(|pairing| pairing.paired.is_none()) else { return };
        let phones = remote::phones();
        if phones.len() > pairing.phones_before {
            pairing.paired = phones.last().and_then(|phone| phone["name"].as_str()).map(str::to_string);
        }
    }

    // ---- Pull requests ----

    fn build_reviews(&mut self) {
        let (rows, error, loading) = self.reviews.current();
        self.review_rows = rows;
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

    pub(crate) fn handle_reviews(&mut self, key: Key, _screen: &mut dyn Screen) -> String {
        let row = self.review_rows.get(self.panels[2].selected).cloned();
        match key {
            Key::Enter => row.map(|row| open_in_scratchpad("prs", &row.url)).unwrap_or_default(),
            Key::Char('c') => match row {
                Some(row) => match self.reviews.checkouts.get(&row.full_repository) {
                    None => format!("no local checkout of {}: start a session there first", row.repository),
                    Some(checkout) => {
                        let agent = match settings::launch_agent() { Ok(agent) => agent, Err(error) => return error };
                        manager_in_background(&["--agent", &agent, "review", checkout, &row.number.to_string()]);
                        format!("preparing a review of {}#{} …", row.repository, row.number)
                    }
                },
                None => String::new(),
            },
            _ => String::new(),
        }
    }

    // ---- Todo ----

    fn build_todo(&mut self) {
        // Like the issues: the viewed session's project only, every project when none is viewed.
        let viewed = current_session();
        self.todo_scope = self.sessions.iter().find(|s| s.session == viewed).map(|s| s.project.clone()).unwrap_or_default();
        self.todo_tab = self.todo_tabs.get(&viewed).copied().unwrap_or(TODO_TAB_OPEN);
        let scope = self.todo_scope.clone();
        let mut entries: Vec<Entry> = self.todos.items.iter()
            .filter(|item| scope.is_empty() || item.project() == scope)
            .map(|item| Entry::Typed(item.clone()))
            .collect();
        for issue in self.reviews.issues() {
            let status = self.todos.status_of(&issue.key());
            let priority = todo::issue_priority(&issue.labels);
            entries.push(Entry::Issue { issue, status, priority });
        }
        entries.sort_by(todo::todo_order);
        for (index, (_, statuses)) in TODO_TABS.iter().enumerate() {
            self.todo_counts[index] = entries.iter().filter(|entry| statuses.contains(&entry.status().as_str())).count();
        }
        let shown_statuses = TODO_TABS[self.todo_tab].1;
        entries.retain(|entry| shown_statuses.contains(&entry.status().as_str()));
        let mut lines = Vec::new();
        // The tabs past Proposed hide proposals: without this line they wait unseen there.
        let proposed = self.todo_counts[TODO_TAB_PROPOSED];
        if proposed > 0 && !shown_statuses.contains(&todo::PROPOSED) {
            let ideas = if proposed == 1 { "1 idea from Claude waits".to_string() } else { format!("{proposed} ideas from Claude wait") };
            lines.push(Line::new(vec![segment(0, "?", bold(MAGENTA)), segment(2, format!("{ideas} for you · ] Proposed"), bold(MAGENTA))]));
        }
        if entries.is_empty() {
            let empty = match shown_statuses {
                [_, _, ..] => "Nothing open. ] next tab.",
                [todo::PROPOSED] => "No idea from Claude waiting for you. ] next tab.",
                ["doing"] => "Nothing in progress. ] next tab.",
                ["done"] => "Nothing done in the last 7 days.",
                _ => "Nothing to do. t adds an item for the viewed project; [ ] switch tabs.",
            };
            lines.push(Line::new(vec![segment(0, empty, dim(FG_DARK))]));
        }
        for (index, entry) in entries.iter().enumerate() {
            let status = entry.status();
            let done = status == "done";
            let mark = if done { String::new() } else { todo::priority_mark(entry.priority()) };
            let mark_style = if entry.priority() > todo::PRIORITY_NORMAL { bold(RED) } else { dim(FG_DARK) };
            let start = 2 + if mark.is_empty() { 0 } else { width_of(&mark) as u16 + 1 };
            let (glyph, glyph_style) = match status.as_str() {
                "doing" => ("◐", bold(BLUE)),
                todo::PROPOSED => ("?", bold(MAGENTA)),
                "done" => ("✓", dim(GREEN)),
                _ => ("•", bold(YELLOW)),
            };
            let faded = done.then(|| dim(FG_DARK));
            let queued = status == "todo" && self.todos.is_queued(&entry.key());
            let filed_by_claude = matches!(entry, Entry::Typed(item) if !item.source().is_empty());
            let mut right = if status == "doing" { vec![part("in progress", style(BLUE))] }
                        else if queued { vec![part("queued", bold(CYAN))] }
                        else if filed_by_claude { vec![part("by Claude", style(YELLOW))] } else { vec![] };
            if let Entry::Typed(item) = entry {
                if item.is_design() {
                    right.insert(0, part("✎ design", faded.unwrap_or(bold(MAGENTA))));
                }
                if item.is_blocked_report() {
                    right.insert(0, part(format!("⛔ {}", item.reporter()), faded.unwrap_or(bold(RED))));
                }
            }
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

    /// The project, then the tabs with their counts, the shown one in brackets.
    fn todo_subtitle(&self) -> String {
        let scope = if self.todo_scope.is_empty() { "all projects" } else { self.todo_scope.as_str() };
        let tabs: Vec<String> = TODO_TABS.iter().enumerate().map(|(index, (name, _))| {
            let count = self.todo_counts[index];
            if index == self.todo_tab { format!("[{name} {count}]") } else { format!("{name} {count}") }
        }).collect();
        let queued = self.todos.queued.len();
        let queue = if queued > 0 { format!(" · {queued} queued") } else { String::new() };
        format!("{scope} · {}{queue}", tabs.join(" · "))
    }

    fn show_todo_tab(&mut self, tab: usize) {
        self.todo_tab = tab;
        self.todo_tabs.insert(current_session(), tab);
    }

    fn switch_todo_tab(&mut self, step: isize) {
        self.show_todo_tab((self.todo_tab as isize + step).rem_euclid(TODO_TABS.len() as isize) as usize);
        self.panels[3].selected = 0;
        self.build_todo();
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
                let Some((title, notes)) = edit_in_popup(&format!("to do in {}", project_name(&directory)), "", "", "") else {
                    return "nothing added".into();
                };
                let project = viewed.map(|s| s.project).unwrap_or_else(|| project_name(&directory));
                let id = self.todos.add(&project, &directory, &title);
                if !notes.is_empty() {
                    self.todos.edit(&id, &title, &notes);
                }
                self.show_todo_tab(TODO_TAB_TO_DO);
                format!("added to {}", project_name(&directory))
            }
            Key::Char('[') => {
                self.switch_todo_tab(-1);
                String::new()
            }
            Key::Char(']') => {
                self.switch_todo_tab(1);
                String::new()
            }
            Key::Char('e') => match entry {
                Some(Entry::Typed(item)) => match edit_in_popup(&format!("edit · {}", item.project()), &item.text(), &item.notes(), &item.why()) {
                    Some((title, notes)) if self.todos.edit(&item.id(), &title, &notes) => "saved".into(),
                    _ => "unchanged".into(),
                },
                Some(Entry::Issue { .. }) => "an issue is edited on GitHub".into(),
                None => String::new(),
            },
            Key::Char('y') => match entry {
                Some(Entry::Typed(item)) if item.status() == todo::PROPOSED => {
                    self.todos.approve(&item.id());
                    "approved: it is in To do now".into()
                }
                _ => "y approves a proposal (Proposed tab)".into(),
            },
            Key::Char('n') => match entry {
                Some(Entry::Typed(item)) if item.status() == todo::PROPOSED => {
                    self.todos.decline(&item.id());
                    "declined: Claude will not propose it again".into()
                }
                _ => "n declines a proposal (Proposed tab)".into(),
            },
            Key::Char('h') => match entry {
                Some(entry) if entry.status() == todo::PROPOSED => "approve it first (y)".into(),
                Some(entry) => self.hand_to_claude(&entry, screen),
                None => String::new(),
            },
            Key::Char('N') => match entry {
                Some(entry) if entry.status() == todo::PROPOSED => "approve it first (y)".into(),
                Some(entry) => self.hand_to_new_session(&entry, screen),
                None => String::new(),
            },
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
                Some(entry) => {
                    show_in_popup(&todo_details(&entry));
                    String::new()
                }
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
    pub fn hand_to_claude(&mut self, entry: &Entry, _screen: &mut dyn Screen) -> String {
        let (directory, text, repository) = match entry {
            Entry::Typed(item) => (item.cwd(), todo::prompt_of(entry), String::new()),
            Entry::Issue { issue, .. } => (self.reviews.checkouts.get(&issue.full_repository).cloned().unwrap_or_default(), todo::prompt_of(entry), issue.repository.clone()),
        };
        if directory.is_empty() {
            return format!("no local checkout of {}: start a session there first", if repository.is_empty() { "?" } else { &repository });
        }
        let key = entry.key();
        let text = todo::with_todo_note(&text, &key);
        // A design to-do goes to the project's designer session, wherever its checkout is; any
        // other item to the session on screen when it works on the item's project (a parallel
        // clone's session lives in another folder than the one the item was filed in), else to a
        // session of its folder that is not the designer.
        let design_project = match entry {
            Entry::Typed(item) if item.is_design() => Some(item.project()),
            _ => None,
        };
        let is_design = design_project.is_some();
        let mut candidates = self.sessions.iter().filter(|session| session.state != "unknown" && match &design_project {
            Some(project) => session.role == "designer" && session.project == *project,
            None => session.cwd == directory && session.role != "designer",
        });
        let viewed = if is_design { None } else { self.viewed_session_for(entry) };
        let running = viewed.or_else(|| candidates.clone().find(|session| !session.is_external()).cloned());
        if running.is_none() && candidates.any(|session| session.is_external()) {
            return "external session: no terminal target; use its own window".into();
        };
        let text = if is_design && running.is_none() { format!("Load the figcraft-design skill first. {text}") } else { text };
        let outcome = match &running {
            Some(session) => {
                let error = manager(&["say", &session.session, &text]);
                if !error.is_empty() {
                    return error;
                }
                format!("sent to {}", session.project)
            }
            None => {
                let agent = match settings::launch_agent() { Ok(agent) => agent, Err(error) => return error };
                // Out of sight, as a card of its own (claude-sessions background): in the session pane
                // it replaced the session on screen, and the Todo panel lost its project.
                manager_in_background(&["--agent", &agent, "background", &directory, &text]);
                format!("starting a session in {}", project_name(&directory))
            }
        };
        self.todos.set_status(&key, "doing", running.as_ref().map(|s| s.session.as_str()).unwrap_or(""));
        outcome
    }

    /// The item to a new session in a parallel clone of its repository, on a branch named after it
    /// (claude-sessions parallel), never to a running one: it does not touch the files a session
    /// already works on. Then it is in progress.
    pub fn hand_to_new_session(&mut self, entry: &Entry, _screen: &mut dyn Screen) -> String {
        let directory = match entry {
            Entry::Typed(item) => item.cwd(),
            Entry::Issue { issue, .. } => self.reviews.checkouts.get(&issue.full_repository).cloned().unwrap_or_default(),
        };
        if directory.is_empty() {
            return "no local checkout of this issue's repository: clone it first".into();
        }
        let agent = match settings::launch_agent() { Ok(agent) => agent, Err(error) => return error };
        let key = entry.key();
        let (branch, text) = todo::handoff_of(entry);
        manager_in_background(&["--agent", &agent, "parallel", &directory, &branch, &text]);
        self.todos.set_status(&key, "doing", "");
        format!("starting a new session in a clone of {} (branch {branch})", project_name(&directory))
    }

    /// The session in the session pane, when it can take this item: running, not the designer, and
    /// on the item's project (by name, as the Todo list is narrowed) or the issue's repository.
    fn viewed_session_for(&self, entry: &Entry) -> Option<Session> {
        let viewed = current_session();
        let session = self.sessions.iter().find(|s| !s.is_external() && s.session == viewed && s.state != "unknown" && s.role != "designer")?;
        let belongs = match entry {
            Entry::Typed(item) => item.project() == session.project,
            Entry::Issue { issue, .. } => self.reviews.repositories.contains(&issue.full_repository),
        };
        belongs.then(|| session.clone())
    }

    // ---- The whole column ----

    fn panel_hints(&self, index: usize) -> Vec<(&'static str, &'static str)> {
        match index {
            0 => vec![],
            1 => self.sessions_hints(),
            2 => vec![("⏎", "open"), ("c", "review"), ("P", "list")],
            _ => self.todo_hints(),
        }
    }

    /// Each tab's keys, the one that moves its items along first.
    /// In the Open tab, the selected item's status picks them.
    pub(crate) fn todo_hints(&self) -> Vec<(&'static str, &'static str)> {
        let selected_status = self.todo_entries.get(self.panels[3].selected).map(Entry::status);
        let status = match TODO_TABS[self.todo_tab].1 {
            [only] => *only,
            _ => selected_status.as_deref().unwrap_or("todo"),
        };
        match status {
            todo::PROPOSED => vec![("y", "approve"), ("n", "decline"), ("⏎", "details"), ("e", "edit"), ("[ ]", "tabs")],
            "doing" => vec![("d", "done"), ("⏎", "details"), ("i", "back to to do"), ("e", "edit"), ("t", "add"), ("[ ]", "tabs")],
            "done" => vec![("⏎", "details"), ("d", "back to to do"), ("t", "add"), ("[ ]", "tabs")],
            _ => vec![("h", "to a session"), ("N", "to a new session"), ("⏎", "details"), ("Q", "queue"), ("i", "doing"), ("d", "done"), ("e", "edit"), ("t", "add"), ("[ ]", "tabs")],
        }
    }

    fn panel_help(index: usize) -> Vec<(&'static str, &'static str)> {
        match index {
            0 => vec![],
            1 => Self::sessions_help(),
            2 => vec![("⏎", "open the pull request in the pull requests window"),
                      ("c", "a session reviewing it, in its own clone of the repository, using the default agent"),
                      ("P", "the pull requests window on the viewed session's repository (or your review requests)"),
                      ("✓ ● ✗", "CI passing, running, failing"), ("approved", "review state: approved, changes (requested), review (required)")],
            _ => vec![("⏎", "the item's details: title, notes, why Claude proposed it, who filed it, when"),
                      ("[ ]", "previous / next tab: Open (Proposed, To do and In progress together) · Proposed · To do · In progress · Done"),
                      ("h", "hand the item over: the session on screen if it is on the item's project (a parallel clone too), Claude Code or opencode alike, else the project's running session, or a new one using the default agent"),
                      ("N", "hand the item to a new session using the default agent, in a parallel clone of its repository on a branch named after it (todo-<id>, issue-<n>), never to a running one"),
                      ("e", "edit the title and notes in your editor, in a popup (first line: the title)"),
                      ("y n", "approve or decline an idea Claude proposed; a declined idea is not proposed again"),
                      ("Q", "queue it (again: unqueue): a session of its project idle for 5 min at its prompt takes the first queued item, by priority, one at a time"),
                      ("t", "add an item for the viewed session's project (or pick a project), title and notes in your editor"),
                      ("i", "mark in progress (h does it too); i again: back to to do"),
                      ("d", "mark done, or back to to do; done items stay 7 days, dimmed at the bottom"),
                      ("+", "raise a typed item's priority: ! then !! up to !!!!"), ("-", "lower it"), ("x", "remove a typed item"),
                      ("! ↓", "raised and low priority; an issue's comes from its labels (P0/P1/urgent/high, P3/P4/low)"),
                      ("? • ◐ ✓", "proposed · to do · in progress · done; ◇ an open GitHub issue still to do"),
                      ("", "Claude marks items itself with claude-todo (start, done, add) and proposes ideas (propose)")],
        }
    }

    pub(crate) fn subtitle(&self, index: usize) -> String {
        match index {
            0 => self.status_subtitle(),
            1 => String::new(),
            2 => self.reviews_subtitle(),
            _ => self.todo_subtitle(),
        }
    }

    /// Pull requests with none open, Todo with nothing open: just their borders, unless focused.
    fn is_collapsed(&self, index: usize) -> bool {
        if index == self.focus {
            return false;
        }
        match index {
            2 => self.review_rows.is_empty() && self.reviews.current().1.is_empty(),
            3 => self.todo_counts[0] == 0,
            _ => false,
        }
    }

    /// Fixed panels keep their size, collapsed ones their borders; of the flexible ones, the others take
    /// no more rows than their lines need and at most a quarter of the rest, the focused one the remainder.
    pub fn layout(&self, height: u16) -> Vec<u16> {
        let status_height = self.panels[0].lines.len() as i64 + 2;
        let collapsed: Vec<usize> = [2usize, 3].into_iter().filter(|index| self.is_collapsed(*index)).collect();
        let flexible: Vec<usize> = [1usize, 2, 3].into_iter().filter(|index| !collapsed.contains(index)).collect();
        let available = height as i64 - HEADER_LINES as i64 - 1 - status_height - 2 * collapsed.len() as i64;
        let expanded = if flexible.contains(&self.focus) { self.focus } else { flexible[0] };
        let minimum = MIN_PANEL_LINES as i64;
        let others = minimum.max(available / 4);
        let mut heights = vec![status_height as u16, 2, 2, 2];
        for &index in &flexible {
            heights[index] = others.min(minimum.max(self.panels[index].lines.len() as i64 + 2)) as u16;
        }
        let taken: i64 = flexible.iter().filter(|index| **index != expanded).map(|index| heights[*index] as i64).sum();
        heights[expanded] = minimum.max(available - taken) as u16;
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
        if let Some(menu) = self.settings_menu.as_mut() {
            menu.draw(buffer);
        }
        if let Some(pairing) = self.pairing.as_ref() {
            pairing.draw(buffer);
        }
        if self.help_open {
            self.draw_help(buffer);
        }
        if !self.has_pane_focus {
            fade(buffer);
        }
    }

    fn draw_panel(&mut self, buffer: &mut Buffer, index: usize, top: u16, height: u16, width: u16) {
        if height < 2 || width < 4 {
            return;
        }
        let focused = self.has_pane_focus && index == self.focus;
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
            // The Todo tab shown: a chip over its "[name count]", readable at a glance.
            if index == 3 {
                if let (Some(open), Some(close)) = (subtitle.find('['), subtitle.find(']')) {
                    let column = 2 + width_of(&label) as u16 + 3 + width_of(&subtitle[..open]) as u16;
                    put(buffer, top, column, &format!(" {} ", &subtitle[open + 1..close]), on(BLACK, BLUE).add_modifier(Modifier::BOLD));
                }
                let proposed = format!("Proposed {}", self.todo_counts[TODO_TAB_PROPOSED]);
                if self.todo_counts[TODO_TAB_PROPOSED] > 0 && self.todo_tab != TODO_TAB_PROPOSED {
                    if let Some(start) = subtitle.find(&format!(" {proposed} ")) {
                        let column = 2 + width_of(&label) as u16 + 3 + width_of(&subtitle[..start]) as u16;
                        put(buffer, top, column, &format!(" {proposed} "), on(BLACK, MAGENTA).add_modifier(Modifier::BOLD));
                    }
                }
            }
        }
        for offset in 0..inner_height {
            let row = top + 1 + offset as u16;
            put(buffer, row, 0, "│", border);
            put(buffer, row, width - 2, "│", border);
            let line_index = panel.scroll + offset;
            let Some(line) = panel.lines.get(line_index) else { continue };
            let highlighted = focused && line.item.is_some() && line.item == Some(panel.selected);
            let recolor = |style: Style, fg: u8| -> Style {
                if !highlighted { return style; }
                let preserve_state = index == 1 && (style.fg == Some(color(RED)) || style.fg == Some(color(BLUE)));
                if preserve_state { style.bg(color(BG_HIGHLIGHT)) } else { style.fg(color(fg)).bg(color(BG_HIGHLIGHT)) }
            };
            if highlighted {
                put(buffer, row, 1, &" ".repeat(inner), on(FG, BG_HIGHLIGHT));
            }
            if let Some(accent) = line.accent {
                put(buffer, row, 1, "▌", recolor(accent, BLUE));
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
        if hidden_below > 0 && inner_height > 0 {
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
        let needing = self.sessions.iter().filter(|s| s.needs_attention()).count();
        let mut column = 2 + title.len() as u16 + 2;
        if needing > 0 {
            let text = format!("{needing} need you");
            put(buffer, 0, column, &text, bold(RED));
            column += text.len() as u16 + 2;
        }
        let proposed = self.todos.items.iter().filter(|item| item.status() == todo::PROPOSED).count();
        if proposed > 0 {
            let text = format!("? {proposed} idea{}", if proposed == 1 { "" } else { "s" });
            put(buffer, 0, column, &text, bold(MAGENTA));
            column += width_of(&text) as u16 + 2;
        }
        if !self.held.is_empty() {
            let text = format!("⛔ {}", self.held.len());
            put(buffer, 0, column, &text, bold(YELLOW));
            column += width_of(&text) as u16 + 2;
        }
        let stamp = chrono::Local::now().format("%H:%M  %a %d %b").to_string();
        put(buffer, 0, width.saturating_sub(2 + stamp.len() as u16).max(2), &stamp, style(FG_DARK));
        let cost = self.costs.totals().today;
        if cost > 0.0 {
            let amount = format!("≈ {}", money(cost));
            let label = " today";
            let start = width as i64 - 2 - stamp.len() as i64 - 3 - width_of(&amount) as i64 - label.len() as i64;
            if start > column as i64 {
                put(buffer, 0, start as u16, &amount, bold(MAGENTA));
                put(buffer, 0, start as u16 + width_of(&amount) as u16, label, dim(FG_DARK));
            }
        }
    }

    fn draw_footer(&self, buffer: &mut Buffer, row: u16, width: u16) {
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
            0 => String::new(),
            1 => self.handle_sessions(key, screen),
            2 => self.handle_reviews(key, screen),
            _ => self.handle_todo(key, screen),
        }
    }

    pub fn handle(&mut self, key: Key, screen: &mut dyn Screen) {
        if matches!(key, Key::FocusGained | Key::FocusLost) {
            self.has_pane_focus = key == Key::FocusGained;
            return;
        }
        self.message.clear();
        if self.menu.is_some() {
            self.message = self.handle_menu(key, screen);
            return;
        }
        if self.settings_menu.is_some() {
            self.message = self.handle_settings_menu(key, screen);
            return;
        }
        if self.pairing.is_some() {
            self.pairing = None;
            return;
        }
        if self.help_open {
            self.help_open = false;
            return;
        }
        if let Key::F(number @ 1..=3) = key {
            self.focus_panel(number as usize); // F1..F3 from the tmux Ctrl+digit binds
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
            Key::Char('?') => self.help_open = true,
            Key::Char(':') => {
                let entries = self.palette_entries();
                if let Some(choice) = screen.outside(&mut || pick_action(&entries)) {
                    if let Some((panel, key)) = choice.split_once('\t') {
                        if let Ok(panel) = panel.parse::<usize>() {
                            self.focus_panel(panel);
                        }
                        if PALETTE_ONLY.iter().any(|(_, name, _)| *name == key) {
                            self.message = self.run_palette_only(key, screen);
                        } else {
                            let key = if key == "⏎" { Key::Enter } else { key.chars().next().map(Key::Char).unwrap_or(Key::Esc) };
                            self.handle(key, screen);
                        }
                    }
                }
            }
            Key::Char('a') => {
                if let Some(directory) = screen.outside(&mut || pick_project()).filter(|d| !d.is_empty()) {
                    self.message = match settings::launch_agent() {
                        Ok(agent) => self.manager_async(&["--agent", &agent, "new", &directory]),
                        Err(error) => error,
                    };
                }
            }
            Key::Char('O') => {
                if let Some(directory) = screen.outside(&mut || pick_project()).filter(|d| !d.is_empty()) {
                    self.message = self.manager_async(&["--agent", "opencode", "new", &directory]);
                }
            }
            Key::Char('S') => self.settings_menu = Some(SettingsMenu::new()),
            Key::Char('H') => {
                let past = self.past_sessions();
                if let Some(index) = screen.outside(&mut || pick_past_session(&past)).and_then(|chosen| chosen.parse::<usize>().ok()) {
                    let chosen = &past[index].0;
                    self.message = self.manager_async(&["resume", &chosen.cwd, &chosen.session]);
                }
            }
            Key::Char('P') => self.message = self.manager_async(&["prs"]),
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

    /// The design action of the : list: the canvas link, then the project; the session links its card and watches the comments.
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
        let is_external = self.selected_session().is_some_and(|session| session.is_external());
        let mut entries: Vec<String> = GLOBAL_HELP.iter()
            .filter(|(key, _)| key.chars().count() == 1 && !":?".contains(*key))
            .map(|(key, label)| format!("-\t{key}\t{:<13} {key:<2} {label}", "everywhere"))
            .collect();
        for index in 0..4 {
            for (key, label) in Self::panel_help(index) {
                if index == 1 && is_external
                    && session_key_requires_terminal(if key == "⏎" { Key::Enter } else { Key::Char(key.chars().next().unwrap()) }) {
                    continue;
                }
                if key.chars().count() == 1 || key == "⏎" {
                    entries.push(format!("{index}\t{key}\t{:<13} {key:<2} {label}", PANEL_NAMES[index]));
                }
            }
        }
        for (panel, name, label) in PALETTE_ONLY {
            if name == "remote" && is_external {
                continue;
            }
            let scope = panel.parse::<usize>().map_or("everywhere", |index| PANEL_NAMES[index]);
            entries.push(format!("{panel}\t{name}\t{scope:<13}    {label}"));
        }
        entries
    }

    fn run_palette_only(&mut self, name: &str, screen: &mut dyn Screen) -> String {
        let known = self.selected_session().filter(|s| s.state != "unknown");
        match name {
            "new-with-agent" => {
                let Some(directory) = screen.outside(&mut || pick_project()).filter(|directory| !directory.is_empty()) else { return String::new() };
                let Some(agent) = screen.outside(&mut || pick_agent()) else { return String::new() };
                self.manager_async(&["--agent", &agent, "new", &directory])
            }
            "digest" => self.manager_async(&["digest"]),
            "pair-phone" => self.start_pairing(),
            "design" => self.start_design_session(screen),
            "clones" => {
                manager_in_background(&["clones"]);
                String::new()
            }
            "status-page" => {
                let _ = Command::new("xdg-open").arg(status::PAGE).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
                "opening status.claude.com …".into()
            }
            "remote" => match known {
                Some(s) if s.is_external() => "external session: no terminal target; use its own window".into(),
                Some(s) if !s.agent().can(Feature::RemoteControl) => format!("Remote Control is not {}'s to do: {} runs {}", s.agent().label, s.project, s.agent().name),
                Some(s) => {
                    manager_in_background(&["remote", &s.session]);
                    format!("toggling remote control for {} …", s.project)
                }
                None => String::new(),
            },
            "drop-queued" => match known {
                Some(s) if prompts::count(&s.session) == 0 => "no prompt queued for it".into(),
                Some(s) if self.confirm(screen, &format!("drop the {} prompt(s) queued for {}?", prompts::count(&s.session), s.project)) => {
                    prompts::clear(&s.session);
                    "queued prompts dropped".into()
                }
                _ => String::new(),
            },
            _ => panic!("no palette action named {name}"),
        }
    }

    /// Past sessions not running now, newest first, with their titles.
    /// Claude's past sessions and opencode's, newest first, the running ones left out.
    pub(crate) fn past_sessions(&mut self) -> Vec<(PastSession, String)> {
        self.history.refresh();
        let running: Vec<&str> = self.sessions.iter().map(|s| s.session.as_str()).collect();
        let mut past: Vec<(PastSession, String)> = self.history.entries.iter()
            .map(|entry| (entry.clone(), self.history.title(&entry.session))).collect();
        match sessions::opencode_past_sessions(&paths::opencode_database()) {
            Ok(opencode) => past.extend(opencode),
            Err(error) => util::log_event("ERROR", &format!("H: opencode's past sessions left out ({}): {error}", paths::opencode_database().display())),
        }
        past.retain(|(entry, _)| !running.contains(&entry.session.as_str()));
        past.sort_by(|a, b| b.0.since.cmp(&a.0.since));
        past
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
/// A session outside tmux (exit_session handles the others): its terminal window, or its card.
fn kill_session(session: &Session) -> String {
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

fn session_key_requires_terminal(key: Key) -> bool {
    matches!(key, Key::Enter | Key::Char('r' | 'e' | '$' | 'C' | 'x'))
}

/// The card keys that need a feature of the session's agent, and what they do, named for a
/// refusal (claude-sessions refuses them too, and Remote Control).
pub(crate) fn key_feature(key: Key) -> Option<(Feature, &'static str)> {
    match key {
        Key::Char('e') => Some((Feature::Effort, "the effort level")),
        Key::Char('$') => Some((Feature::Billing, "billing to API credits")),
        Key::Char('C') => Some((Feature::Compact, "/compact")),
        _ => None,
    }
}

/// The claude-sessions command in `arguments`, past a leading --agent <name>.
fn command_name<'a>(arguments: &[&'a str]) -> &'a str {
    match arguments {
        ["--agent", _, command, ..] => command,
        _ => arguments[0],
    }
}

fn pick_project() -> Option<String> {
    fzf("project> ", &sessions::project_choices(), &[])
}

/// The agent a new session runs (agents::AGENTS); None when the pick is cancelled, and then
/// nothing starts.
fn pick_agent() -> Option<String> {
    let names: Vec<String> = agents::names().into_iter().map(String::from).collect();
    fzf("agent> ", &names, &[]).filter(|agent| agents::find(agent).is_some())
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
        let badge = agents::of_session_id(&entry.session).and_then(|agent| agent.badge).map(|badge| format!("{badge}  ")).unwrap_or_default();
        format!("{index}\t{when}  {:<18} {badge}{}", entry.project, if title.is_empty() { &entry.prompt } else { title })
    }).collect();
    let chosen = fzf("resume> ", &lines, &["--delimiter", "\t", "--with-nth", "2.."])?;
    chosen.split('\t').next().map(str::to_string)
}

/// A line typed in the plain terminal, outside the column's screen.
/// The editor's file: the title on the first line, the notes below, then help (Claude's reason
/// for a proposal included) that is left out when read back.
pub(crate) fn editor_text(title: &str, notes: &str, why: &str) -> String {
    let mut text = format!("{title}\n\n{notes}\n\n{EDITOR_HELP_MARK}\n# First line: the title. Below it: the notes, as long as needed.\n# Save and quit to keep it; an empty title cancels.\n");
    if !why.is_empty() {
        text.push_str(&format!("# Why Claude proposed it:\n# {}\n", why.replace('\n', "\n# ")));
    }
    text
}

/// Title and notes back from the editor's file; None for an empty title.
pub(crate) fn parse_editor_text(text: &str) -> Option<(String, String)> {
    let body = text.split(EDITOR_HELP_MARK).next().unwrap_or("");
    let mut lines = body.lines().skip_while(|line| line.trim().is_empty());
    let title = lines.next()?.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let notes = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    Some((title, notes))
}

/// Theo's editor ($EDITOR, else nvim) in a tmux popup over the cockpit, on a file of the runtime
/// folder; the column waits for it to close.
fn edit_in_popup(heading: &str, title: &str, notes: &str, why: &str) -> Option<(String, String)> {
    let path = crate::paths::runtime_dir().join(format!("claude-todo-{}.md", util::token_hex(4)));
    fs::write(&path, editor_text(title, notes, why)).ok()?;
    let _ = Command::new("tmux").args(["display-popup", "-E", "-w", "80%", "-h", "70%", "-T", &format!(" {heading} · save and quit keeps it ")])
        .args(["sh", "-c", "exec \"${EDITOR:-nvim}\" \"$1\"", "sh"]).arg(&path).status();
    let text = fs::read_to_string(&path).unwrap_or_default();
    let _ = fs::remove_file(&path);
    parse_editor_text(&text)
}

/// Read-only text in a tmux popup (less: q closes it).
fn show_in_popup(text: &str) {
    let path = crate::paths::runtime_dir().join(format!("claude-todo-{}.txt", util::token_hex(4)));
    if fs::write(&path, text).is_err() {
        return;
    }
    let _ = Command::new("tmux").args(["display-popup", "-E", "-w", "70%", "-h", "60%", "-T", " todo · q closes "])
        // No --quit-if-one-screen: a short text would close the popup the moment it opened.
        .args(["less", "-R", "--no-init"]).arg(&path).status();
    let _ = fs::remove_file(&path);
}

/// What Enter shows for an item or an issue.
pub(crate) fn todo_details(entry: &Entry) -> String {
    match entry {
        Entry::Typed(item) => {
            let status = match item.status().as_str() { todo::PROPOSED => "proposed by Claude", "doing" => "in progress", "done" => "done", _ => "to do" };
            let created = chrono::DateTime::from_timestamp(item.created(), 0).map(|time| time.with_timezone(&chrono::Local).format("%d/%m %H:%M").to_string()).unwrap_or_default();
            let filed = if item.source().is_empty() { "you".to_string() } else { format!("Claude ({})", item.source()) };
            let mut text = format!("{}\n\n{} · {status} · filed by {filed} on {created}\n", item.text(), item.project());
            if !item.why().is_empty() {
                text.push_str(&format!("\nWhy Claude proposed it\n{}\n", item.why()));
            }
            let notes = item.notes();
            text.push_str(&format!("\nNotes\n{}\n", if notes.is_empty() { "(none: e adds some)" } else { notes.as_str() }));
            text.push_str(if item.status() == todo::PROPOSED { "\nIn the column: y approve · n decline · e edit\n" } else { "\nIn the column: e edit · h hand to a session · d done\n" });
            text
        }
        Entry::Issue { issue, status, .. } => format!("{}#{} {}\n\n{status} · labels: {}\n\n{}\n", issue.repository, issue.number, issue.title,
                                                       issue.labels.join(", "), issue.url),
    }
}

/// What a key does to a one-line prompt.
#[derive(Debug, PartialEq)]
pub(crate) enum LineEdit {
    Typing,
    Done,
    Cancelled,
}

/// One key on the prompt's line: Enter sends it, Escape and Ctrl+C cancel it.
pub(crate) fn edit_line(line: &mut String, code: KeyCode, modifiers: KeyModifiers) -> LineEdit {
    match code {
        KeyCode::Enter => LineEdit::Done,
        KeyCode::Esc => LineEdit::Cancelled,
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => LineEdit::Cancelled,
        KeyCode::Backspace => {
            line.pop();
            LineEdit::Typing
        }
        KeyCode::Char(typed) if !modifiers.contains(KeyModifiers::CONTROL) => {
            line.push(typed);
            LineEdit::Typing
        }
        _ => LineEdit::Typing,
    }
}

/// A one-line prompt under the column, read key by key in raw mode: in the terminal's own line
/// mode a Ctrl+C killed the whole column ("Pane is dead (signal 2)").
fn input_line(screen: &mut dyn Screen, prompt: &str) -> String {
    screen.outside(&mut || {
        let _ = crossterm::terminal::enable_raw_mode();
        let mut line = String::new();
        let outcome = loop {
            print!("\r\x1b[2K  {prompt}{line}");
            let _ = io::stdout().flush();
            let step = match event::read() {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => edit_line(&mut line, key.code, key.modifiers),
                Ok(Event::Paste(pasted)) => {
                    line.push_str(&pasted.replace(['\n', '\r'], " "));
                    LineEdit::Typing
                }
                Ok(_) => LineEdit::Typing,
                Err(_) => LineEdit::Cancelled,
            };
            if step != LineEdit::Typing {
                break step;
            }
        };
        let _ = crossterm::terminal::disable_raw_mode();
        (outcome == LineEdit::Done).then(|| line.trim().to_string())
    }).unwrap_or_default()
}

/// The column out of focus: every cell dim, and the filled ones (chips, highlights) on the grey
/// highlight. tmux's window-style cannot do it, it only reaches cells in the default colours.
fn fade(buffer: &mut Buffer) {
    for cell in buffer.content.iter_mut() {
        cell.modifier.insert(Modifier::DIM);
        cell.modifier.remove(Modifier::BOLD);
        if cell.bg != Color::Reset {
            cell.bg = color(BG_HIGHLIGHT);
        }
    }
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
            Event::FocusGained => return Some(Key::FocusGained),
            Event::FocusLost => return Some(Key::FocusLost),
            Event::Resize(..) => return None,
            _ => continue,
        }
    }
}

pub const POLL_INTERVAL: Duration = POLL;
/// How often the column looks whether another session went into the session pane (Ctrl+Tab, a
/// banner, n): its Todo and Pull requests follow within this, not at the next full tick.
pub const VIEW_CHECK_INTERVAL: Duration = Duration::from_millis(40);

impl App {
    pub fn viewed_changed(&self) -> bool {
        current_session() != self.shown_session
    }
}

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
