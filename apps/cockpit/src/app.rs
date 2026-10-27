//! The action panels (Sessions, Pull requests, Todo, Ideas), on-demand Status, the header and footer,
//! the effort and settings menus, the help, and every key.

use crate::agents::{self, Feature};
use crate::continuity::Continuity;
mod continuity_ui;
mod new_session;
mod chief_ui;
pub mod refresh;
use crate::costs::Costs;
use crate::errors::ErrorReporter;
use crate::paths;
use crate::reviews::{self, Review, Reviews};
use crate::sessions::{self, CONTEXT_COMPACT_AT, History, PastSession, Restorable, Session, ViewState};
use crate::settings::{self, LAUNCH_SETTINGS, SETTINGS};
use crate::status::{self, Provider, ProviderStatus};
use crate::todo::{self, Entry, TodoList};
use crate::todo_composer::{TodoComposer, Outcome};
use crate::usage::{self, BudgetAlerts, Refresher, Window};
use crate::prompts;
use crate::queue::{self, QUEUE_CHECK_S};
use crate::sleep;
use crate::{approval, remote};
use crate::util::{self, age, current_session, manager, manager_in_background, money, now, project_name, start_directory};
use serde_json::Value;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;
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
// No YELLOW: amber means "needs you" and nothing else.
const PROJECT_COLORS: [u8; 4] = [MAGENTA, CYAN, GREEN, BLUE];
const HEADER_LINES: u16 = 1;
const MIN_PANEL_LINES: u16 = 4;
const MIN_SESSION_TITLE_WIDTH: usize = 8;
const POLL: Duration = Duration::from_millis(250);
/// How long a session that exited cleanly may stay in the snapshot before its card shows again.
const EXIT_SETTLE_S: f64 = 15.0;
/// How long a restarting session's card is held without a report from its new process.
const RESTART_TIMEOUT_S: i64 = 90;
const DEFAULT_COLUMN_WIDTH: u16 = 60;
const TITLES_PER_TICK: usize = 8;
const GREYPROXY_POLL_S: f64 = 10.0;
/// The same limit as vitals' disk warning (apps/vitals/src/checks.rs).
const DISK_WARNING_PERCENT: f64 = 85.0;
const DISK_CHECK_S: f64 = 60.0;
const RESTORE_CHECK_S: f64 = 30.0;
const TIME_LABEL: &str = "Active time";
/// The Todo panel's tabs: the statuses each one shows. Open merges everything not done yet.
// Proposals live in the Ideas tab: Open shows only work Theo agreed to.
const TODO_TABS: [(&str, &[&str]); 5] = [("Open", &["todo", "doing"]), ("Proposed", &[todo::PROPOSED]), ("To do", &["todo"]),
                                         ("In progress", &["doing"]), ("Done", &["done"])];
const TODO_TAB_OPEN: usize = 0;
const TODO_TAB_PROPOSED: usize = 1;
const TODO_TAB_TO_DO: usize = 2;
const TODO_TAB_IN_PROGRESS: usize = 3;
const EDITOR_HELP_MARK: &str = "#── everything from this line on is left out ──";
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const PANEL_NAMES: [&str; 5] = ["Status", "Sessions", "Pull requests", "Todo", "Ideas"];
/// The tabs' panels, in the order of their keys 1, 2, 3.
const TAB_PANELS: [usize; 3] = [1, 3, 4];

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusAction { ClaudeStatus, CodexStatus, Digest, Clones }

impl StatusAction {
    fn hint(self) -> &'static str {
        match self {
            Self::ClaudeStatus => "Claude status page", Self::CodexStatus => "Codex status page",
            Self::Digest => "daily digest", Self::Clones => "clones review",
        }
    }
}
const GLOBAL_HINTS: [(&str, &str); 2] = [("s", "status"), ("?", "help")];
/// ⌘ in key hints: the physical Command key, which keyd turns into Ctrl (system/etc/keyd).
const GLOBAL_HELP: &[(&str, &str)] = &[
    ("1 2 3", "Sessions · Todo · Ideas"), ("s", "Status: usage, spending, Chief, PRs, phone"),
    ("?", "this help (for the tab you're on)"), ("esc", "close / back"),
    ("N", "new session: blank or Claude Design canvas"),
    ("H", "resume a past session, searched by project and prompt"),
    ("S", "settings: default agent, models, on/off choices, phone pairing"),
    ("P W G", "windows on top: pull requests, Claude on the web, greyproxy"),
    ("⌘1 ⌘2 ⌘3", "a tab from inside the session"), ("→ ←", "into the session and back"),
    ("q", "close the column; sessions keep running"),
];
/// What the signs on a session card mean: Sessions help's last section.
const LEGEND: &[(&str, &str)] = &[
    ("!", "needs you"), ("◐", "working"), ("✓", "finished"), ("☾", "asleep"), ("↳", "waits on a todo"),
    ("⇄", "linked"), ("◆", "the Chief"), ("▶", "your approved command runs"), ("‖", "later: its ask is hidden"),
    ("●", "finished with a result you haven't opened"), ("○", "idle: waiting for your next message"),
    ("⟳", "restart needed (or restarting)"), ("⚲", "pinned"), ("↑2", "unpushed commits"), ("✉2", "queued messages"),
    ("⬆", "update available"), ("$", "billed to API credits"), ("·", "state unknown: no report"),
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

/// How many answers the Chief gave a session, and its newest few for the details.
struct ChiefAnswers {
    count: usize,
    recent: Vec<Value>,
}

/// The cells right of a session card's title that its badges may use, at least a short title being kept.
fn session_card_right_room(width: u16, title_column: usize) -> usize {
    (width as usize).saturating_sub(4 + title_column + MIN_SESSION_TITLE_WIDTH + 2)
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
    /// A bar painted from this column to the right edge (the amber "needs you" ask); its texts keep
    /// their own colours on a selected card.
    pub fill: Option<(u16, Style)>,
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

/// Where a session stands for Theo: the session list's groups, in their order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Attention { NeedsYou, Working, Quiet }

impl Attention {
    fn heading(self) -> &'static str {
        match self { Attention::NeedsYou => "NEEDS YOU", Attention::Working => "WORKING", Attention::Quiet => "QUIET" }
    }
}

/// One session card's content: line 1 (glyph, doing now, age), and the ask bar when it needs Theo.
struct Card {
    group: Attention,
    glyph: &'static str,
    tint: u8,
    doing: String,
    age: String,
    stale: bool,
    /// Bright text in QUIET: a result Theo has not opened yet.
    bright: bool,
    ask: Option<(String, &'static str)>,
    ask_count: String,
}

/// A line under the tabs about the machine rather than one session. Its coloured left edge is
/// never amber: amber means "needs you". Status lists every notice with its detail.
#[derive(Clone)]
struct Notice {
    glyph: &'static str,
    tint: u8,
    text: String,
    key: &'static str,
    detail: String,
}

/// Notices the Sessions tab shows; the others wait behind "+N more · s for Status".
const NOTICES_SHOWN: usize = 2;
/// Badges a card's first line holds at most, the most urgent first.
const CARD_BADGES_SHOWN: usize = 2;

fn notice_line(notice: &Notice) -> Line {
    let right = if notice.key.is_empty() { Vec::new() } else { vec![part(notice.key, dim(FG_DARK))] };
    Line { segments: vec![segment(0, notice.glyph, bold(notice.tint)), segment(2, notice.text.clone(), style(FG))],
           right, accent: Some(bold(notice.tint)), ..Line::default() }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Requests greyproxy holds, as a NEEDS YOU card: only Theo can allow or deny them. Its entries
/// read "<sandbox> → host:port" (sessions::greyproxy_held).
fn greyproxy_card(held: &[String], since: i64) -> Vec<Line> {
    let accent = Some(bold(YELLOW));
    let sandboxes: BTreeSet<&str> = held.iter().map(|entry| entry.split_once(" → ").map_or("", |(sandbox, _)| sandbox)).collect();
    let why = match sandboxes.iter().next() {
        Some(sandbox) if sandboxes.len() == 1 && !sandbox.is_empty() => {
            let destinations: Vec<&str> = held.iter().filter_map(|entry| entry.split_once(" → ").map(|(_, destination)| destination)).collect();
            why_segments(sandbox, &destinations.join(", "))
        },
        _ => why_segments("greyproxy", &held.join(", ")),
    };
    vec![
        Line { segments: vec![segment(0, "!", bold(YELLOW)), segment(2, format!("greyproxy held {} request{}", held.len(), plural(held.len())), bold(FG))],
               right: vec![part(util::age(since), dim(FG_DARK))], accent, ..Line::default() },
        Line { segments: why, accent, ..Line::default() },
        ask_bar(None, "Allow or deny in greyproxy", "", "G review"),
        Line::default(),
    ]
}

/// Line 2 of a card: `project › task`, the task cut first, never the project.
fn why_segments(project: &str, task: &str) -> Vec<(u16, String, Style)> {
    let project = crate::teams::sidebar::shown(project);
    let column = 2 + width_of(&project) as u16;
    let mut segments = vec![segment(2, project.clone(), bold(project_color(&project)))];
    if !task.is_empty() && task != project {
        segments.push(segment(column, " › ", dim(FG_DARK)));
        segments.push(segment(column + 3, task, style(FG_DARK)));
    }
    segments
}

/// Line 3 of a card that needs Theo: the exact ask on an amber bar, with the key that acts on it.
fn ask_bar(item: Option<usize>, ask: &str, count: &str, key_hint: &str) -> Line {
    let bar = on(BLACK, YELLOW);
    let text = if count.is_empty() { ask.to_string() } else { format!("{ask}   {count}") };
    Line { segments: vec![segment(2, format!(" {text}"), bar.add_modifier(Modifier::BOLD))],
           right: vec![part(format!("{key_hint} "), bar)], item, accent: Some(bold(YELLOW)), fill: Some((2, bar)) }
}

#[derive(Default)]
pub struct PanelState {
    pub lines: Vec<Line>,
    pub fixed_top: Vec<Line>,
    pub fixed_bottom: Vec<Line>,
    pub items: usize,
    pub selected: usize,
    pub scroll: usize,
}

enum TeamLinks { Loading, Ready(Value), Unavailable }

impl PanelState {
    fn content_height(&self) -> usize {
        self.fixed_top.len() + self.lines.len() + self.fixed_bottom.len()
    }

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
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Char(char),
    /// Bracketed paste characters must never become panel or form shortcuts.
    Paste(String),
    Ctrl(char),
    Enter,
    Tab,
    BackTab,
    Esc,
    Backspace,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    F(u8),
    Click { x: u16, y: u16, double: bool },
    /// tmux moved its focus into the column or out of it (focus-events).
    FocusGained,
    FocusLost,
    Resize,
}

/// Keep the inspected text stable even if the source session changes or disappears.
#[derive(Default)]
struct SessionDetails {
    title: String,
    text: String,
    scroll: usize,
    actions: Vec<DetailAction>,
    /// The session whose Chief answers `c` opens from here; set only when it has some.
    chief_answers_of: Option<String>,
    /// Session details (`i`): the session its keys act on, and those keys by group, drawn as key caps.
    session: Option<String>,
    todo: Option<String>,
    displayed_updates: Vec<String>,
    key_groups: Vec<(&'static str, Vec<(&'static str, &'static str)>)>,
}

/// Session details' keys, grouped as frame 2200 draws them; they work on the selected card too.
const SESSION_DETAIL_KEYS: [(&str, &[(&str, &str)]); 3] = [
    ("WORK", &[("e", "effort"), ("$", "billing: subscription / API"), ("C", "/compact"), ("c", "Chief answers"), ("k", "links"), ("d", "drop queued"), ("u", "updates"), ("Q", "queue explanations"), ("v", "mark displayed updates reviewed")]),
    ("CODE", &[("g", "lazygit"), ("U", "push / apply"), ("+", "parallel clone"), ("o", "design canvas"), ("t", "task trail")]),
    ("SESSION", &[("m", "remote control"), ("w", "my note"), ("z", "sleep"), ("R", "restart"), ("x", "end")]),
];

enum DetailRow {
    Heading(String),
    Text(String),
    Keys(Vec<(&'static str, &'static str)>),
}

fn is_detail_heading(line: &str) -> bool {
    let heading = line.split(" (").next().unwrap_or(line);
    heading.len() > 2 && heading.chars().all(|character| character.is_ascii_uppercase() || " &".contains(character))
}

#[derive(Clone)]
enum DetailAction {
    Reviewed { session: String, identities: Vec<String> },
    OpenSession(String),
    OpenTask(String),
    OpenPr(String),
    ResumeSession { session: String, cwd: String },
}

impl SessionDetails {
    fn draw(&mut self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        if width < 8 || height < 6 { return; }
        let top = HEADER_LINES;
        let bottom = height - 2;
        let inner = width.saturating_sub(4) as usize;
        let body_width = inner.saturating_sub(2).max(2);
        let body_height = (bottom.saturating_sub(top + 3) as usize).max(1);
        let rows = self.rows(body_width);
        self.scroll = self.scroll.min(rows.len().saturating_sub(body_height));
        for row in top..=bottom {
            put(buffer, row, 1, &" ".repeat((width - 2) as usize), on(FG, BLACK));
            put(buffer, row, 1, "│", style(BLUE));
            put(buffer, row, width - 2, "│", style(BLUE));
        }
        put(buffer, top, 1, &format!("┌{}┐", "─".repeat(inner)), style(BLUE));
        put(buffer, top, 3, &fit(&self.title, inner.saturating_sub(2)), bold(BLUE));
        for (offset, line) in rows.iter().skip(self.scroll).take(body_height).enumerate() {
            let row = top + 1 + offset as u16;
            match line {
                DetailRow::Heading(text) => { put(buffer, row, 3, text, bold(FG_DARK)); }
                DetailRow::Text(text) => { put(buffer, row, 3, text, style(FG)); }
                DetailRow::Keys(keys) => {
                    let mut column = 3;
                    for (key, label) in keys {
                        put(buffer, row, column, &format!(" {key} "), on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD));
                        column += width_of(key) as u16 + 3;
                        put(buffer, row, column, label, style(FG_DARK));
                        column += width_of(label) as u16 + 2;
                    }
                }
            }
        }
        let progress = format!("{}–{} / {}", self.scroll + 1, (self.scroll + body_height).min(rows.len()), rows.len());
        if bottom - 2 > top + body_height as u16 {
            put(buffer, bottom - 2, 3, &fit(&progress, body_width), dim(FG_DARK));
        }
        let footer = if self.session.is_some() { "esc back · the same keys work on the selected card without opening i" }
            else if self.actions.is_empty() { "↑↓ scroll  g/G top/end  Esc close" } else { "↑↓ scroll  Enter actions  Esc close" };
        put(buffer, bottom - 1, 3, &fit(footer, body_width), dim(FG_DARK));
        put(buffer, bottom, 1, &format!("└{}┘", "─".repeat(inner)), style(BLUE));
    }

    /// The text wrapped to `width` (capitalised lines become headings), then each key group
    /// flowed into as few rows of key caps as fit.
    fn rows(&self, width: usize) -> Vec<DetailRow> {
        let mut rows: Vec<DetailRow> = wrap_detail_text(&self.text, width).into_iter()
            .map(|line| if is_detail_heading(&line) { DetailRow::Heading(line) } else { DetailRow::Text(line) }).collect();
        for (heading, keys) in &self.key_groups {
            rows.push(DetailRow::Heading((*heading).into()));
            let mut current: Vec<(&str, &str)> = Vec::new();
            let mut used = 0;
            for &(key, label) in keys {
                let cell = width_of(key) + 3 + width_of(label) + 2;
                if !current.is_empty() && used + cell > width {
                    rows.push(DetailRow::Keys(std::mem::take(&mut current)));
                    used = 0;
                }
                current.push((key, label));
                used += cell;
            }
            if !current.is_empty() { rows.push(DetailRow::Keys(current)); }
        }
        rows
    }
}

fn wrap_detail_text(text: &str, width: usize) -> Vec<String> {
    let mut wrapped = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            wrapped.push(String::new());
            continue;
        }
        for line in approval::wrap(paragraph, width) {
            let mut piece = String::new();
            for character in line.chars() {
                let mut candidate = piece.clone();
                candidate.push(character);
                if !piece.is_empty() && width_of(&candidate) > width {
                    wrapped.push(std::mem::take(&mut piece));
                }
                piece.push(character);
            }
            wrapped.push(piece);
        }
    }
    wrapped
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
                        Ok(value) if value.is_empty() && name == "idle-sleep-after" => (sleep::DEFAULT_AFTER_MIN.to_string(), FG_DARK),
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
            SettingsRow::Launch(3) => "⏎ next delay",
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

/// A session told to exit, and the `cockpit exit` doing it.
pub(crate) struct Exiting {
    pub(crate) session: String,
    tmux_pane: String,
    project: String,
    child: std::process::Child,
    ended_at: Option<f64>,
}

/// A saved draft stays visible without moving the session pane or borrowing its preferences.
struct SavedTodoScope {
    viewed_session: String,
    owner_session: String,
    project: String,
}

pub struct App {
    pub(crate) continuity: Result<Continuity, String>,
    continuity_error: Option<String>,
    pub history: History,
    pub view: ViewState,
    pub(crate) usage: Refresher,
    pub(crate) alerts: BudgetAlerts,
    pub costs: Costs,
    pub reviews: Reviews,
    pub todos: TodoList,
    pub errors: ErrorReporter,
    pub claude_status: ProviderStatus,
    pub codex_status: ProviderStatus,
    pub panels: [PanelState; 5],
    pub focus: usize,
    previous_panel: usize,
    pub help_open: bool,
    status_actions: Vec<Option<StatusAction>>,
    status_selection_lost: bool,
    help_scroll: usize,
    /// Whether tmux's focus is in the column: out of it, nothing there is drawn as focused.
    pub has_pane_focus: bool,
    pub message: String,
    /// Sessions told to exit (x): off the list at once, back with a warning if the exit fails.
    pub(crate) exiting: Vec<Exiting>,
    /// The cards of the sessions being restarted, as last drawn: held until each reports again.
    restarting_cards: HashMap<String, Session>,
    pub(crate) labelled: (String, String),
    pub(crate) menu: Option<EffortMenu>,
    session_details: Option<SessionDetails>,
    todo_composer: Option<TodoComposer>,
    new_session: Option<new_session::NewSession>,
    pub(crate) settings_menu: Option<SettingsMenu>,
    /// The Team view (: team), from the daemon's host-only team socket.
    pub(crate) team_view: Option<crate::teams::view::TeamView>,
    chief_view: Option<crate::chief::view::ChiefView>,
    chief_status: Result<Value, String>,
    /// Team requests, escalations and uncertain deliveries waiting for Theo; session -> "team role · task".
    team_attention: usize,
    team_roles: HashMap<String, String>,
    qa_sessions: HashSet<String>,
    /// Whether the last team check succeeded: until then anyone may be a member, and no one sleeps.
    team_known: bool,
    pub(crate) team_checked: f64,
    team_links: TeamLinks,
    pub(crate) sidebar_rows: Vec<crate::teams::sidebar::SidebarRow>,
    /// Where each tab sits on the tabs row (start, end, panel), for clicks.
    tab_ranges: Vec<(u16, u16, usize)>,
    collapsed_teams: HashSet<String>,
    sidebar_selection_lost: bool,
    sidebar_session_tasks: HashMap<String, crate::teams::TaskKey>,
    pub(crate) pairing: Option<Pairing>,
    pub restore: Vec<Restorable>,
    pub held: Vec<String>,
    /// When greyproxy's current holds began: its NEEDS YOU card's age.
    held_since: Option<i64>,
    pub(crate) held_checked: f64,
    notices: Vec<Notice>,
    /// The footer shows a y/n question (confirm), in blue rather than a message's foreground.
    is_confirming: bool,
    pub(crate) queue_checked: f64,
    /// Handed items the scheduler refused: retried at the next queue check, not on every tick,
    /// where each refusal cost the daemon a full live-session scan (about 25 a second).
    handed_refused: HashSet<String>,
    pub(crate) sleep_checked: f64,
    /// session -> the idle start `cockpit sleep` was asked about: a refusal is not asked again
    /// until the session worked and went idle again.
    sleep_asked: HashMap<String, i64>,
    /// Whether Theo games right now; the queue waits meanwhile (a test sets its own).
    pub gaming: fn() -> bool,
    /// The fullest watched filesystem (util::fullest_disk; a test sets its own), read every
    /// DISK_CHECK_S.
    pub disk_usage: fn() -> Option<(String, f64)>,
    pub(crate) disk: Option<(String, f64)>,
    pub(crate) disk_checked: f64,
    pub(crate) restore_checked: f64,
    pub(crate) start_files: Vec<(&'static str, f64)>,
    pub width: u16,
    /// The session cards shown, after the attention filter; the card selection indexes into it.
    pub sessions: Vec<Session>,
    /// Every running, non-external session not being exited, whatever the attention filter hides.
    pub running_sessions: Vec<Session>,
    pub(crate) session_task_titles: HashMap<String, String>,
    session_branches: HashMap<String, String>,
    session_queued: HashMap<String, usize>,
    /// What the Chief answered per session, as the team host last reported it; only sessions with answers.
    chief_answers: HashMap<String, ChiefAnswers>,
    session_roots: HashMap<String, String>,
    pub(crate) windows: Vec<Window>,
    quota_rows: Vec<usage::QuotaRow>,
    pub(crate) review_rows: Vec<Review>,
    pub todo_entries: Vec<Entry>,
    idea_entries: Vec<todo::Item>,
    pub(crate) todo_scope: String,
    /// The session in the session pane as of the last tick: its change wakes the column at once.
    shown_session: String,
    /// The Todo tab shown (TODO_TABS), and how many entries each holds.
    pub(crate) todo_tab: usize,
    /// session -> the Todo tab it last showed: a session not in it shows Open.
    todo_tabs: HashMap<String, usize>,
    todo_categories: HashMap<String, Option<todo::Category>>,
    todo_recovery_pending: bool,
    saved_todo_scope: Option<SavedTodoScope>,
    todo_jump: Option<(String, String)>,
    /// session -> when the column last sent it a queued prompt: the next one waits for a turn that
    /// ended after that.
    pub(crate) prompts_sent: HashMap<String, i64>,
    subscription_held: HashSet<String>,
    subscription_directories: HashSet<String>,
    subscription_error: Option<String>,
    /// origin -> the waiting reason last shown, so each reason appears once.
    subscription_waiting_shown: HashMap<String, String>,
    subscription_polled_at: Option<f64>,
    /// craft app -> the checkout folders its running instances serve (sessions::craft_roots).
    pub(crate) craft_roots: HashMap<String, HashSet<String>>,
    pub(crate) todo_counts: [usize; 5],
    pub(crate) hit_lines: HashMap<u16, (usize, Option<usize>)>,
    pub(crate) hit_actions: HashMap<u16, (u16, u16, &'static str)>,
    /// The session cards' data as the last tick read it (gather_sessions); a key press redraws from
    /// it instead of reading tmux and the files again.
    gathered: Option<Gathered>,
    /// cockpit commands a key started (open, view, digest…), not waited for: the column stays
    /// responsive while they run; an error shows once one ends (collect_manager).
    pending: Vec<(String, std::process::Child)>,
    /// The plan's name (usage::plan, from the credentials file), read once a tick.
    plan: String,
    /// When craft_roots was last asked of pgrep (CRAFT_CHECK_S).
    craft_checked: f64,
    /// Commits each running copy with a card on screen holds and GitHub lacks
    /// (sessions::running_copy_unpushed), and when counted.
    pub(crate) running_copies_unpushed: HashMap<PathBuf, usize>,
    running_copies_checked: f64,
}

/// pgrep for the craft apps' project folders forked on every tick (4 a second); which project
/// FigCraft serves changes when Theo opens it.
const CRAFT_CHECK_S: f64 = 5.0;

/// How often the column looks for sessions to put to sleep.
const SLEEP_CHECK_S: f64 = 60.0;
/// `cockpit sleep`'s exit status when the session must stay awake (typed text, background work…).
const SLEEP_REFUSED: i32 = 3;

/// What a press on the "Sleep after" row cycles through, in minutes.
const SLEEP_AFTER_CHOICES: [i64; 4] = [30, 60, 120, 240];

/// What the session cards are drawn from (App::gather_sessions).
struct Gathered {
    running: Vec<Session>,
    branches: HashMap<String, String>,
    queued: HashMap<String, usize>,
    roots: HashMap<String, String>,
    paused: usize,
    running_copies_unpushed: HashMap<PathBuf, usize>,
    start_files: Vec<(&'static str, f64)>,
    craft_roots: HashMap<String, HashSet<String>>,
}

impl App {
    pub fn new() -> Self {
        let mut todos = TodoList::load();
        let (message, todo_recovery_pending) = match crate::todo_editor::recover(&mut todos) {
            Ok(Some(0)) => (String::new(), false),
            Ok(Some(count)) => (format!("recovered {count} TODO draft(s) in their original projects"), false),
            Ok(None) => (String::new(), true),
            Err(error) => (format!("TODO draft recovery failed; drafts kept: {error}"), false),
        };
        Self {
            continuity: Continuity::load(&paths::continuity()),
            continuity_error: None,
            status_actions: Vec::new(), status_selection_lost: false,
            history: History::default(), view: ViewState::load(), usage: Refresher::new(), alerts: BudgetAlerts::new(util::send_notice),
            costs: Costs::default(), reviews: Reviews::default(), todos, errors: ErrorReporter::new(),
            claude_status: ProviderStatus::default(), codex_status: ProviderStatus::new(Provider::Codex), panels: Default::default(), focus: 1, previous_panel: 1,
            help_open: false, help_scroll: 0, has_pane_focus: true, message, exiting: Vec::new(), restarting_cards: HashMap::new(), labelled: Default::default(), menu: None, session_details: None, todo_composer: None, new_session: None, settings_menu: None, team_view: None, chief_view: None, team_attention: 0, team_roles: HashMap::new(), qa_sessions: HashSet::new(), team_known: false, team_checked: 0.0,
            team_links: TeamLinks::Loading, sidebar_rows: Vec::new(), tab_ranges: Vec::new(), collapsed_teams: HashSet::new(), sidebar_selection_lost: false, sidebar_session_tasks: HashMap::new(), pairing: None, restore: Vec::new(),
            held: Vec::new(), held_since: None, held_checked: 0.0, notices: Vec::new(), is_confirming: false, queue_checked: 0.0, handed_refused: HashSet::new(), sleep_checked: 0.0, sleep_asked: HashMap::new(), gaming: util::gaming,
            disk_usage: util::fullest_disk, disk: None, disk_checked: 0.0, restore_checked: 0.0, start_files: Vec::new(),
            width: DEFAULT_COLUMN_WIDTH, sessions: Vec::new(), running_sessions: Vec::new(), windows: Vec::new(), quota_rows: Vec::new(), review_rows: Vec::new(), todo_entries: Vec::new(), idea_entries: Vec::new(),
            session_task_titles: HashMap::new(), session_branches: HashMap::new(),
            session_queued: HashMap::new(), chief_answers: HashMap::new(), chief_status: Err("Not read yet".into()), session_roots: HashMap::new(),
            todo_scope: String::new(), shown_session: String::new(), todo_tab: TODO_TAB_OPEN, todo_tabs: HashMap::new(), todo_categories: HashMap::new(), saved_todo_scope: None, todo_jump: None, todo_counts: [0; 5], prompts_sent: HashMap::new(), subscription_held: HashSet::new(), subscription_directories: HashSet::new(), subscription_error: None, craft_roots: HashMap::new(), hit_lines: HashMap::new(), hit_actions: HashMap::new(),
            subscription_polled_at: None, subscription_waiting_shown: HashMap::new(),
            gathered: None, pending: Vec::new(), plan: String::new(), craft_checked: 0.0, running_copies_unpushed: HashMap::new(), running_copies_checked: 0.0, todo_recovery_pending,
        }
    }

    /// A panel taking the focus shows from its top: while small it scrolled to keep its selection
    /// in view, and that scroll would otherwise stay once it is big again.
    pub fn focus_panel(&mut self, index: usize) {
        if index == 0 { self.refresh_chief_status(); }
        if TAB_PANELS.contains(&self.focus) {
            self.previous_panel = self.focus;
        }
        if index != self.focus {
            self.panels[index].scroll = 0;
        }
        self.focus = index;
    }

    pub fn tick(&mut self) {
        self.shown_session = current_session();
        self.refresh();
        self.maintain();
    }

    /// Time-driven work and presentation from the last gathered sessions. Unlike tick(), this
    /// does not reread every session's inputs or spawn tmux merely because a second passed.
    pub fn maintain(&mut self) {
        if self.todo_recovery_pending {
            match crate::todo_editor::recover(&mut self.todos) {
                Ok(None) => (),
                Ok(Some(count)) => {
                    self.todo_recovery_pending = false;
                    if count > 0 { self.message = format!("recovered {count} TODO draft(s) in their original projects"); }
                }
                Err(error) => {
                    self.todo_recovery_pending = false;
                    self.message = format!("TODO draft recovery failed; drafts kept: {error}");
                }
            }
        }
        self.todos.apply_updates();
        self.check_team();
        self.errors.tick(|| sessions::reported().into_iter()
            .filter(|session| !self.qa_sessions.contains(&session.session)).collect());
        self.claude_status.tick();
        self.codex_status.tick();
        self.usage.tick(false);
        self.alerts.check(&self.windows);
        self.costs.tick();
        self.reviews.tick();
        self.watch_pairing();
        self.redraw();
        self.push_viewed_state();
        self.dispatch_subscription_routes();
        self.poll_subscription_routes();
        self.dispatch_queue();
        self.dispatch_prompts();
        self.dispatch_sleep();
    }

    /// A cockpit command a key asked for, started and not waited for (it takes 40 to 60 ms,
    /// up to 500): the column keeps answering keys meanwhile (security audit 2026-10-19,
    /// performance 2). Its error, if any, shows once it ends (collect_manager).
    pub(crate) fn manager_async(&mut self, arguments: &[&str]) -> String {
        let started = Command::new("cockpit").args(arguments).env("COCKPIT_LOG_STDERR", "1")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
        match started {
            Ok(child) => {
                self.pending.push((command_name(arguments).to_string(), child));
                String::new()
            }
            Err(error) => format!("cockpit {}: {error}", command_name(arguments)),
        }
    }

    /// `s`: the session sleeps now, whatever its idle time; cockpit's own checks (typed text,
    /// background work, a conversation to resume) still apply. Its stderr is kept out of the
    /// log (COCKPIT_LOG_STDERR unset): a refusal is shown, not reported as an error.
    fn sleep_now(&mut self, session: &Session) -> String {
        if session.asleep.is_some() {
            return format!("{} is already asleep", session.project);
        }
        if session.terminal {
            return format!("{} runs in its own terminal: the cockpit could not wake it", session.project);
        }
        if !self.team_known {
            return format!("cannot tell yet whether {} is in a team: try again in a moment", session.project);
        }
        let started = Command::new("cockpit").args(["sleep", &session.session])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn();
        match started {
            Ok(child) => {
                self.pending.push(("sleep".into(), child));
                format!("putting {} to sleep …", session.project)
            }
            Err(error) => format!("cockpit sleep: {error}"),
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
                    if status.success() {
                        has_ended |= command != "subscription-sweep";
                        continue;
                    }
                    has_ended = true;
                    let mut errors = String::new();
                    if let Some(mut stderr) = child.stderr.take() {
                        let _ = stderr.read_to_string(&mut errors);
                    }
                    let last = errors.trim().lines().last().unwrap_or("").to_string();
                    // `s` on a session that must stay awake: cockpit logged why at INFO.
                    if command == "sleep" && status.code() == Some(SLEEP_REFUSED) && !last.is_empty() {
                        self.message = last;
                        continue;
                    }
                    util::log_event("ERROR", &format!("cockpit {command}: {}", if last.is_empty() { "failed" } else { &last }));
                    self.message = if last.is_empty() { format!("cockpit {command} failed") } else { last };
                }
                Err(error) => {
                    has_ended = true;
                    self.message = format!("cockpit {command}: {error}");
                }
            }
        }
        has_ended
    }

    pub fn poll_subscription_routes(&mut self) {
        let now = util::now();
        if self.subscription_polled_at.is_some_and(|last| now - last < 60.0)
            || self.pending.iter().any(|(command, _)| command == "subscription-sweep") { return; }
        self.subscription_polled_at = Some(now);
        let error = self.manager_async(&["subscription-sweep"]);
        if !error.is_empty() { util::log_event("WARN", &error); }
    }

    fn show_subscription_waiting(&mut self, routes: &[crate::subscription_fallback::Route]) {
        let waiting: HashMap<String, String> = routes.iter()
            .filter_map(|route| Some((route.original.session.clone(), route.waiting.clone()?))).collect();
        use crate::subscription_fallback::Phase;
        for route in routes.iter().filter(|route| matches!(route.phase, Phase::Active | Phase::Pending(_))) {
            let Some(reason) = &route.waiting else { continue };
            if self.subscription_waiting_shown.get(&route.original.session) == Some(reason) { continue; }
            self.message = format!("{}: subscription switch waiting: {reason}", project_name(&util::repository_root(&route.directory)));
        }
        self.subscription_waiting_shown = waiting;
    }

    pub fn dispatch_subscription_routes(&mut self) {
        use crate::subscription_fallback::{self, Direction, Phase};
        let result = (|| -> Result<(), String> {
            let routes = subscription_fallback::read_routes(&paths::subscription_routes())?;
            self.subscription_held.clear();
            self.subscription_directories.clear();
            for route in routes.iter().filter(|route| matches!(route.original.selection.agent.as_str(), "claude" | "codex")) {
                if matches!(route.phase, Phase::Cancelled | Phase::Completed) { continue; }
                self.subscription_held.extend(route.retired_sessions.iter().cloned());
                if route.original.session != route.current.session { self.subscription_held.insert(route.original.session.clone()); }
                match &route.phase {
                    Phase::Pending(pending) => {
                        self.subscription_held.insert(route.current.session.clone());
                        self.subscription_directories.insert(route.directory.clone());
                        if pending.direction == Direction::Return { self.subscription_held.insert(route.original.session.clone()); }
                        if let Some(replacement) = &pending.replacement { self.subscription_held.insert(replacement.session.clone()); }
                    }
                    Phase::Paused(_) => {
                        self.subscription_held.insert(route.current.session.clone());
                        self.subscription_directories.insert(route.directory.clone());
                    }
                    Phase::Active => {
                        if route.awaiting_initial_admission() {
                            self.subscription_held.insert(route.current.session.clone());
                            self.subscription_directories.insert(route.directory.clone());
                        }
                        for retired in &route.retired_sessions {
                            prompts::transfer_subscription_prompts(retired, &route.current.session)?;
                        }
                    }
                    Phase::Cancelled | Phase::Completed => unreachable!(),
                }
            }
            self.show_subscription_waiting(&routes);
            Ok(())
        })();
        let error = result.err();
        if error != self.subscription_error {
            if let Some(error) = &error {
                util::log_event("WARN", &format!("automatic native actions paused: subscription routing: {error}"));
                self.message = format!("Subscription routing paused: {error}");
            }
            self.subscription_error = error;
        }
    }

    fn subscription_holds(&self, session: &Session) -> bool {
        matches!(session.agent.as_str(), "claude" | "codex")
            && (self.subscription_error.is_some() || self.subscription_held.contains(&session.session))
    }

    /// A session craft-open asked to restart (its figcraft-app server had exited at its start) is
    /// restarted once its turn ends, in a tmux session of its own; its queued prompt then waits for
    /// the restarted session. A terminal session is its terminal's: it is only told in the log.
    fn dispatch_restarts(&mut self) {
        let ready: Vec<Session> = self.running_sessions.iter()
            .filter(|session| !self.subscription_holds(session) && !session.is_external() && session.state == "waiting" && prompts::restart_wanted(&session.session)).cloned().collect();
        for session in ready {
            prompts::restart_done(&session.session);
            if session.terminal {
                util::log_event("WARN", &format!("{} waited for a restart at the end of its turn, but runs in its own terminal: restart it there", session.project));
                continue;
            }
            manager_in_background(&["restart", &session.session, "", "detached"]);
            self.prompts_sent.insert(session.session.clone(), util::now() as i64);
            util::log_event("INFO", &format!("{} restarted at the end of its turn", session.project));
        }
    }

    /// Each session whose turn has ended gets its oldest queued prompt (prompts.rs), one per turn:
    /// the next waits until the session worked and stopped again.
    pub fn dispatch_prompts(&mut self) {
        self.dispatch_restarts();
        let ready: Vec<Session> = self.running_sessions.iter()
            // An asleep card is no process: the wake its message asked for delivers it.
            .filter(|session| !self.subscription_holds(session) && !session.is_external() && session.asleep.is_none() && session.state == "waiting"
                && session.since > self.prompts_sent.get(&session.session).copied().unwrap_or(0))
            .cloned().collect();
        for session in ready {
            match crate::subscription_fallback::has_native_inbox(&session.session) {
                Ok(true) => continue,
                Ok(false) => (),
                Err(error) => { util::log_event("WARN", &format!("native message delivery paused: {error}")); continue; },
            }
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

    /// Every minute: a session left at its prompt past the sleep delay exits and keeps its card
    /// (`cockpit sleep` checks again before it exits it); notes a week old are dropped.
    pub fn dispatch_sleep(&mut self) {
        let now = util::now();
        if now - self.sleep_checked < SLEEP_CHECK_S {
            return;
        }
        self.sleep_checked = now;
        for note in sleep::expire(now as i64) {
            util::log_event("INFO", &format!("{} asleep for over a week, forgotten", note.project));
        }
        if !self.team_known {
            return;
        }
        let context = sleep::SleepContext {
            now: now as i64, enabled: settings::is_on("idle-sleep"), after_s: sleep::after_minutes() * 60,
            viewed: current_session(),
        };
        let eligible: Vec<Session> = self.running_sessions.iter()
            .filter(|session| !self.subscription_holds(session))
            .filter(|session| self.sleep_asked.get(&session.session) != Some(&session.since))
            .filter(|session| sleep::block(session, &context, self.todos.busy(&session.session), prompts::count(&session.session) > 0).is_none())
            .cloned().collect();
        self.sleep_asked.retain(|asked, _| self.running_sessions.iter().any(|session| &session.session == asked));
        for session in eligible {
            manager_in_background(&["sleep", &session.session]);
            self.sleep_asked.insert(session.session, session.since);
        }
    }

    /// Handed work starts fresh sessions on each tick. Every half minute, an idle session with no
    /// assigned work gets its project's first ordinary queued item. Not the session Theo has on
    /// screen in the cockpit: he may be about to type in it.
    pub fn dispatch_queue(&mut self) {
        let now = util::now();
        if self.todos.queued.is_empty() {
            return;
        }
        let context = self.queue_context();
        if now - self.queue_checked >= QUEUE_CHECK_S {
            self.handed_refused.clear();
        }
        self.start_sessions_for_handed_work(&context);
        if now - self.queue_checked < QUEUE_CHECK_S {
            return;
        }
        self.queue_checked = now;
        let idle: Vec<Session> = self.all_sessions().into_iter()
            .filter(|session| !self.subscription_holds(session))
            .filter(|session| queue::block(session, &context, self.todos.busy(&session.session), prompts::count(&session.session) > 0).is_none())
            .collect();
        for session in idle {
            let Some(entry) = self.next_queued(&session) else { continue };
            let (key, text) = match &entry {
                Entry::Typed(item) => (item.id(), todo::prompt_of(&entry)),
                Entry::Issue { issue, .. } => (issue.key(), todo::prompt_of(&entry)),
            };
            let token=match queue::reserve_task(&session.cwd,&key,Some(&session.session),true) {Ok(token)=>token,Err(error)=>{util::log_event("WARN",&format!("queue: {error}"));continue;}};
            if let Err(error) = queue::launch_reserved(&token,&["say", &session.session, &todo::with_todo_note(&text, &key)],false) {
                util::log_event("WARN", &format!("queue: cannot give {key} to {}: {error}", session.project));
                continue;
            }
            if !self.todos.set_status(&key, "doing", &session.session) {
                self.message = format!("queue: sent {key}, but cannot save its status");
                continue;
            }
            if let Err(error) = self.record_task_assignment(&entry, Some(&session)) { self.continuity_error = Some(error); }
            util::log_event("INFO", &format!("queue: {key} given to {} ({}), idle for {} min", session.project,
                                             &session.session[..8.min(session.session.len())], (now as i64 - session.since) / 60));
            self.message = format!("queue: gave {} its next item", session.project);
        }
    }

    /// Handed work and setup blockers get fresh background sessions on every tick, before idle
    /// sessions can absorb them. Theo's own Q items keep waiting for a session he starts.
    fn start_sessions_for_handed_work(&mut self, context: &queue::QueueContext) {
        if !context.enabled || context.gaming || !context.host_available {
            return;
        }
        let mut handed: Vec<Entry> = self.todos.items.iter()
            .filter(|item| item.status() == "todo" && self.todos.is_queued(&item.id())
                && (!item.queued_by().is_empty() || item.is_blocked_report()) && !item.is_waiting() && !item.is_design()
                && !self.handed_refused.contains(&item.id()))
            .map(|item| Entry::Typed(item.clone())).collect();
        handed.sort_by(todo::queue_order);
        for entry in handed {
            let Entry::Typed(item) = &entry else { continue };
            let project = project_name(&util::repository_root(&item.cwd()));
            if self.subscription_error.is_some() || self.subscription_directories.iter().any(|directory| project_name(&util::repository_root(directory)) == project) {
                continue;
            }
            let directory = match start_directory(&item.cwd()) {
                Ok(directory) => directory,
                Err(error) => { util::log_event("WARN", &format!("queue: cannot start a session for {}: {error}", item.id())); continue }
            };
            let agent = match settings::launch_agent() {
                Ok(agent) => agent,
                Err(error) => { util::log_event("WARN", &format!("queue: cannot start a session for {}: {error}", item.id())); return }
            };
            let token=match queue::reserve_task(&directory,&item.id(),None,true) {Ok(token)=>token,Err(error)=>{
                util::log_event("WARN",&format!("queue: {} not started, retried at the next queue check: {error}", item.id()));
                self.handed_refused.insert(item.id());
                continue;
            }};
            if let Err(error)=queue::launch_reserved(&token,&["--agent", &agent, "background", &directory, &todo::with_todo_note(&todo::prompt_of(&entry), &item.id())],true) {util::log_event("WARN",&error);continue;}
            if !self.todos.set_status(&item.id(), "doing", "") {
                self.message = format!("queue: started session, but cannot save status for {}", item.id());
                continue;
            }
            if let Err(error) = self.record_task_assignment(&entry, None) { self.continuity_error = Some(error); }
            util::log_event("INFO", &format!("queue: started a fresh session in {project} for {} ({})", item.id(), item.queued_by()));
            crate::audit::record("column", "queue.session_started", "ok", serde_json::json!({"id": item.id(), "project": project, "queued_by": item.queued_by()}));
            self.message = format!("queue: started a session in {project} for handed work");
        }
    }

    /// The project's first queued item still to do: typed items of its project, issues of its
    /// repository.
    fn next_queued(&self, session: &Session) -> Option<Entry> {
        queue::candidates(session, &self.todos, &self.reviews.issues(), &self.reviews.checkouts).into_iter()
            .next()
    }

    /// Everything read again (sessions, usage, plan), then drawn.
    pub fn build(&mut self) {
        self.refresh();
        self.check_team();
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
        self.build_ideas();
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
        let state = self.running_sessions.iter().find(|s| s.session == viewed)
            .map(|session| if session.needs_input { "needs-you".into() } else { session.state.clone() }).unwrap_or_default();
        if (viewed.clone(), state.clone()) == self.labelled {
            return;
        }
        self.labelled = (viewed.clone(), state.clone());
        if !viewed.is_empty() && !state.is_empty() {
            manager_in_background(&["label-state", &state, &viewed]);
        }
    }

    // ---- Status ----

    fn build_status(&mut self) {
        let selected_action = self.status_actions.get(self.panels[0].selected).copied();
        let previous_selection = self.panels[0].selected;
        self.status_actions = vec![None; self.quota_rows.len()];
        let room = self.width.saturating_sub(4) as usize;
        let labels: Vec<_> = self.quota_rows.iter().map(|quota| {
            let window = quota.label.split_once(" · ").map_or("Quota", |(_, window)| window);
            match window {
                "5h" => "5-hour".into(),
                "week" => "Weekly".into(),
                _ => window.replace(" this week", " weekly"),
            }
        }).collect();
        let capacities: Vec<_> = self.quota_rows.iter().map(|quota| match (quota.state.as_str(), quota.remaining) {
            ("Available", Some(remaining)) => format!("{remaining}%"),
            ("Stale", Some(remaining)) => format!("Stale · last {remaining}%"),
            (state, Some(remaining)) => format!("{remaining}% {state}"),
            (state, None) => state.into(),
        }).collect();
        let label_width = labels.iter().map(|label| width_of(label)).max().unwrap_or(0);
        let capacity_width = capacities.iter().map(|text| width_of(text)).chain([4]).max().unwrap_or(4);
        let forecast_width = self.quota_rows.iter().map(|quota| width_of(quota.forecast.text())).chain([11]).max().unwrap_or(11);
        let reset_width = self.quota_rows.iter().map(|quota| width_of(quota.resets.trim_start_matches("resets in "))).chain([9]).max().unwrap_or(9);
        let capacity_column = 2 + label_width + 2;
        let forecast_column = capacity_column + capacity_width + 2;
        let reset_column = forecast_column + forecast_width + 2;
        let full_table = reset_column + reset_width <= room;
        // Every notice, the Sessions tab showing only the first NOTICES_SHOWN.
        let mut lines = Vec::new();
        if !self.notices.is_empty() {
            lines.push(Line::new(vec![segment(0, "NOTICES", bold(FG_DARK))]));
            for notice in &self.notices {
                lines.push(notice_line(notice));
                if !notice.detail.is_empty() {
                    lines.push(Line { segments: vec![segment(2, notice.detail.clone(), dim(FG_DARK))], accent: Some(bold(notice.tint)), ..Line::default() });
                }
            }
            lines.push(Line::default());
        }
        lines.push(Line::new(vec![segment(0, "USAGE", bold(FG_DARK))]));
        if full_table {
            lines.push(Line::new(vec![segment(capacity_column as u16, "LEFT", dim(FG_DARK)),
                segment(forecast_column as u16, "RUNS OUT IN", dim(FG_DARK)), segment(reset_column as u16, "RESETS IN", dim(FG_DARK))]));
        } else {
            lines.push(Line::new(vec![segment(0, "LEFT · remaining capacity", dim(FG_DARK))]));
        }
        let mut previous_provider = "";
        for (index, quota) in self.quota_rows.iter().enumerate() {
            let provider = quota.label.split_once(" · ").map_or(quota.label.as_str(), |(provider, _)| provider);
            if provider != previous_provider {
                if !previous_provider.is_empty() { lines.push(Line::default()); }
                lines.push(Line::new(vec![segment(0, provider, bold(FG))]));
                previous_provider = provider;
            }
            let tint = bold(match quota.state.as_str() {
                "Available" if quota.remaining.is_some_and(|remaining| remaining <= 50) => YELLOW,
                "Available" => GREEN,
                "Stale" => YELLOW,
                "Low" => RED,
                "Exhausted" | "Blocked" => RED,
                _ => FG_DARK,
            });
            let forecast_tint = match quota.forecast {
                usage::Forecast::Immediate => bold(RED),
                _ if matches!(quota.state.as_str(), "Low" | "Exhausted" | "Blocked") => bold(RED),
                usage::Forecast::Depletion(_) => bold(YELLOW),
                _ => dim(FG_DARK),
            };
            let reset = if quota.resets.is_empty() { "—" } else { quota.resets.trim_start_matches("resets in ") };
            let mut row = Line { item: Some(index), ..Line::default() };
            if capacity_column + capacity_width <= room {
                row.segments = vec![segment(2, &labels[index], style(FG)), segment(capacity_column as u16, &capacities[index], tint)];
            } else {
                for label in wrap_detail_text(&labels[index], room.saturating_sub(2).max(2)) {
                    lines.push(Line { segments: vec![segment(2, label, style(FG))], item: Some(index), ..Line::default() });
                }
                row.segments = vec![segment(4, fit(&capacities[index], room.saturating_sub(4)), tint)];
            }
            if full_table {
                row.segments.push(segment(forecast_column as u16, quota.forecast.text(), forecast_tint));
                row.segments.push(segment(reset_column as u16, reset, dim(FG_DARK)));
            }
            lines.push(row);
            if !full_table {
                let forecast = match &quota.forecast {
                    usage::Forecast::UntilReset => "Lasts until reset".into(),
                    usage::Forecast::Immediate => "May run out now".into(),
                    _ => format!("Runs out in {}", quota.forecast.text()),
                };
                for (text, tint) in [(forecast, forecast_tint), (format!("Resets in {reset}"), dim(FG_DARK))] {
                    for text in wrap_detail_text(&text, room.saturating_sub(4).max(2)) {
                        lines.push(Line { segments: vec![segment(4, text, tint)], item: Some(index), ..Line::default() });
                    }
                }
            }
        }
        lines.push(Line::default());
        for text in wrap_detail_text("Estimates use window-average pace. — = no estimate.", room.max(2)) {
            lines.push(Line::new(vec![segment(0, text, dim(FG_DARK))]));
        }
        let billing = self.costs.billing_totals();
        // All sessions together, never the sum of the projects' times: parallel sessions overlap.
        let together = self.costs.time_together();
        let mut spending = Vec::new();
        let has_priced_replies = billing.recorded_replies > billing.unpriced_replies;
        if billing.recorded_replies > 0 {
            for (label, totals) in [("API credits (est.)", billing.api), ("Subscription (API value)", billing.subscription),
                ("Unknown billing (API value)", billing.unknown)] {
                let amounts = if has_priced_replies { [totals.today, totals.week, totals.month].map(money) }
                    else { std::array::from_fn(|_| "—".into()) };
                spending.push((label, amounts, bold(MAGENTA)));
            }
            let total = [0, 1, 2].map(|period| [billing.api, billing.subscription, billing.unknown].iter()
                .map(|totals| [totals.today, totals.week, totals.month][period]).sum::<f64>());
            let amounts = if has_priced_replies { total.map(money) } else { std::array::from_fn(|_| "—".into()) };
            spending.push(("≈ Total at API prices", amounts, bold(MAGENTA)));
        }
        if together.month >= 60.0 {
            spending.push((TIME_LABEL, [together.today, together.week, together.month].map(Self::duration_text), bold(CYAN)));
        }
        let error = self.costs.error();
        let all_time = self.costs.all_time();
        {
            lines.push(Line::default());
            for text in wrap_detail_text("SPENDING · token-based estimates, not your bill", room.max(2)) {
                lines.push(Line::new(vec![segment(0, text, bold(FG_DARK))]));
            }
            if self.costs.is_loading() {
                lines.push(Line::new(vec![segment(0, "Loading usage…", dim(FG_DARK))]));
            } else if billing.recorded_replies == 0 && error.is_empty() {
                lines.push(Line::new(vec![segment(0, "No recorded usage", dim(FG_DARK))]));
            }
            if billing.unpriced_replies > 0 {
                let warning = format!("Incomplete estimate: {} unpriced replies; amounts exclude them.", billing.unpriced_replies);
                for text in wrap_detail_text(&warning, room.max(2)) {
                    lines.push(Line::new(vec![segment(0, text, style(YELLOW))]));
                }
            }
            let label_width = spending.iter().map(|(label, _, _)| width_of(label)).chain([width_of("All-time API value")]).max().unwrap_or(0);
            let amount_width = spending.iter().flat_map(|(_, amounts, _)| amounts.iter().map(|text| width_of(text))).chain([5]).max().unwrap_or(5);
            let columns = [0, 1, 2].map(|index| (label_width + 2 + index * (amount_width + 2)) as u16);
            let periods = ["TODAY", "WEEK", "MONTH"];
            if columns[2] as usize + amount_width <= room {
                if !spending.is_empty() {
                    lines.push(Line::new(periods.iter().enumerate().map(|(index, period)|
                        segment(columns[index] + (amount_width - width_of(period)) as u16, *period, dim(FG_DARK))).collect()));
                }
                for (label, amounts, tint) in spending {
                    let mut segments = vec![segment(0, label, style(FG))];
                    segments.extend(amounts.iter().enumerate().map(|(index, amount)|
                        segment(columns[index] + (amount_width - width_of(amount)) as u16, amount, tint)));
                    lines.push(Line::new(segments));
                }
            } else {
                for (label, amounts, tint) in spending {
                    for text in wrap_detail_text(label, room.max(2)) {
                        lines.push(Line::new(vec![segment(0, text, style(FG))]));
                    }
                    for (period, amount) in periods.iter().zip(amounts) {
                        for text in wrap_detail_text(&format!("{period}  {amount}"), room.saturating_sub(2).max(2)) {
                            lines.push(Line::new(vec![segment(2, text, tint)]));
                        }
                    }
                }
            }
            if has_priced_replies {
                let amount = format!("≈ {}", money(all_time));
                let label = "All-time API value";
                if label_width + 2 + width_of(&amount) <= room {
                    lines.push(Line::new(vec![segment(0, label, style(FG)), segment(label_width as u16 + 2, amount, bold(MAGENTA))]));
                } else {
                    for text in wrap_detail_text(label, room.max(2)) { lines.push(Line::new(vec![segment(0, text, style(FG))])); }
                    for text in wrap_detail_text(&amount, room.saturating_sub(2).max(2)) { lines.push(Line::new(vec![segment(2, text, bold(MAGENTA))])); }
                }
            }
            if billing.recorded_replies > 0 {
                for explanation in ["Subscription = API-price value, not a subscription charge.", "Unknown = billing mode unverified; includes historical records."] {
                    for text in wrap_detail_text(explanation, room.max(2)) {
                        lines.push(Line::new(vec![segment(0, text, dim(FG_DARK))]));
                    }
                }
            }
            for text in wrap_detail_text(&error, room.max(2)) {
                lines.push(Line::new(vec![segment(0, text, style(YELLOW))]));
            }
        }
        let mut projects: Vec<String> = self.running_sessions.iter().map(|session| session.project.clone()).collect();
        projects.sort();
        projects.dedup();
        let project_times: Vec<(String, String)> = projects.into_iter()
            .filter_map(|project| Self::time_spent_text(self.costs.time_spent(&project)).map(|time| (project, time))).collect();
        if !project_times.is_empty() {
            lines.push(Line::default());
            lines.push(Line::new(vec![segment(0, "TIME BY PROJECT", bold(FG_DARK))]));
            for (project, time) in project_times {
                let project = crate::teams::sidebar::shown(&project);
                let column = width_of(&project) as u16 + 2;
                lines.push(Line::new(vec![segment(0, project.clone(), bold(project_color(&project))), segment(column, time, dim(FG_DARK))]));
            }
        }
        lines.push(Line::default());
        lines.push(Line::new(vec![segment(0, "CHIEF", bold(FG_DARK))]));
        let chief_lines = match &self.chief_status {
            Ok(snapshot) => crate::chief::view::ChiefView::new(snapshot.clone()).status_lines(),
            Err(error) => vec![format!("Chief unavailable · {}", crate::chief::view::one_line(error, 160))],
        };
        for text in chief_lines {
            for line in wrap_detail_text(&text, room.max(2)) { lines.push(Line::new(vec![segment(0, line, style(FG))])); }
        }
        lines.push(Line::new(vec![segment(0, " c ", on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD)), segment(4, "Chief view", dim(FG_DARK)),
            segment(16, " l ", on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD)), segment(20, "answer log", dim(FG_DARK)),
            segment(32, " h ", on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD)), segment(36, "habits", dim(FG_DARK))]));
        lines.push(Line::default());
        lines.push(Line::new(vec![segment(0, "SERVICES", bold(FG_DARK))]));
        for (provider, action) in [(&self.claude_status, StatusAction::ClaudeStatus), (&self.codex_status, StatusAction::CodexStatus)] {
            let index = self.status_actions.len();
            self.status_actions.push(Some(action));
            lines.extend(provider_status_lines(provider, room).into_iter().map(|mut line| { line.item = Some(index); line }));
        }
        lines.push(Line::default());
        lines.push(Line::new(vec![segment(0, "UTILITIES", bold(FG_DARK))]));
        for (label, action) in [("Daily digest", StatusAction::Digest), ("Clones review / cleanup", StatusAction::Clones)] {
            let index = self.status_actions.len();
            self.status_actions.push(Some(action));
            lines.push(Line { segments: vec![segment(0, label, style(FG))], item: Some(index), ..Default::default() });
        }
        let panel = &mut self.panels[0];
        panel.items = self.status_actions.len();
        if let Some(Some(action)) = selected_action {
            panel.selected = self.status_actions.iter().position(|candidate| *candidate == Some(action)).expect("all Status actions remain available");
        } else if selected_action == Some(None) && previous_selection >= self.quota_rows.len() {
            self.status_selection_lost = true;
        }
        panel.clamp();
        panel.lines = lines;
    }

    fn status_subtitle(&self) -> String {
        self.plan.clone()
    }

    // ---- Sessions ----

    /// x on a card: a session in tmux leaves the list at once while `cockpit exit` runs.
    fn exit_session(&mut self, session: &Session) -> String {
        if session.asleep.is_some() {
            // No process to wait for: `cockpit exit` forgets its note and remembers it ended.
            manager_in_background(&["exit", &session.session]);
            return format!("ended {}", session.project);
        }
        if session.tmux_pane.is_empty() {
            return kill_session(session);
        }
        match util::manager_child(&["exit", &session.session]) {
            Ok(child) => {
                self.exiting.push(Exiting { session: session.session.clone(), tmux_pane: session.tmux_pane.clone(), project: session.project.clone(), child, ended_at: None });
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
        let mut is_pending = |exit: &mut Exiting| {
            // The hook can disappear before tmux closes, leaving a fallback card with a new ID.
            if !running.iter().any(|s| s.session == exit.session || s.tmux_pane == exit.tmux_pane) {
                return false;
            }
            if exit.ended_at.is_none() {
                match exit.child.try_wait() {
                    Ok(None) => return true,
                    Ok(Some(status)) if status.success() => exit.ended_at = Some(now),
                    Ok(Some(status)) => {
                        warnings.push(format!("⚠ {} did not exit ({status}): see ~/.local/state/cockpit/log", exit.project));
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
        };
        for mut exit in std::mem::take(&mut self.exiting) {
            if is_pending(&mut exit) {
                self.exiting.push(exit);
            } else {
                // Its session can be gone while `cockpit exit` still runs.
                util::reap_in_background(exit.child);
            }
        }
        if let Some(warning) = warnings.pop() {
            self.message = warning;
        }
    }

    /// What the session cards are drawn from, read from tmux, pgrep and the per-session files: once
    /// per tick, never on a key press (security audit 2026-10-19, performance 1).
    fn gather_sessions(&mut self) -> Gathered {
        let mut running: Vec<Session> = sessions::running();
        let known: Vec<String> = running.iter().map(|session| session.conversation.as_ref().unwrap_or(&session.session))
            .filter(|session_id| agents::is_session_id(session_id)).cloned().collect();
        self.history.load_titles(&known, TITLES_PER_TICK);
        for session in &mut running {
            if session.state != "unknown" {
                session.facts = sessions::statusline_facts(&session.session);
            }
            session.title = self.history.title(session.conversation.as_deref().unwrap_or(&session.session));
            session.design = sessions::design_of(&session.session);
            session.role = sessions::role_of(&session.session);
        }
        let running_copies: HashSet<PathBuf> = running.iter().filter_map(|s| sessions::running_copy_of(&s.cwd)).collect();
        self.observe_task_trails(&running);
        if util::read_json(&paths::snapshot()).is_some_and(|snapshot| snapshot.get("sessions").is_some_and(Value::is_array)) {
            if let Ok(continuity) = &mut self.continuity {
                match continuity.observe_sessions(&running, util::now_secs()) {
                    Ok(true) => self.continuity_error = None,
                    Ok(false) => {},
                    Err(error) => self.continuity_error = Some(error),
                }
            }
        }
        Gathered {
            branches: running.iter().map(|session| (session.session.clone(), util::git_branch(&session.cwd))).collect(),
            queued: running.iter().map(|session| (session.session.clone(), prompts::count(&session.session))).collect(),
            roots: running.iter().map(|session| (session.session.clone(), util::repository_root(&session.cwd))).collect(),
            running,
            paused: std::fs::read_to_string(paths::game_frozen()).map(|list| list.lines().filter(|line| !line.trim().is_empty()).count()).unwrap_or(0),
            start_files: sessions::start_file_mtimes(),
            // What a running copy holds and GitHub lacks: U on a card of its clones pushes it
            // (apply_run). Counted here, out of the sandbox, in the running copy no session writes;
            // only for the cards on screen, and like craft_roots at most every CRAFT_CHECK_S.
            running_copies_unpushed: if now() - self.running_copies_checked >= CRAFT_CHECK_S {
                self.running_copies_checked = now();
                running_copies.into_iter().map(|live| { let count = sessions::running_copy_unpushed(&live); (live, count) }).collect()
            } else {
                self.running_copies_unpushed.iter().filter(|(live, _)| running_copies.contains(*live)).map(|(live, count)| (live.clone(), *count)).collect()
            },
            craft_roots: if now() - self.craft_checked >= CRAFT_CHECK_S {
                self.craft_checked = now();
                ["figcraft"].iter().filter_map(|app| Some((app.to_string(), sessions::craft_roots(app)?))).collect()
            } else {
                self.craft_roots.clone()
            },
        }
    }

    /// A session being restarted keeps its card, marked Restarting, and the selection while its old
    /// process is gone and the new one has not reported: without it the column selected and viewed
    /// the next card, which also killed a restart launching in the session pane. The new process's
    /// card takes the selection over.
    fn hold_restarting_cards(&mut self, running: &mut Vec<Session>, selected_id: &mut Option<String>) {
        let mut held = HashMap::new();
        for restart in sessions::restarts() {
            let previous = self.restarting_cards.remove(&restart.session)
                .or_else(|| self.running_sessions.iter().find(|session| session.session == restart.session).cloned());
            let old_pane = previous.as_ref().map(|card| card.tmux_pane.clone()).unwrap_or_default();
            if let Some(replacement) = restart.replacement(running, &old_pane) {
                if selected_id.as_deref() == Some(restart.session.as_str()) {
                    *selected_id = Some(replacement.session.clone());
                }
                sessions::end_restart(&restart.session);
                continue;
            }
            if now() as i64 - restart.since > RESTART_TIMEOUT_S {
                util::log_event("WARN", &format!("{} did not report within {RESTART_TIMEOUT_S}s of its restart", restart.project));
                self.message = format!("⚠ {} did not come back from its restart: cockpit logs", restart.project);
                sessions::end_restart(&restart.session);
                continue;
            }
            running.retain(|session| !restart.is_unreported_launch(session));
            let still_running = running.iter().position(|session| session.session == restart.session).map(|index| running.remove(index));
            let mut card = still_running.or(previous).unwrap_or_else(|| Session {
                session: restart.session.clone(), project: restart.project.clone(), cwd: restart.cwd.clone(),
                since: restart.since, started: Some(restart.since), ..Session::default()
            });
            card.restarting = true;
            card.state = "unknown".into();
            card.needs_input = false;
            card.ask_to_run.clear();
            held.insert(restart.session.clone(), card.clone());
            running.push(card);
        }
        self.restarting_cards = held;
    }

    /// A sleeping session keeps a card until it is woken. A note the session outlived (it started
    /// after the sleep) is stale. While the old process still shows, or a wake is under way
    /// (the restarting card), that card stands and the note adds none.
    fn add_asleep_cards(&self, running: &mut Vec<Session>) {
        let waking: HashSet<String> = sessions::restarts().into_iter().map(|restart| restart.session).collect();
        for note in sleep::asleep() {
            if waking.contains(&note.session) { continue; }
            match running.iter().find(|session| session.session == note.session) {
                Some(session) if session.started.is_some_and(|started| started >= note.slept_at) => sleep::forget(&note.session),
                Some(_) => {}
                None => running.push(note.card()),
            }
        }
    }

    fn build_sessions(&mut self) {
        use crate::teams::sidebar::{self, SidebarRow};
        let selected_row = self.sidebar_rows.get(self.panels[1].selected).cloned();
        let selected_team = selected_row.as_ref().and_then(|row| match row {
            SidebarRow::Team { task } | SidebarRow::PlannedQa { task } => Some(task.clone()),
            SidebarRow::Session { session_id } => self.sidebar_session_tasks.get(session_id).cloned(),
        });
        let mut selected_id = self.selected_session().map(|session| session.session);
        let gathered = match self.gathered.take() {
            Some(gathered) => gathered,
            None => self.gather_sessions(),
        };
        let mut running: Vec<Session> = gathered.running.clone();
        self.session_branches = gathered.branches.clone();
        self.session_queued = gathered.queued.clone();
        self.session_roots = gathered.roots.clone();
        self.settle_exits(&running);
        running.retain(|s| !s.is_external() && !self.exiting.iter().any(|exit| exit.session == s.session || exit.tmux_pane == s.tmux_pane));
        self.hold_restarting_cards(&mut running, &mut selected_id);
        self.add_asleep_cards(&mut running);
        for session in &mut running {
            session.pinned = self.view.pinned.contains(&session.session);
        }
        self.running_sessions = sessions::grouped_by_project(running.clone());
        // A card keeps its place: pinned first, then in the order the sessions started.
        let running = sessions::grouped_by_project(running);
        let mut rows = match &self.team_links {
            TeamLinks::Ready(snapshot) => sidebar::grouped_rows(snapshot, &running, &self.collapsed_teams).unwrap_or_default(),
            _ => running.iter().map(|session| SidebarRow::Session { session_id: session.session.clone() }).collect(),
        };
        if let TeamLinks::Ready(snapshot) = &self.team_links {
            let hidden_tasks: Vec<_> = rows.iter().filter_map(|row| match row {
                SidebarRow::Team { task } if self.team_is_inactive(snapshot, task, &running) => Some(task.clone()),
                _ => None,
            }).collect();
            rows.retain(|row| match row {
                SidebarRow::Team { task } | SidebarRow::PlannedQa { task } => !hidden_tasks.contains(task),
                SidebarRow::Session { session_id } => !running.iter().find(|session| session.session == *session_id)
                    .and_then(|session| sidebar::member_task(snapshot, session)).is_some_and(|task| hidden_tasks.contains(&task)),
            });
        }
        // Grouped by attention across projects (NEEDS YOU, WORKING, QUIET); a team stays one block.
        let now_secs = util::now_secs();
        let mut blocks: Vec<(Attention, (bool, i64), Vec<SidebarRow>)> = Vec::new();
        for row in rows {
            let team_task = match (&self.team_links, &row) {
                (TeamLinks::Ready(_), SidebarRow::Team { task } | SidebarRow::PlannedQa { task }) => Some(task.clone()),
                (TeamLinks::Ready(snapshot), SidebarRow::Session { session_id }) => running.iter().find(|session| session.session == *session_id)
                    .and_then(|session| sidebar::member_task(snapshot, session)),
                _ => None,
            };
            let joins_team = team_task.is_some() && !matches!(row, SidebarRow::Team { .. }) && blocks.last().is_some_and(|(_, _, block)|
                matches!(block.first(), Some(SidebarRow::Team { task }) if Some(task) == team_task.as_ref()));
            if joins_team { blocks.last_mut().unwrap().2.push(row); } else { blocks.push((Attention::Quiet, (false, 0), vec![row])); }
        }
        for (group, oldest_ask, block) in &mut blocks {
            let members: Vec<&Session> = block.iter().filter_map(|row| match row {
                SidebarRow::Session { session_id } => running.iter().find(|session| session.session == *session_id), _ => None }).collect();
            let team_needs_you = matches!((&self.team_links, block.first()), (TeamLinks::Ready(snapshot), Some(SidebarRow::Team { task })) if sidebar::attention(snapshot, task).is_some());
            *group = members.iter().map(|session| self.session_attention(session, now_secs)).min().unwrap_or(Attention::Quiet);
            if team_needs_you { *group = Attention::NeedsYou; }
            let is_pinned = members.iter().any(|session| session.pinned);
            let oldest = members.iter().filter(|session| self.is_actionable(session, now_secs)).map(|session| session.since).min().unwrap_or(0);
            *oldest_ask = (!is_pinned, if *group == Attention::NeedsYou { oldest } else { 0 });
        }
        // Pinned first inside their group, then the oldest ask first in NEEDS YOU; the others keep their places.
        blocks.sort_by_key(|(group, order, _)| (*group, *order));
        let mut blocks_per_group: HashMap<Attention, usize> = HashMap::new();
        for (group, _, _) in &blocks { *blocks_per_group.entry(*group).or_default() += 1; }
        let row_groups: Vec<Attention> = blocks.iter().flat_map(|(group, _, block)| std::iter::repeat_n(*group, block.len())).collect();
        let row_ends_block: Vec<bool> = blocks.iter().flat_map(|(_, _, block)| (0..block.len()).map(move |position| position + 1 == block.len())).collect();
        let rows: Vec<SidebarRow> = blocks.into_iter().flat_map(|(_, _, block)| block).collect();
        let current = current_session();
        let mut lines = Vec::new();
        if now() - self.restore_checked > RESTORE_CHECK_S {
            self.restore_checked = now();
            self.restore = sessions::restore_offer(&self.view, &self.running_sessions.iter().map(|s| s.session.clone()).collect());
        }
        if now() - self.held_checked > GREYPROXY_POLL_S {
            self.held_checked = now();
            let held = sessions::greyproxy_held(&paths::greyproxy_db());
            for host in held.iter().filter(|host| !self.held.contains(host)) {
                crate::audit::record("column", "greyproxy.held", "waiting", serde_json::json!({"destination": host}));
            }
            self.held_since = if held.is_empty() { None } else { self.held_since.or(Some(util::now_secs())) };
            self.held = held;
        }
        if now() - self.disk_checked > DISK_CHECK_S {
            self.disk_checked = now();
            self.disk = (self.disk_usage)();
        }
        // game-performance froze the sessions for a game: their cards stand still until it ends.
        let paused = gathered.paused;
        self.start_files = gathered.start_files.clone();
        self.running_copies_unpushed = gathered.running_copies_unpushed.clone();
        self.craft_roots = gathered.craft_roots.clone();
        self.gathered = Some(gathered);
        self.notices = self.current_notices(paused);
        for notice in self.notices.iter().take(NOTICES_SHOWN) {
            lines.push(notice_line(notice));
        }
        if self.notices.len() > NOTICES_SHOWN {
            lines.push(Line::new(vec![segment(0, format!("+{} more · s for Status", self.notices.len() - NOTICES_SHOWN), dim(FG_DARK))]));
        }
        if !self.notices.is_empty() { lines.push(Line::default()); }
        self.session_task_titles.clear();
        for session in &running {
            let branch = self.session_branches.get(&session.session).map(String::as_str).unwrap_or("");
            self.session_task_titles.insert(session.session.clone(), self.session_task_title(session, branch));
        }
        let link_notice = match &self.team_links {
            TeamLinks::Loading => Some("Loading team links…"),
            TeamLinks::Unavailable => Some("Team links unavailable"),
            TeamLinks::Ready(snapshot) if !rows.iter().any(|row| matches!(row, SidebarRow::Team { .. })) && sidebar::list(snapshot, "members").is_empty() => {
                Some("No linked teams")
            },
            _ => None,
        };
        let mut group = None;
        let held_cards = usize::from(!self.held.is_empty());
        if held_cards > 0 {
            group = Some(Attention::NeedsYou);
            let count = blocks_per_group.get(&Attention::NeedsYou).copied().unwrap_or(0) + held_cards;
            lines.push(Line::new(vec![segment(0, format!("{} · {count}", Attention::NeedsYou.heading()), bold(YELLOW))]));
            lines.extend(greyproxy_card(&self.held, self.held_since.unwrap_or(now_secs)));
        }
        for (index, row) in rows.iter().enumerate() {
            let session = if let SidebarRow::Session { session_id } = row { running.iter().find(|session| session.session == *session_id) } else { None };
            let team_task = match (&self.team_links, row) {
                (TeamLinks::Ready(_), SidebarRow::Team { task } | SidebarRow::PlannedQa { task }) => Some(task.clone()),
                (TeamLinks::Ready(snapshot), _) => session.and_then(|session| sidebar::member_task(snapshot, session)),
                _ => None,
            };
            let row_group = row_groups.get(index).copied().unwrap_or(Attention::Quiet);
            if group != Some(row_group) {
                group = Some(row_group);
                let count = blocks_per_group.get(&row_group).copied().unwrap_or(0);
                let tint = if row_group == Attention::NeedsYou { bold(YELLOW) } else { bold(FG_DARK) };
                lines.push(Line::new(vec![segment(0, format!("{} · {count}", row_group.heading()), tint)]));
            }
            if let (TeamLinks::Ready(snapshot), Some(task)) = (&self.team_links, &team_task) {
                match row {
                    SidebarRow::Team { .. } => {
                        let members: Vec<&Session> = running.iter().filter(|session| sidebar::member_task(snapshot, session).as_ref() == Some(task)).collect();
                        let marker = if self.collapsed_teams.contains(&sidebar::task_key(task)) { "▸" } else { "▾" };
                        let (glyph, tint) = match row_group {
                            Attention::NeedsYou => ("!", YELLOW),
                            Attention::Working => ("◐", BLUE),
                            Attention::Quiet => ("✓", GREEN),
                        };
                        let age = members.iter().map(|session| session.started.unwrap_or(session.since)).min().map(util::age).unwrap_or_default();
                        let accent = (row_group == Attention::NeedsYou).then_some(bold(YELLOW));
                        lines.push(Line { segments: vec![segment(0, glyph, bold(tint)), segment(2, match sidebar::member_count(snapshot, task, &running) { 0 => format!("{marker} Team"), count => format!("{marker} Team of {count}") }, bold(FG))],
                            right: vec![part(age, dim(FG_DARK))], item: Some(index), accent, ..Line::default() });
                        let project = sidebar::project_name(snapshot, task, &running);
                        lines.push(Line { segments: why_segments(&project, &self.team_task_title(snapshot, task)), item: Some(index), accent, ..Line::default() });
                        let reason = sidebar::attention(snapshot, task).map(|reason| reason.trim_start_matches("Needs you: ").to_string());
                        if let Some(reason) = reason { lines.push(ask_bar(Some(index), &reason, "", "⏎ open")); }
                        if sidebar::has_qa_plan(snapshot, task) || snapshot["handoffs"].as_array().is_some_and(|handoffs| !handoffs.is_empty()) {
                            lines.push(Line { segments: vec![segment(2, sidebar::handoff_summary(snapshot, task), dim(FG_DARK))], item: Some(index), accent, ..Line::default() });
                        }
                        if members.is_empty() || self.collapsed_teams.contains(&sidebar::task_key(task)) { lines.push(Line::default()); }
                        continue;
                    },
                    SidebarRow::PlannedQa { .. } => {
                        lines.push(Line { segments: vec![segment(2, "QA        ", style(FG_DARK)), segment(12, sidebar::qa_status(snapshot, task), dim(FG_DARK))], item: Some(index), ..Line::default() });
                        if row_ends_block.get(index).copied().unwrap_or(true) { lines.push(Line::default()); }
                        continue;
                    },
                    SidebarRow::Session { .. } => {
                        let Some(session) = session else { continue };
                        let role = sidebar::membership(snapshot, session).and_then(sidebar::role).map(sidebar::role_label).unwrap_or("Member");
                        let accent = (session.session == current).then_some(style(BLUE));
                        let card = self.session_card(session, now_secs);
                        let tint = if card.ask.is_some() { YELLOW } else if card.group == Attention::Working { BLUE } else { FG_DARK };
                        // A member's own step while it works, else its title: the line still says which session it is.
                        let has_step = card.group == Attention::Working && sessions::doing_now(session, now_secs).is_some();
                        let (shown, status) = if has_step { (card.doing.clone(), card.age.clone()) }
                            else { (sidebar::shown(self.session_display_title(session)), format!("{} {}", card.doing.split(" · ").next().unwrap_or(""), card.age)) };
                        lines.push(Line { segments: vec![segment(2, format!("{role:<10}"), style(tint)), segment(12, shown, if card.ask.is_some() { style(YELLOW) } else { style(FG) })],
                            right: vec![part(status, dim(FG_DARK))], item: Some(index), accent, ..Line::default() });
                        if let Some((ask, verb)) = &card.ask { lines.push(ask_bar(Some(index), ask, &card.ask_count, &format!("⏎ {verb}"))); }
                        if row_ends_block.get(index).copied().unwrap_or(true) { lines.push(Line::default()); }
                        continue;
                    },
                }
            }
            let Some(session) = session else { continue };
            let card = self.session_card(session, now_secs);
            let accent = if card.ask.is_some() { Some(bold(YELLOW)) } else { (session.session == current).then_some(bold(BLUE)) };
            let task_title = self.session_task_title(session, self.session_branches.get(&session.session).map(String::as_str).unwrap_or(""));
            self.session_task_titles.insert(session.session.clone(), task_title);
            let text_style = if card.group == Attention::Quiet && !card.bright { style(FG_DARK) } else { bold(FG) };
            // Until the grouping replaces continuity updates (todo f192c901), a new question stays marked; an
            // unseen result is the card's own state (session_card).
            let updates = self.continuity.as_ref().ok().map(|state| state.updates()).unwrap_or_default();
            // Chief ✓N leads line one when it fits beside the age, else it opens line two.
            let chief_count = self.chief_answers.get(&session.session).map(|answers| format!("Chief ✓{}", answers.count));
            let (chief_on_line_one, chief_on_line_two) = match chief_count {
                Some(count) if width_of(&count) + 2 + width_of(&card.age) <= session_card_right_room(self.width, 2) => (Some(count), None),
                other => (None, other),
            };
            let mut right: Vec<Part> = chief_on_line_one.into_iter().map(|count| part(count, dim(FG_DARK))).collect();
            right.extend(updates.iter().filter(|update| update.session == session.session && update.kind == crate::continuity::UpdateKind::Question)
                .map(|_| part("new question", style(MAGENTA))));
            // The badges give way before the age does.
            let room = session_card_right_room(self.width, 2);
            let parts_width = |parts: &[Part]| parts.iter().map(|part| width_of(&part.text) + 2).sum::<usize>();
            let mut badges: Vec<Part> = if session.restarting { Vec::new() } else { self.card_badges(session) }
                .into_iter().take(CARD_BADGES_SHOWN).map(|badge| part(badge, style(FG_DARK))).collect();
            while !badges.is_empty() && parts_width(&right) + parts_width(&badges) + width_of(&card.age) > room { badges.pop(); }
            right.extend(badges);
            right.push(part(card.age.clone(), if card.stale { style(FG) } else { dim(FG_DARK) }));
            lines.push(Line { segments: vec![segment(0, card.glyph, bold(card.tint)), segment(2, card.doing.clone(), text_style)],
                right, item: Some(index), accent, ..Line::default() });
            let mut why = why_segments(&session.project, &sidebar::shown(self.session_display_title(session)));
            if let Some(count) = chief_on_line_two {
                let shift = width_of(&count) as u16 + 3;
                for segment in &mut why { segment.0 += shift; }
                why.insert(0, segment(2, format!("{count} · "), dim(FG_DARK)));
            }
            if session.pinned { why.insert(0, segment(0, "⚲", dim(FG_DARK))); }
            lines.push(Line { segments: why, item: Some(index), accent, ..Line::default() });
            if let Some((ask, verb)) = &card.ask { lines.push(ask_bar(Some(index), ask, &card.ask_count, &format!("⏎ {verb}"))); }
            if selected_id.as_deref() == Some(session.session.as_str()) && session.asleep.is_none() {
                let meta = sidebar::shown(&self.card_meta(session));
                for text in wrap_detail_text(&meta, (self.width as usize).saturating_sub(8).max(8)).into_iter().take(2) {
                    lines.push(Line { segments: vec![segment(2, text, dim(FG_DARK))], item: Some(index), accent, ..Line::default() });
                }
            }
            lines.push(Line::default());
        }
        if rows.is_empty() {
            lines.push(Line::new(vec![segment(0, "No running session.", dim(FG_DARK))]));
            lines.push(Line::new(vec![segment(0, format!("a  start one in a project{}", link_notice.map(|notice| format!(" · {notice}")).unwrap_or_default()), dim(FG_DARK))]));
        } else if let Some(notice) = link_notice {
            lines.push(Line::new(vec![segment(0, notice, style(FG_DARK))]));
        }
        self.sessions = running;
        // In the list's order, so walking the sessions follows what Theo sees.
        let row_position = |session: &Session| rows.iter().position(|row| matches!(row, SidebarRow::Session { session_id } if *session_id == session.session)).unwrap_or(usize::MAX);
        self.sessions.sort_by_key(|session| row_position(session));
        self.sidebar_rows = rows;
        self.sidebar_session_tasks = match &self.team_links {
            TeamLinks::Ready(snapshot) => self.sessions.iter().filter_map(|session| sidebar::member_task(snapshot, session).map(|task| (session.session.clone(),task))).collect(),
            _ => HashMap::new(),
        };
        let panel = &mut self.panels[1];
        let selected_index = selected_id.as_ref().and_then(|id| self.sidebar_rows.iter().position(|row| matches!(row, SidebarRow::Session { session_id } if session_id == id)))
            .or_else(|| selected_row.as_ref().and_then(|selected| self.sidebar_rows.iter().position(|row| row == selected)))
            .or_else(|| selected_team.as_ref().and_then(|task| self.sidebar_rows.iter().position(|row| matches!(row, SidebarRow::Team { task: candidate } if candidate == task))))
            .or_else(|| selected_row.is_none().then(|| self.sidebar_rows.iter().position(|row| matches!(row, SidebarRow::Session { .. }))).flatten());
        if selected_team.is_some() && selected_index.is_none() { self.sidebar_selection_lost = true; }
        if let Some(index) = selected_index {
            panel.selected = index;
        }
        panel.items = self.sidebar_rows.len();
        panel.clamp();
        panel.lines = lines;
        self.panels[1].fixed_top = self.session_summary_lines();
        self.panels[1].fixed_bottom = self.selected_session_detail_lines();
        if selected_team.is_none() && selected_id.as_ref().is_some_and(|id| !self.all_sessions().iter().any(|session| session.session == *id)) && selected_index.is_none()
            && !is_viewer_loading() {
            // Viewing an asleep card wakes it, and moving the selection never does.
            if let Some(session) = self.selected_session().filter(|session| session.asleep.is_none()) {
                let error = self.manager_async(&["view", &session.session]);
                if !error.is_empty() {
                    self.message = error;
                }
            }
        }
    }

    /// Claude's incident, sessions paused for a game, a full disk, sessions to restore: in this order.
    fn current_notices(&self, paused: usize) -> Vec<Notice> {
        let mut notices = Vec::new();
        let (claude, _) = self.claude_status.snapshot();
        if let Some(claude) = claude.filter(|c| !c.indicator.is_empty() && c.indicator != "none") {
            let mut details: Vec<String> = claude.degraded.iter().take(2).map(|(name, state)| format!("{name}: {state}")).collect();
            details.extend(claude.incidents.iter().take(1).cloned());
            let tint = if claude.indicator == "maintenance" { BLUE } else { RED };
            notices.push(Notice { glyph: "!", tint, text: format!("Claude: {}", claude.description), key: "s status", detail: details.join(" · ") });
        }
        if paused > 0 {
            notices.push(Notice { glyph: "‖", tint: BLUE, text: format!("{paused} session{} paused while you game · resume after", plural(paused)),
                key: "", detail: "they resume where they stopped when the game ends".into() });
        }
        if let Some((mount, used)) = self.disk.as_ref().filter(|(_, used)| *used >= DISK_WARNING_PERCENT) {
            notices.push(Notice { glyph: "▲", tint: RED, text: format!("Disk {used:.0}% full on {mount}"), key: "",
                detail: "make -C ~/dotfiles disk shows what uses it".into() });
        }
        if !self.restore.is_empty() {
            let names = self.restore.iter().map(|r| if r.project.is_empty() { "?" } else { r.project.as_str() }).collect::<Vec<_>>().join(", ");
            notices.push(Notice { glyph: "↺", tint: BLUE, text: format!("{} session{} ran before the restart", self.restore.len(), plural(self.restore.len())),
                key: "B restore", detail: format!("{names}: B brings them back, one after the other") });
        }
        notices
    }

    fn session_attention(&self, session: &Session, now: i64) -> Attention {
        if self.is_actionable(session, now) {
            return Attention::NeedsYou;
        }
        if session.asleep.is_some() || session.needs_attention() {
            return Attention::Quiet;
        }
        match sessions::activity_label(session) {
            "Working" | "Command running" | "Restarting" => Attention::Working,
            _ => Attention::Quiet,
        }
    }

    fn session_card(&self, session: &Session, now: i64) -> Card {
        let group = self.session_attention(session, now);
        let doing_now = sessions::doing_now(session, now);
        let doing_text = doing_now.as_ref().map(|doing| crate::teams::sidebar::shown(&doing.text));
        let stale_for = doing_now.as_ref().and_then(|doing| doing.stale_for);
        let mut card = Card { group, glyph: "·", tint: FG_DARK, doing: String::new(), age: util::age(session.since), stale: false, bright: false, ask: None, ask_count: String::new() };
        if let Some(slept_at) = session.asleep {
            (card.glyph, card.tint, card.doing, card.age) = ("☾", GREEN, "Asleep".into(), util::age(slept_at));
            return card;
        }
        if group == Attention::NeedsYou {
            let mut asks: Vec<(String, &'static str)> = Vec::new();
            match session.ask_to_run.as_str() {
                "approval" => asks.push(("Approve a command".into(), "review")),
                "input" => asks.push(("A running command needs your input".into(), "answer")),
                _ => {}
            }
            let request = util::collapse(&sessions::waiting_request(session));
            if !request.is_empty() {
                asks.push((crate::teams::sidebar::shown(&request), "answer"));
            } else if session.state == "needs-you" && asks.is_empty() {
                asks.push(("An answer or permission is needed".into(), "answer"));
            }
            if asks.is_empty() {
                asks.push(("Answer a question".into(), "answer"));
            }
            if asks.len() > 1 { card.ask_count = format!("1 of {}", asks.len()); }
            card.ask = asks.into_iter().next();
            (card.glyph, card.tint) = ("!", YELLOW);
            card.doing = doing_text.unwrap_or_else(|| "Waiting for you".into());
            return card;
        }
        if session.restarting {
            (card.glyph, card.tint, card.doing) = ("⟳", BLUE, "Restarting…".into());
            return card;
        }
        if let Some((badge, detail)) = self.session_dependency_activity(session) {
            card.glyph = "↳";
            card.doing = detail.replacen("Waiting on TODO", "Waiting on", 1).replacen("Blocked by TODO", "Blocked by", 1);
            if badge == "Waiting on TODO" {
                card.age = "queued".into();
            } else {
                card.doing = "Blocked: its todo isn't running · r run it".into();
            }
            return card;
        }
        if let Some(expiry) = self.session_deferred_until(session, now) {
            let minutes = ((expiry - now).max(0) + 59) / 60;
            (card.glyph, card.doing) = ("‖", format!("Later · back in {minutes}m"));
            return card;
        }
        match sessions::activity_label(session) {
            "Command running" => {
                (card.glyph, card.tint, card.doing) = ("▶", BLUE, "Running your approved command".into());
            },
            "Working" => {
                (card.glyph, card.tint) = ("◐", BLUE);
                card.doing = doing_text.unwrap_or_else(|| "Working".into());
                if let Some(stale_for) = stale_for {
                    (card.age, card.stale) = (format!("no update {}", util::duration(stale_for)), true);
                } else if let Some(doing) = &doing_now {
                    card.age = util::age(doing.at);
                }
            },
            "Ready" if self.has_new_result(session) => {
                (card.glyph, card.tint, card.doing, card.bright) = ("●", GREEN, "Finished · new result".into(), true);
            },
            "Ready" if sessions::is_idle(session, now) => {
                card.glyph = "○";
                card.doing = format!("Idle {} · waiting for your next message", util::duration(now - session.since));
            },
            "Ready" => {
                (card.glyph, card.tint) = ("✓", GREEN);
                let outcome = session.last_result.as_ref().map(|result| result.outcome.trim()).filter(|outcome| !outcome.is_empty())
                    .or(Some(session.outcome.trim()).filter(|outcome| !outcome.is_empty()));
                card.doing = match outcome { Some(outcome) => format!("Finished · {}", crate::teams::sidebar::shown(outcome)), None => "Finished".into() };
            },
            _ if session.since > 0 => card.doing = format!("State unknown (no report for {})", util::duration(now - session.since)),
            _ => card.doing = "State unknown (no report yet)".into(),
        }
        card
    }

    /// A result the session finished that Theo has not opened yet (continuity updates).
    fn has_new_result(&self, session: &Session) -> bool {
        self.continuity.as_ref().is_ok_and(|continuity| continuity.updates().iter()
            .any(|update| update.session == session.session && update.kind == crate::continuity::UpdateKind::Result))
    }

    /// A card's line-1 badges, the most urgent first: restart needed, unpushed, queued, update, API credits.
    fn card_badges(&self, session: &Session) -> Vec<String> {
        let mut badges = Vec::new();
        if !session.is_external() && !sessions::changed_since_start(session, &self.start_files).is_empty() {
            badges.push("⟳".to_string());
        }
        let unpushed = if session.unpushed > 0 { session.unpushed as usize } else { self.running_copy_unpushed(&session.cwd) };
        if unpushed > 0 { badges.push(format!("↑{unpushed}")); }
        let queued = self.session_queued.get(&session.session).copied().unwrap_or(0);
        if queued > 0 { badges.push(format!("✉{queued}")); }
        if session.update { badges.push("⬆".into()); }
        if session.credits { badges.push("$".into()); }
        badges
    }

    /// The selected card's meta line: agent, model, effort, context, background work, remote, terminal,
    /// canvas and billing. None of these sit on a card's first line.
    fn card_meta(&self, session: &Session) -> String {
        let facts = &session.facts;
        let mut parts = vec![session.agent().label.to_string()];
        if !facts.model.is_empty() { parts.push(facts.model.clone()); }
        if !facts.effort.is_empty() { parts.push(facts.effort.clone()); }
        if let Some(context) = facts.context { parts.push(format!("ctx {}%", context.round())); }
        parts.extend(sessions::background_badge(session));
        if facts.remote { parts.push("⇅ remote".into()); }
        if session.terminal { parts.push("▭ terminal".into()); }
        if let Some(design) = &session.design { parts.push(format!("◆ {}", design.title)); }
        parts.push(if session.credits { "$ API".into() } else { "subscription".into() });
        parts.join(" · ")
    }

    /// The todo a blocked card waits on that nobody runs: `r` on the card starts it.
    fn stopped_blocker(&self, session: &Session) -> Option<todo::Item> {
        let branch_id = self.session_branches.get(&session.session).and_then(|branch| branch.strip_prefix("todo-"));
        self.todos.items.iter()
            .filter(|item| item.status() != "done" && (item.session() == session.session
                || (branch_id == Some(item.id().as_str()) && item.project() == session.project)))
            .flat_map(|item| item.after())
            .find_map(|id| self.todos.items.iter().find(|item| item.id() == id && !matches!(item.status().as_str(), "done" | "doing")).cloned())
    }

    fn session_dependency_activity(&self, session: &Session) -> Option<(&'static str, String)> {
        if session.asleep.is_some() || session.needs_attention() || sessions::activity_label(session) != "Ready" {
            return None;
        }
        let branch_id = self.session_branches.get(&session.session).and_then(|branch| branch.strip_prefix("todo-"));
        let mut seen = HashSet::new();
        let mut blockers: Vec<_> = self.todos.items.iter()
            .filter(|item| item.status() != "done" && (item.session() == session.session
                || (branch_id == Some(item.id().as_str()) && item.project() == session.project)))
            .flat_map(|item| item.after()).filter(|id| seen.insert(id.clone()))
            .filter_map(|id| match self.todos.items.iter().find(|item| item.id() == id) {
                Some(item) if item.status() == "done" => None,
                Some(item) => Some((item.status() == "doing", util::collapse(&item.text()))),
                None => Some((false, id)),
            }).collect();
        blockers.sort_by_key(|(is_running, _)| *is_running);
        let (is_running, title) = blockers.first()?;
        let (badge, prefix) = if *is_running {
            ("Waiting on TODO", "Waiting on TODO")
        } else {
            ("Blocked · TODO not running", "Blocked by TODO")
        };
        let mut detail = format!("{prefix}: {title}");
        if blockers.len() > 1 { detail += &format!(" · +{} more", blockers.len() - 1); }
        Some((badge, detail))
    }

    fn session_task_title(&self, session: &Session, branch: &str) -> String {
        let title = util::collapse(&session.title);
        if !title.is_empty() {
            return title;
        }
        if let Some(title) = branch.strip_prefix("todo-")
            .and_then(|id| self.todos.items.iter().find(|item| item.id() == id && item.project() == session.project))
            .map(|item| util::collapse(&item.text())).filter(|title| !title.is_empty()) {
            return title;
        }
        if branch.is_empty() { session.project.clone() } else { branch.into() }
    }

    fn session_display_title<'a>(&'a self, session: &'a Session) -> &'a str {
        if !session.label.is_empty() {
            return &session.label;
        }
        self.session_task_titles.get(&session.session).map(String::as_str).unwrap_or(&session.project)
    }

    fn selected_session_detail_lines(&self) -> Vec<Line> {
        use crate::teams::sidebar::{self, SidebarRow};
        if self.sidebar_selection_lost {
            return vec![Line::new(vec![segment(0, "Selected member unavailable · choose a row", style(YELLOW))])];
        }
        if let (TeamLinks::Ready(snapshot), Some(SidebarRow::Team { task } | SidebarRow::PlannedQa { task })) =
            (&self.team_links, self.sidebar_rows.get(self.panels[1].selected)) {
            return vec![Line::new(vec![segment(0, format!("Selected team: {}", self.team_task_title(snapshot, task)), bold(CYAN))]),
                Line::new(vec![segment(0, sidebar::shown(&format!("{} / {}", task.project, task.task_id)), style(FG_DARK))]),
                Line::new(vec![segment(0, sidebar::attention(snapshot, task).unwrap_or_else(|| sidebar::handoff_summary(snapshot, task)), style(FG_DARK))])];
        }
        let Some(selected) = self.selected_session() else { return vec![] };
        let session = &selected;
        let branch = self.session_branches.get(&session.session).map(String::as_str).unwrap_or("");
        let task_title = self.session_task_titles.get(&session.session).map(String::as_str).unwrap_or(&session.project);
        let mut lines = Vec::new();
        let needs_attention = session.needs_attention();
        let facts = &session.facts;
        let mut badges = Vec::new();
        if !session.ask_to_run.is_empty() {
            let tint = if session.ask_to_run == "running" { BLUE } else { YELLOW };
            badges.push(part(format!("ask-to-run: {}", session.ask_to_run), bold(tint)));
        }
        if session.is_external() {
            badges.push(part("external", bold(CYAN)));
        }
        if !session.is_external() && facts.context.is_some_and(|context| context >= CONTEXT_COMPACT_AT) {
            badges.push(part("/compact", bold(BLUE)));
        }
        if session.update {
            badges.push(part("⬆ update", bold(GREEN)));
        }
        if !session.is_external() && !sessions::changed_since_start(session, &self.start_files).is_empty() {
            badges.push(part("⟳ restart", bold(BLUE)));
        }
        if facts.remote {
            badges.push(part("⇅ remote", bold(CYAN)));
        }
        if session.terminal {
            badges.push(part("▭ terminal", bold(MAGENTA)));
        }
        if session.credits {
            badges.push(part("$ credits", dim(FG_DARK)));
        }
        if let Some(badge) = session.agent().badge {
            badges.push(part(badge, bold(BLUE)));
        }
        match session.role.as_str() {
            "designer" => badges.push(part("✎ designer", bold(MAGENTA))),
            "developer" => badges.push(part("⌨ developer", style(CYAN))),
            _ => {}
        }
        if let Some(team) = self.team_roles.get(&session.session) {
            badges.push(part(team.clone(), bold(MAGENTA)));
        }
        // Launchers attach every craft server; show only instances serving this checkout.
        for app in session.apps.split_whitespace() {
            let Some((name, "on")) = app.split_once(':') else { continue };
            let serves_it = self.craft_roots.get(name).is_none_or(|roots| self.session_roots.get(&session.session).is_some_and(|root| roots.contains(root)));
            if serves_it {
                badges.push(part(format!("◆ {name}"), bold(GREEN)));
            }
        }
        if let Some(background) = sessions::background_badge(session) {
            badges.push(part(background, bold(BLUE)));
        }
        let queued = self.session_queued.get(&session.session).copied().unwrap_or(0);
        if queued > 0 {
            badges.push(part(format!("✉ {queued} queued"), bold(CYAN)));
        }
        if session.unpushed > 0 {
            let count = if sessions::is_reviewed_clone(&session.cwd) { "unapplied" } else { "unpushed" };
            badges.push(part(format!("↑{} {count}", session.unpushed), bold(BLUE)));
        }
        let running_copy_unpushed = self.running_copy_unpushed(&session.cwd);
        if running_copy_unpushed > 0 {
            badges.push(part(format!("↑{running_copy_unpushed} unpushed"), bold(BLUE)));
        }
        if session.pinned {
            badges.push(part("⚲", dim(FG_DARK)));
        }
        if let Some(answers) = self.chief_answers.get(&session.session) {
            badges.push(part(format!("Chief ✓{}", answers.count), dim(FG_DARK)));
        }
        badges.push(part(age(session.since), dim(FG_DARK)));
        let mut right = Vec::new();
        if session.state != "unknown" {
            if !session.is_external() {
                let effort = if facts.effort.is_empty() { "effort" } else { facts.effort.as_str() };
                right.push(Part { text: format!("{effort} ▾"), style: style(CYAN), action: Some("effort") });
            }
            right.push(part(&facts.model, dim(FG_DARK)));
            right.push(part(facts.context.map(|c| format!("{}%", c.round())).unwrap_or_default(), bold(context_color(facts.context))));
        }
        right.retain(|p| !p.text.is_empty());
        lines.push(Line { segments: vec![segment(0, format!("Selected: {task_title}"), bold(BLUE))], right: vec![Part { text: "i Full details".into(), style: style(CYAN), action: Some("session-details") }], ..Line::default() });
        let location = if branch.is_empty() { session.project.clone() } else { format!("{} / {branch}", session.project) };
        lines.push(Line { segments: vec![segment(0, location, dim(FG_DARK))], right, ..Line::default() });
        if let Ok(continuity) = &self.continuity {
            if let Some(note) = continuity.note(&session.session) {
                lines.push(Line::new(vec![segment(0, format!("Your note: {note}"), style(CYAN))]));
            }
        }
        if let Some(design) = &session.design {
            badges.push(part(format!("◆ {}", design.title), style(MAGENTA)));
        }
        let extra = if needs_attention {
            sessions::activity_text(session)
        } else if session.state == "waiting" && !session.outcome.is_empty() {
            format!("Last turn: {}", util::collapse(&session.outcome))
        } else {
            String::new()
        };
        let wrapped = wrap_detail_text(&extra, (self.width as usize).saturating_sub(5).max(2));
        for index in 0..2 {
            let mut text = wrapped.get(index).cloned().unwrap_or_default();
            if index == 1 && wrapped.len() > 2 {
                text = format!("{}…", fit(&text, (self.width as usize).saturating_sub(6)));
            }
            lines.push(Line::new(vec![segment(0, text, if needs_attention { style(YELLOW) } else { dim(FG_DARK) })]));
        }
        lines.push(Line { right: badges, ..Line::default() });
        if let TeamLinks::Ready(snapshot) = &self.team_links {
            if let Some(member) = sidebar::membership(snapshot, session) {
                if let Some(task) = sidebar::task_of(member) {
                    let role = sidebar::role(member).map(sidebar::role_label).unwrap_or("Member");
                    lines.insert(0, Line::new(vec![segment(0, format!("{} / {role} · {}", self.team_task_title(snapshot, &task), self.session_display_title(session)), bold(CYAN))]));
                    lines.push(Line::new(vec![segment(0, format!("{} · {} · {}", session.agent, session.session, task.task_id), style(FG_DARK))]));
                    lines.push(Line::new(vec![segment(0, sidebar::handoff_summary(snapshot, &task), style(FG_DARK))]));
                    for line in &mut lines {
                        for (_, text, _) in &mut line.segments { *text = sidebar::shown(text); }
                        for part in &mut line.right { part.text = sidebar::shown(&part.text); }
                    }
                }
            }
        }
        lines
    }

    fn session_summary_lines(&self) -> Vec<Line> {
        let now = util::now_secs();
        // One bucket per session, in the cards' precedence, so the counts cannot overlap.
        let (mut needing, mut working, mut ready, mut idle, mut asleep, mut unknown, mut waiting, mut blocked) = (0, 0, 0, 0, 0, 0, 0, 0);
        for session in self.all_sessions() {
            if session.needs_attention() {
                if self.session_deferred_until(&session, now).is_none() { needing += 1; }
            } else if let Some((badge, _)) = self.session_dependency_activity(&session) {
                if badge == "Waiting on TODO" { waiting += 1; } else { blocked += 1; }
            } else if sessions::is_idle(&session, now) {
                idle += 1;
            } else if session.asleep.is_some() {
                asleep += 1;
            } else {
                match sessions::activity_label(&session) {
                    "Working" | "Command running" => working += 1,
                    "Ready" => ready += 1,
                    _ => unknown += 1,
                }
            }
        }
        needing += usize::from(!self.held.is_empty());
        let mut attention: String = if needing == 0 { "Nothing needs you".into() } else { format!("! {needing} need you") };
        if let Ok(continuity) = &self.continuity {
            let unseen = continuity.updates().len();
            if unseen > 0 { attention += &format!(" · {unseen} unseen"); }
            let deferred = continuity.deferred_count(util::now_secs());
            if deferred > 0 { attention += &format!(" · {deferred} deferred"); }
        }
        let mut counts = format!("{working} working · {ready} ready");
        if waiting > 0 { counts += &format!(" · {waiting} waiting on TODO"); }
        if blocked > 0 { counts += &format!(" · {blocked} blocked"); }
        if idle > 0 {
            counts += &format!(" · {idle} idle");
        }
        if asleep > 0 {
            counts += &format!(" · {asleep} asleep");
        }
        if unknown > 0 {
            counts += &format!(" · {unknown} unknown");
        }
        let outdated = self.running_sessions.iter().filter(|session| !sessions::changed_since_start(session, &self.start_files).is_empty()).count();
        if outdated > 0 {
            counts += &format!(" · {outdated} restart available");
        }
        if self.team_attention > 0 {
            counts += &format!(" · team: {} {} you (: team)", self.team_attention, if self.team_attention == 1 { "needs" } else { "need" });
        }
        vec![
            Line::new(vec![segment(0, attention, bold(if needing > 0 { YELLOW } else { FG_DARK }))]),
            Line { segments: vec![segment(0, counts, dim(FG_DARK))], right: vec![Part { text: "n Next needing me".into(), style: style(if needing > 0 { YELLOW } else { FG_DARK }), action: (needing > 0).then_some("next-attention") }], ..Line::default() },
        ]
    }

    pub(crate) fn selected_session(&self) -> Option<Session> {
        if self.sidebar_selection_lost { return None; }
        let crate::teams::sidebar::SidebarRow::Session { session_id } = self.sidebar_rows.get(self.panels[1].selected)? else { return None };
        self.sessions.iter().find(|session| session.session == *session_id).cloned()
    }

    fn team_is_inactive(&self, snapshot: &Value, task: &crate::teams::TaskKey, sessions: &[Session]) -> bool {
        let is_done = self.todos.items.iter().any(|item| item.id() == task.task_id && item.status() == "done" && !item.cwd().is_empty()
            && (task.project == format!("path:{}", item.cwd())
                || crate::teams::sidebar::list(snapshot, "sessions").iter().any(|session| session["project"] == task.project && session["cwd"] == item.cwd())));
        is_done && crate::teams::sidebar::attention(snapshot, task).is_none()
            && !sessions.iter().any(|session| crate::teams::sidebar::member_task(snapshot, session).as_ref() == Some(task)
                && session.asleep.is_none() && (session.needs_attention() || sessions::activity_label(session) != "Ready"))
    }

    fn team_task_title(&self, snapshot: &Value, task: &crate::teams::TaskKey) -> String {
        use crate::teams::sidebar;
        let belongs = |cwd: &str| !cwd.is_empty() && (task.project == format!("path:{cwd}")
            || sidebar::list(snapshot, "sessions").iter().any(|session| session["project"] == task.project && session["cwd"] == cwd));
        if let Some(item) = self.todos.items.iter().find(|item| item.id() == task.task_id && belongs(&item.cwd())) {
            if !item.text().trim().is_empty() { return sidebar::shown(&item.text()); }
        }
        if let Some(issue) = self.reviews.issues().iter().find(|issue| issue.key() == task.task_id
            && self.reviews.checkouts.get(&issue.full_repository).is_some_and(|cwd| belongs(cwd))) {
            return sidebar::shown(&issue.title);
        }
        sidebar::shown(&task.task_id)
    }

    fn select_sidebar_target(&mut self, target: crate::teams::sidebar::SidebarRow) -> bool {
        use crate::teams::sidebar::{self, SidebarRow};
        let task = match &target {
            SidebarRow::Team { task } => Some(task.clone()),
            SidebarRow::Session { session_id } => self.sessions.iter().find(|session| session.session == *session_id).and_then(|session| match &self.team_links {
                TeamLinks::Ready(snapshot) => sidebar::member_task(snapshot, session), _ => None,
            }),
            _ => None,
        };
        if let Some(task) = task { self.collapsed_teams.remove(&sidebar::task_key(&task)); }
        self.build_sessions();
        if let Some(index) = self.sidebar_rows.iter().position(|row| row == &target) {
            self.panels[1].selected = index;
            self.sidebar_selection_lost = false;
            return true;
        }
        self.sidebar_selection_lost = true;
        false
    }

    fn next_sidebar_attention(&mut self) -> String {
        use crate::teams::sidebar::{self, SidebarRow};
        let rows = match &self.team_links {
            TeamLinks::Ready(snapshot) => sidebar::grouped_rows(snapshot, &self.sessions, &HashSet::new()).unwrap_or_default(),
            _ => self.sessions.iter().map(|session| SidebarRow::Session { session_id:session.session.clone() }).collect(),
        };
        let current = self.sidebar_rows.get(self.panels[1].selected);
        let selected_index = current.and_then(|current| rows.iter().position(|row| row == current));
        let candidates: Vec<_> = rows.iter().enumerate().filter(|(_, row)| match row {
            SidebarRow::Session { session_id } => self.sessions.iter().find(|session| session.session == *session_id).is_some_and(|session| self.is_actionable(session, util::now_secs())),
            SidebarRow::Team { task } => matches!(&self.team_links, TeamLinks::Ready(snapshot) if sidebar::attention(snapshot, task).is_some()
                && !self.sessions.iter().any(|session| sidebar::member_task(snapshot, session).as_ref() == Some(task) && self.is_actionable(session, util::now_secs()))),
            _ => false,
        }).collect();
        let Some((_, target)) = candidates.iter().find(|(index, _)| selected_index.is_none_or(|selected| *index > selected)).or(candidates.first()) else {
            if !self.held.is_empty() { return "only greyproxy needs you: G reviews its held requests".into(); }
            return "nothing needs you".into();
        };
        let target = (*target).clone();
        if !self.select_sidebar_target(target.clone()) { return "Selected session unavailable; choose a row".into(); }
        if let SidebarRow::Session { session_id } = target {
            let attended = self.manager_async(&["attend", &session_id]);
            self.mark_result_seen(&session_id).unwrap_or(attended)
        } else { String::new() }
    }

    fn open_session_details(&mut self) {
        let Some(selected) = self.selected_session() else { return };
        let session = &selected;
        let now = util::now_secs();
        let card = self.session_card(session, now);
        let original = self.session_task_titles.get(&session.session).map(String::as_str).unwrap_or(&session.project);
        let branch = self.session_branches.get(&session.session).map(String::as_str).unwrap_or("");
        let shown = self.session_display_title(session);
        let mut text = if shown == session.project { format!("{shown}\n") } else { format!("{} › {shown}\n", session.project) };
        if original != self.session_display_title(session) && original != session.project { text += &format!("Task: {original}\n"); }
        text += "\nWHERE & WHO\n";
        let mut place = Vec::new();
        if sessions::clone_name(&session.cwd).is_some() || !branch.is_empty() {
            if let Some(clone) = sessions::clone_name(&session.cwd) { place.push(format!("clone {clone}")); }
            if !branch.is_empty() { place.push(format!("branch {branch}")); }
        } else if !session.cwd.is_empty() { place.push(session.cwd.clone()); }
        if session.unpushed > 0 { place.push(format!("↑{} unpushed", session.unpushed)); }
        if !place.is_empty() { text += &format!("{}\n", place.join(" · ")); }
        let facts = &session.facts;
        let mut who = vec![session.agent().label.to_string()];
        if !facts.model.is_empty() { who.push(facts.model.clone()); }
        if !facts.effort.is_empty() { who.push(format!("effort {}", facts.effort)); }
        if let Some(context) = facts.context { who.push(format!("ctx {}%", context.round())); }
        if session.since > 0 {
            who.push(format!("since {}", chrono::DateTime::from_timestamp(session.since, 0)
                .map(|time| time.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default()));
        }
        text += &format!("{}\nBilling: {}\n\nAROUND IT\n", who.join(" · "),
            if session.credits { "API credits" } else { "subscription" });
        let mut around = Vec::new();
        if let Some(answers) = self.chief_answers.get(&session.session) { around.push(format!("◆ Chief answered {}", answers.count)); }
        if let Some(team) = self.team_roles.get(&session.session) { around.push(format!("⇄ {team}")); }
        let queued = self.session_queued.get(&session.session).copied().unwrap_or(0);
        if queued > 0 { around.push(format!("✉ {queued} queued message{}", if queued == 1 { "" } else { "s" })); }
        if let Some(background) = sessions::background_badge(session) { around.push(format!("◷ {background}")); }
        if facts.remote { around.push("⇅ remote control on".into()); }
        text += &format!("{}\n\nMY NOTE (never sent to the agent)\n", if around.is_empty() { "nothing else around it".into() } else { around.join(" · ") });
        let note = self.continuity.as_ref().ok().and_then(|continuity| continuity.note(&session.session).map(str::to_string));
        text += &format!("{}\n\n", note.unwrap_or_else(|| "none · w writes one".into()));
        if session.needs_attention() {
            let request = if !session.full_request.is_empty() { session.full_request.clone() }
                else if !session.request.is_empty() { session.request.clone() }
                else { sessions::activity_text(session) };
            text += &format!("ITS ASK\n{request}\n\n");
        }
        if !session.outcome.is_empty() {
            text += &format!("LAST TURN\n{}\n\n", session.outcome);
        }
        if let Some(answers) = self.chief_answers.get(&session.session) {
            let room = (self.width as usize).saturating_sub(6).max(2);
            text += "CHIEF ANSWERS\n";
            for answer in answers.recent.iter().take(3) {
                for line in crate::chief::view::recent_answer_lines(answer, room) { text += &format!("{line}\n"); }
            }
            text += "c all answers\n\n";
        }
        if let TeamLinks::Ready(snapshot) = &self.team_links {
            if let Some(task) = crate::teams::sidebar::member_task(snapshot, session) {
                let member = crate::teams::sidebar::membership(snapshot, session).expect("task came from membership");
                text += &format!("Session: {}\nAgent: {}\nTeam: {} / {}\nRole: {}\n{}\n", session.session, session.agent,
                    task.project, task.task_id, member["role"].as_str().unwrap_or("Member"), crate::teams::sidebar::handoff_summary(snapshot, &task));
                text = crate::approval::escape_hiding(&text, "", "");
            }
        }
        if session.last_result.is_none() && !session.reports_turns { text += "Result tracking unavailable; restart to enable\n\n"; }
        let agent = session.agent();
        let key_groups = SESSION_DETAIL_KEYS.iter().map(|(heading, keys)| (*heading, keys.iter().copied().filter(|(key, _)| {
            let key = Key::Char(key.chars().next().unwrap());
            !(session.is_external() && session_key_requires_terminal(key.clone()))
                && (key != Key::Char('c') || self.chief_answers.contains_key(&session.session))
                && key_feature(key, agent).is_none_or(|(feature, _)| agent.can(feature))
        }).collect())).collect();
        let displayed_updates = match &self.continuity {
            Ok(state) => state.updates().into_iter().filter(|update| update.session == session.session).map(|update| {
                text += &format!("\nUNSEEN UPDATE\n{}\n", update.text);
                update.identity
            }).collect(),
            Err(error) => { text += &format!("\nUpdates unavailable: {error}\n"); Vec::new() }
        };
        self.session_details = Some(SessionDetails { title: format!("{} {}", card.glyph, card.doing), text, session: Some(session.session.clone()),
            key_groups, displayed_updates, ..Default::default() });
    }

    /// The selected session's keys, most pressing first: what its card asks for (an approval, a
    /// restart, /compact, a push…), then the everyday keys. The footer cuts the end of the list
    /// when the column is narrow, so the order is what keeps the useful keys on screen.
    /// What the running copy a card's clone applies to holds and GitHub lacks; 0 for other cards.
    fn running_copy_unpushed(&self, cwd: &str) -> usize {
        sessions::running_copy_of(cwd).and_then(|live| self.running_copies_unpushed.get(&live).copied()).unwrap_or(0)
    }

    pub(crate) fn sessions_hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.selected_session().is_none() && self.sidebar_rows.get(self.panels[1].selected).is_some() {
            return vec![("⏎", "fold/unfold team"), ("n", "Next needing me"), ("/", "find"), ("i", "team details")];
        }
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
        if sessions::is_reviewed_clone(&selected.cwd) && (selected.unpushed > 0 || self.running_copy_unpushed(&selected.cwd) > 0) {
            hints.push(("U", if selected.unpushed > 0 { "apply" } else { "push" }));
        } else if selected.unpushed > 0 {
            hints.push(("U", "push"));
        }
        hints.extend([("⏎", "open"), ("n", "Next needing me"), ("/", "find"), ("i", "details")]);
        if self.chief_answers.contains_key(&selected.session) { hints.push(("c", "Chief answers")); }
        hints.extend([("e", "effort"), ("g", "git"), ("+", "another"), ("x", "exit")]);
        if !selected.session.is_empty() && selected.is_external() {
            hints.retain(|(key, _)| !session_key_requires_terminal(if *key == "⏎" { Key::Enter } else { Key::Char(key.chars().next().unwrap()) }));
            if self.attention_indices(util::now_secs()).is_empty() {
                hints.retain(|(key, _)| *key != "n");
            }
        }
        let mut seen = std::collections::HashSet::new();
        hints.retain(|(key, _)| seen.insert(*key));
        let agent = selected.agent();
        hints.retain(|(key, _)| key.chars().next().and_then(|key| key_feature(Key::Char(key), agent)).is_none_or(|(feature, _)| agent.can(feature)));
        hints
    }

    fn sessions_help() -> Vec<(&'static str, &'static str)> {
        vec![("⏎", "open the selected session"), ("n", "next session that needs you"),
             ("l", "later: hide this ask for 20 min (never approves)"), ("i", "details of the selected session"),
              ("N", "new session: blank or Claude Design canvas"), ("R", "restart the session"), ("x", "end the session"),
             ("p", "pin: first inside its group"), ("r", "on a blocked card: run the todo it waits on"), ("⌘R", "restart every session that lacks a change (⟳), once it waits"),
             ("B", "after a reboot: bring back the sessions that were running"),
             ("u", "updates: unseen results, questions and archived return notes"),
             ("Q", "queue: what is waiting and why"), ("v", "mark displayed updates reviewed; never answer or approve"),
             ("", "its other keys (effort, push, note, sleep…) are listed in i and work on the card too")]
    }

    pub(crate) fn handle_sessions(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        use crate::teams::sidebar::{self, SidebarRow};
        match key {
            Key::Char('u') => return self.run_continuity_action("updates", screen),
            Key::Char('Q') => return self.run_continuity_action("queue", screen),
            Key::Char('v') => return self.run_continuity_action("reviewed", screen),
            _ => {}
        }
        if key == Key::Char('n') { return self.next_sidebar_attention(); }
        if !self.sidebar_selection_lost {
            if let Some(SidebarRow::Team { task } | SidebarRow::PlannedQa { task }) = self.sidebar_rows.get(self.panels[1].selected).cloned() {
                if key == Key::Char('i') { return self.open_team_view(); }
                if key == Key::Enter && matches!(self.sidebar_rows.get(self.panels[1].selected), Some(SidebarRow::Team { .. })) {
                    let key = sidebar::task_key(&task);
                    if !self.collapsed_teams.remove(&key) { self.collapsed_teams.insert(key); }
                    self.build_sessions();
                    return String::new();
                }
            }
        }
        let session = self.selected_session();
        if let Some(s) = session.as_ref().filter(|s| s.restarting) && matches!(key, Key::Enter | Key::Char('r' | 'R' | 'e' | '$' | 'C' | 'x' | 'z')) {
            return format!("{} is restarting: wait for it to come back", s.project);
        }
        if session.as_ref().is_some_and(|session| session.is_external()) && session_key_requires_terminal(key.clone()) {
            return "external session: no terminal target; use its own window".into();
        }
        if let Some(s) = session.as_ref() && let Some((feature, what)) = key_feature(key.clone(), s.agent()) {
            let agent = s.agent();
            if !agent.can(feature) {
                return format!("{what} is not {}'s to do: {} runs {}, do it in its pane", agent.label, s.project, agent.name);
            }
        }
        // On a blocked card `r` runs the todo it waits on; on the others it still restarts (as `R`).
        let blocker = if key == Key::Char('r') {
            session.as_ref().filter(|s| self.session_dependency_activity(s).is_some_and(|(badge, _)| badge != "Waiting on TODO"))
                .and_then(|s| self.stopped_blocker(s))
        } else { None };
        if let Some(item) = blocker {
            if item.status() == todo::PROPOSED { return "its todo is still a proposal: approve it in Ideas first".into(); }
            let title = util::collapse(&item.text());
            if !self.confirm(screen, &format!("run its todo \"{title}\" now? a new session starts on it")) { return String::new(); }
            return self.hand_to_new_session(&Entry::Typed(item), screen);
        }
        let known = session.as_ref().filter(|s| s.state != "unknown").cloned();
        match key {
            Key::Enter => session.map(|s| {
                let opened = self.manager_async(&["open", &s.session]);
                self.mark_result_seen(&s.session).unwrap_or(opened)
            }).unwrap_or_default(),
            Key::Char('i') => { self.open_session_details(); String::new() }
            Key::Char('c') => match session {
                Some(s) => self.open_chief_answers(Some(&s.session)),
                None => String::new(),
            },
            Key::Char('p') => {
                if let Some(s) = known {
                    self.view.toggle_pin(&s.session);
                }
                String::new()
            }
            Key::Char('l') => self.toggle_later(),
            Key::Char('t') => match session {
                Some(s) => self.open_task_trail_of_session(&s.session, screen),
                None => String::new(),
            },
            Key::Char('w') => match session {
                Some(s) => self.write_note(&s, screen),
                None => String::new(),
            },
            Key::Char('m') => self.toggle_remote(),
            Key::Char('d') => self.drop_queued(screen),
            Key::Char('k') => self.open_team_view(),
            Key::Char('r' | 'R') => match known {
                Some(s) if self.confirm(screen, &format!("restart {}? same conversation, new process", s.project)) => {
                    manager_in_background(&["restart", &s.session]);
                    format!("restarting {} …", s.project)
                }
                _ => String::new(),
            },
            Key::Ctrl('r') => self.restart_all(screen),
            Key::Char('e') => match session {
                Some(s) if s.agent().can(Feature::Variants) => self.manager_async(&["variants", &s.session]),
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
                    let _ = util::spawn_detached(Command::new("xdg-open").arg(&design.url));
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
            Key::Char('z') => match known {
                Some(s) => self.sleep_now(&s),
                None => String::new(),
            },
            _ => String::new(),
        }
    }

    /// Restart every session sitting at its prompt now. One that works or asks for a permission
    /// would lose its turn: it restarts once that turn ends (dispatch_restarts). The session that
    /// asked for the apply is always one of those, still in its turn when the apply lands.
    pub(crate) fn restart_all(&mut self, screen: &mut dyn Screen) -> String {
        // Only the sessions started before a change they do not have yet (the "⟳ restart" badge);
        // a terminal session is its terminal's and is left alone.
        let stale: Vec<&Session> = self.running_sessions.iter()
            .filter(|s| !s.is_external() && !s.terminal && !sessions::changed_since_start(s, &self.start_files).is_empty()).collect();
        let ready: Vec<Session> = stale.iter().filter(|s| s.state == "waiting").map(|s| (*s).clone()).collect();
        let busy: Vec<Session> = stale.iter().filter(|s| s.state == "working" || s.state == "needs-you").map(|s| (*s).clone()).collect();
        if ready.is_empty() && busy.is_empty() {
            return "no session needs a restart".into();
        }
        let plural = |count: usize| if count > 1 { "s" } else { "" };
        let now_part = if ready.is_empty() { String::new() } else { format!("restart {} session{} now", ready.len(), plural(ready.len())) };
        let later_part = if busy.is_empty() { String::new() } else { format!("{} busy one{} at the end of its turn", busy.len(), plural(busy.len())) };
        let question = match (now_part.is_empty(), later_part.is_empty()) {
            (false, false) => format!("{now_part}, {later_part}?"),
            (true, _) => format!("restart {later_part}?"),
            (_, true) => format!("{now_part}?"),
        } + " same conversations, new processes";
        if !self.confirm(screen, &question) {
            return String::new();
        }
        let mut queued: Vec<&str> = Vec::new();
        let mut not_queued: Vec<String> = Vec::new();
        for session in &busy {
            match prompts::want_restart(&session.session) {
                Ok(()) => queued.push(session.project.as_str()),
                Err(error) => {
                    util::log_event("WARN", &format!("restart-all could not queue {} for the end of its turn: {error}", session.project));
                    not_queued.push(format!("{} ({error})", session.project));
                }
            }
        }
        if !ready.is_empty() {
            let mut arguments = vec!["restart-all"];
            arguments.extend(ready.iter().map(|s| s.session.as_str()));
            manager_in_background(&arguments);
        }
        let mut message = if ready.is_empty() { String::new() } else { format!("restarting {} …", ready.len()) };
        if !queued.is_empty() {
            message += &format!("{}at the end of their turn: {}", if message.is_empty() { "" } else { " " }, queued.join(", "));
        }
        if !not_queued.is_empty() {
            message += &format!("{}not queued, restart by hand: {}", if message.is_empty() { "" } else { " " }, not_queued.join(", "));
        }
        message
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
        if name == "idle-sleep-after" {
            let current = sleep::after_minutes();
            let next = SLEEP_AFTER_CHOICES.iter().copied().find(|minutes| *minutes > current).unwrap_or(SLEEP_AFTER_CHOICES[0]);
            let error = manager(&["setting", name, &next.to_string()]);
            return if error.is_empty() { format!("{label}: {next}") } else { error };
        }
        let selected_model = if name == "opencode-effort" {
            match settings::launch_value("opencode-model") { Ok(model) => model, Err(error) => return error }
        } else { String::new() };
        if name == "opencode-effort" && selected_model.is_empty() { return "pick an OpenCode model before its effort".into(); }
        let menu = self.settings_menu.as_ref().expect("the settings menu is open");
        let Some(catalog) = &menu.catalog else {
            let viewed = current_session();
            let directory = self.running_sessions.iter().find(|session| session.session == viewed).map(|session| session.cwd.clone())
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
            if let Ok(state) = &self.continuity {
                if let Some(expiry) = crate::continuity::DeferralTarget::review(row).and_then(|target| state.deferred_until(&target, util::now_secs())) {
                    right.push(part(format!("Later · back in {}", util::duration((expiry - util::now_secs()).max(0))), style(CYAN)));
                }
            }
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
                              right, item: Some(index), accent: None, fill: None });
        }
        let panel = &mut self.panels[2];
        panel.items = self.review_rows.len();
        panel.clamp();
        panel.lines = lines;
    }

    /// An unfocused Pull requests panel shows a few rows: its border counts all of them.
    fn review_counts(&self) -> Vec<(String, Style)> {
        if self.review_rows.is_empty() {
            return Vec::new();
        }
        let count = |state: fn(&Review) -> bool| self.review_rows.iter().filter(|row| state(row)).count();
        let mut counts = vec![(format!("{} open", self.review_rows.len()), bold(FG_DARK))];
        for (number, label, count_style) in [(count(|row| row.ci == "failing"), "✗", bold(RED)),
                                             (count(|row| row.review == "CHANGES_REQUESTED"), "changes", style(RED)),
                                             (count(|row| row.review == "REVIEW_REQUIRED"), "review", style(YELLOW))] {
            if number > 0 {
                counts.push((format!("{label} {number}"), count_style));
            }
        }
        counts
    }

    fn reviews_subtitle(&self) -> String {
        let scope = self.reviews.repositories.iter().map(|r| r.rsplit('/').next().unwrap_or("")).collect::<Vec<_>>().join(", ");
        let scope = if scope.is_empty() { "awaiting my review".to_string() } else { scope };
        scope + if self.reviews.current().2 { "  ⟳" } else { "" }
    }

    pub(crate) fn handle_reviews(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let row = self.review_rows.get(self.panels[2].selected).cloned();
        match key {
            Key::Char('o') => row.map(|row| open_in_scratchpad("prs", &row.url)).unwrap_or_default(),
            Key::Char('l') => self.toggle_later(),
            Key::Char('t') => self.run_continuity_action("task-trail", screen),
            Key::Enter => match row {
                Some(row) => match self.reviews.checkouts.iter().find(|(repository, _)| repository.eq_ignore_ascii_case(&row.full_repository)).map(|(_, checkout)| checkout) {
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

    fn build_ideas(&mut self) {
        let selected = self.idea_entries.get(self.panels[4].selected).map(todo::Item::id);
        let mut entries: Vec<_> = self.todos.items.iter().filter(|item| item.status() == todo::PROPOSED).cloned().collect();
        entries.sort_by_key(|item| (-item.priority(), item.created()));
        let mut lines = Vec::new();
        if entries.is_empty() {
            lines.push(Line::new(vec![segment(0, "No ideas waiting for your approval.", dim(FG_DARK))]));
        }
        for (index, item) in entries.iter().enumerate() {
            let category = item.category().map_or("Unknown kind", |category| category.label());
            let reason = if item.why().is_empty() { "No reason provided".to_string() } else { util::collapse(&item.why()) };
            for (text, tint) in [(format!("? {}", item.text()), bold(MAGENTA)),
                                 (format!("{} · {category}", item.project()), bold(project_color(&item.project()))),
                                 (self.idea_source(item), style(FG_DARK)),
                                 (format!("Why: {reason}"), style(FG))] {
                lines.push(Line { segments: vec![segment(0, text, tint)], item: Some(index), ..Line::default() });
            }
            lines.push(Line::default());
        }
        let panel = &mut self.panels[4];
        panel.items = entries.len();
        panel.selected = selected.and_then(|id| entries.iter().position(|item| item.id() == id)).unwrap_or(panel.selected);
        panel.clamp();
        panel.lines = lines;
        panel.fixed_top = vec![Line::new(vec![segment(0, "y Approve  n Decline  Enter Details", dim(FG_DARK))])];
        self.idea_entries = entries;
    }

    fn idea_source(&self, item: &todo::Item) -> String {
        let source = item.source();
        let Some(identity) = source.strip_prefix("session ").filter(|identity| !identity.is_empty()) else {
            return "Unknown session".into();
        };
        match self.all_sessions().iter().find(|session| session.session == identity) {
            Some(session) => {
                let label = if session.label.is_empty() { String::new() } else { format!(" · {}", session.label) };
                format!("From {}{label} · {source}", session.project)
            }
            None => format!("{source} · not running"),
        }
    }

    fn handle_ideas(&mut self, key: Key) -> String {
        let Some(item) = self.idea_entries.get(self.panels[4].selected).cloned() else { return String::new(); };
        match key {
            Key::Char('y') | Key::Char('n') => {
                let approved = key == Key::Char('y');
                if approved { self.todos.approve(&item.id()); } else { self.todos.decline(&item.id()); }
                self.build_ideas();
                self.build_todo();
                if approved { "approved: it is in To do now".into() } else { "declined: it will not be proposed again".into() }
            }
            Key::Enter => {
                self.session_details = Some(SessionDetails {
                    title: "Idea".into(),
                    text: format!("{}\n\n{}", self.idea_source(&item), todo_details(&Entry::Typed(item))),
                    scroll: 0, actions: Vec::new(), chief_answers_of: None, ..Default::default()
                });
                String::new()
            }
            _ => String::new(),
        }
    }

    fn build_todo(&mut self) {
        // Like the issues: the viewed session's project only, every project when none is viewed.
        let viewed = current_session();
        if self.todo_jump.as_ref().is_some_and(|(source, _)| *source != viewed) { self.todo_jump = None; }
        if self.saved_todo_scope.as_ref().is_some_and(|scope| scope.viewed_session != viewed) {
            self.saved_todo_scope = None;
        }
        self.todo_scope = self.todo_jump.as_ref().and_then(|(_, key)| self.todos.items.iter().find(|item| item.id() == *key).map(|item| item.project()))
            .or_else(|| self.saved_todo_scope.as_ref().map(|scope| scope.project.clone()))
            .or_else(|| self.running_sessions.iter().find(|s| s.session == viewed).map(|s| s.project.clone())).unwrap_or_default();
        self.todo_tab = self.todo_tabs.get(&self.todo_owner()).copied().unwrap_or(TODO_TAB_OPEN);
        let scope = self.todo_scope.clone();
        let mut entries: Vec<Entry> = self.todos.items.iter()
            .filter(|item| scope.is_empty() || item.project() == scope)
            .map(|item| Entry::Typed(item.clone()))
            .collect();
        for issue in self.reviews.issues() {
            if self.saved_todo_scope.is_some() && issue.repository != scope { continue; }
            let status = self.todos.status_of(&issue.key());
            let priority = todo::issue_priority(&issue.labels);
            entries.push(Entry::Issue { issue, status, priority });
        }
        entries.sort_by(todo::todo_order);
        if let Some(category) = self.todo_category_filter() {
            entries.retain(|entry| matches!(entry, Entry::Typed(item) if item.category() == Some(category)));
        }
        for (index, (_, statuses)) in TODO_TABS.iter().enumerate() {
            self.todo_counts[index] = entries.iter().filter(|entry| statuses.contains(&entry.status().as_str())).count();
        }
        let shown_statuses = TODO_TABS[self.todo_tab].1;
        entries.retain(|entry| shown_statuses.contains(&entry.status().as_str()));
        if self.todo_tab == TODO_TAB_OPEN {
            entries.sort_by_key(|entry| match entry.status().as_str() { "doing" => 0, todo::PROPOSED => 1, _ => 2 });
        }
        let mut lines = Vec::new();
        if entries.is_empty() {
            let empty = if self.todo_category_filter().is_some() {
                "Nothing here matches the kind filter. f changes it; [ ] switch tabs."
            } else { match shown_statuses {
                [_, _, ..] => "Nothing open. ] next tab.",
                [todo::PROPOSED] => "No idea from Claude waiting for you. ] next tab.",
                ["doing"] => "Nothing in progress. ] next tab.",
                ["done"] => "Nothing done in the last 7 days.",
                _ => "Nothing to do. t adds an item for the viewed project; [ ] switch tabs.",
            }};
            lines.push(Line::new(vec![segment(0, empty, dim(FG_DARK))]));
        }
        for (index, entry) in entries.iter().enumerate() {
            let status = entry.status();
            if self.todo_tab == TODO_TAB_OPEN && (index == 0 || entries[index - 1].status() != status) {
                let heading = match status.as_str() { "doing" => "In progress", todo::PROPOSED => "Proposed", _ => "To do" };
                lines.push(Line::new(vec![segment(0, heading, bold(FG_DARK))]));
            }
            let done = status == "done";
            let mark = if done { String::new() } else { todo::priority_mark(entry.priority()) };
            let mark_style = if entry.priority() > todo::PRIORITY_NORMAL { bold(RED) } else { dim(FG_DARK) };
            let start = 19;
            let (glyph, glyph_style) = match status.as_str() {
                "doing" => ("◐", bold(BLUE)),
                todo::PROPOSED => ("?", bold(MAGENTA)),
                "done" => ("✓", dim(GREEN)),
                _ => ("•", bold(YELLOW)),
            };
            let faded = done.then(|| dim(FG_DARK));
            let queued = status == "todo" && self.todos.is_queued(&entry.key());
            let mut right = if queued { vec![part("Q", bold(CYAN))] } else { vec![] };
            if let Entry::Typed(item) = entry {
                if item.is_design() {
                    right = vec![part(if queued { "✎Q" } else { "✎" }, faded.unwrap_or(bold(MAGENTA)))];
                }
                if item.is_blocked_report() {
                    right = vec![part(if queued { "⛔Q" } else { "⛔" }, faded.unwrap_or(bold(RED)))];
                }
                if !item.link().is_empty() {
                    right.insert(0, part("↔", faded.unwrap_or(bold(CYAN))));
                }
                if item.is_waiting() && !done {
                    right = vec![part("waits", bold(YELLOW))];
                }
            }
            let line = match entry {
                Entry::Typed(item) => Line {
                    segments: vec![segment(0, glyph, glyph_style), segment(2, &mark, mark_style),
                                   segment(7, match item.category() { Some(todo::Category::Task) => "", Some(category) => category.label(), None => "Unknown ?" }, faded.unwrap_or(style(if item.category() == Some(todo::Category::Bug) { RED } else { FG_DARK }))),
                                   segment(start, if scope.is_empty() { format!("{} {}", fit(&item.project(), 9), item.text()) } else { item.text() }, faded.unwrap_or(style(FG)))],
                    right, item: Some(index), accent: None, fill: None,
                },
                Entry::Issue { issue, .. } => {
                    let label = format!("{}#{}", issue.repository, issue.number);
                    let labels = issue.labels.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
                    let (glyph, glyph_style) = if status == "todo" { ("◇", style(MAGENTA)) } else { (glyph, glyph_style) };
                    let right = if !right.is_empty() { right } else if !labels.is_empty() { vec![part(labels, dim(FG_DARK))] } else { vec![] };
                    let right = if queued && status == "todo" { vec![part("Q", bold(CYAN))] } else { right };
                    Line {
                        segments: vec![segment(0, glyph, glyph_style), segment(2, &mark, mark_style),
                                       segment(7, &label, faded.unwrap_or(bold(MAGENTA))),
                                       segment(8 + width_of(&label) as u16, &issue.title, faded.unwrap_or(style(FG_DARK)))],
                        right, item: Some(index), accent: None, fill: None,
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

    fn todo_fixed_lines(&mut self, height: u16) {
        let mut tabs = Vec::new();
        let mut column = 0;
        if let Some(scope) = &self.saved_todo_scope {
            let active_width = width_of(&format!(" {} {} ", TODO_TABS[self.todo_tab].0, self.todo_counts[self.todo_tab]));
            let project_room = (self.width as usize).saturating_sub(4 + 10 + active_width + 1);
            let context = format!("saved: {} · ", fit(&scope.project, project_room));
            column = width_of(&context) as u16;
            tabs.push(segment(0, context, bold(CYAN)));
        }
        for index in std::iter::once(self.todo_tab).chain((0..TODO_TABS.len()).filter(|index| *index != self.todo_tab)) {
            let text = format!(" {} {} ", TODO_TABS[index].0, self.todo_counts[index]);
            let tint = if index == self.todo_tab { on(BLACK, BLUE) }
                else if index == TODO_TAB_PROPOSED && self.todo_counts[index] > 0 { on(BLACK, MAGENTA) } else { dim(FG_DARK) };
            tabs.push(segment(column, &text, tint));
            column += width_of(&text) as u16 + 1;
        }
        self.panels[3].fixed_top = vec![Line::new(tabs), Line::new(vec![segment(0, self.todo_subtitle(), dim(FG_DARK))])];
        self.panels[3].fixed_bottom.clear();
        if self.focus != 3 || height < 11 { return; }
        let Some(entry) = self.todo_entries.get(self.panels[3].selected) else { return; };
        let (title, context, notes) = match entry {
            Entry::Typed(item) => (item.text(), todo_context(item), item.notes().lines().find(|line| !line.trim().is_empty()).unwrap_or("No notes · e Edit · Enter Details").to_string()),
            Entry::Issue { issue, status, .. } => (issue.title.clone(), format!("GitHub · {status} · {}#{}", issue.repository, issue.number), issue.url.clone()),
        };
        self.panels[3].fixed_bottom = vec![Line::new(vec![segment(0, title, bold(FG))]), Line::new(vec![segment(0, context, dim(FG_DARK))]), Line::new(vec![segment(0, notes, dim(FG_DARK))])];
    }

    /// Keep the filter ahead of the scope: long project names must not hide it.
    fn todo_subtitle(&self) -> String {
        let scope = if self.todo_scope.is_empty() { "all projects" } else { self.todo_scope.as_str() };
        let queued = self.todos.queued.len();
        let queue = if queued > 0 { format!(" · {queued} queued") } else { String::new() };
        let category = self.todo_category_filter().map(|category| format!("kind: {} · ", category.label())).unwrap_or_default();
        let saved = if self.saved_todo_scope.is_some() { "saved: " } else { "" };
        format!("{category}{saved}{scope}{queue}")
    }

    fn todo_owner(&self) -> String {
        let viewed = current_session();
        self.saved_todo_scope.as_ref().filter(|scope| scope.viewed_session == viewed)
            .map_or(viewed, |scope| scope.owner_session.clone())
    }

    fn todo_category_filter(&self) -> Option<todo::Category> {
        self.todo_categories.get(&self.todo_owner()).copied().flatten()
    }

    fn reveal_todo(&mut self, id: &str) {
        if let Some(category) = self.todo_category_filter() {
            if self.todos.items.iter().any(|item| item.id() == id && item.category() != Some(category)) {
                self.todo_categories.insert(self.todo_owner(), None);
            }
        }
        self.build_todo();
        if let Some(index) = self.todo_entries.iter().position(|entry| entry.key() == id) {
            self.panels[3].selected = index;
        }
    }

    fn show_todo_tab(&mut self, tab: usize) {
        self.todo_tab = tab;
        self.todo_tabs.insert(self.todo_owner(), tab);
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
            Key::Char('T') => self.run_continuity_action("task-trail", screen),
            Key::Char('i') => {
                if let Some(entry) = entry {
                    self.session_details = Some(SessionDetails { title: "Todo details".into(), text: todo_details(&entry),
                        todo: Some(crate::continuity::task_key(&entry)), key_groups: vec![("ACTIONS", vec![("T", "task trail"), ("i", "in progress / back to do")])], ..Default::default() });
                }
                String::new()
            },
            Key::Esc if self.saved_todo_scope.is_some() => {
                self.saved_todo_scope = None;
                self.build_todo();
                String::new()
            }
            Key::Char('t') => {
                let viewed = self.running_sessions.iter().find(|s| s.session == viewed && !s.cwd.is_empty()).cloned();
                let directory = match &viewed {
                    Some(session) => session.cwd.clone(),
                    None => screen.outside(&mut || pick_project()).unwrap_or_default(),
                };
                if directory.is_empty() {
                    return String::new();
                }
                let project = viewed.map(|s| s.project).unwrap_or_else(|| project_name(&directory));
                match TodoComposer::new(project, directory, current_session(), None) {
                    Ok(composer) => self.todo_composer = Some(composer),
                    Err(error) => return format!("cannot protect the TODO draft: {error}"),
                }
                String::new()
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
                Some(Entry::Typed(item)) => {
                    match TodoComposer::new(item.project(), item.cwd(), self.todo_owner(), Some(&item)) {
                        Ok(composer) => self.todo_composer = Some(composer),
                        Err(error) => return format!("cannot protect the TODO draft: {error}"),
                    }
                    String::new()
                }
                Some(Entry::Issue { .. }) => "an issue is edited on GitHub".into(),
                None => String::new(),
            },
            Key::Char('K') => match entry {
                Some(Entry::Typed(item)) => {
                    let Some(chosen) = screen.outside(&mut || pick_todo_kind(item.category(), false)) else { return "unchanged".into() };
                    let Some(category) = todo::Category::parse(&chosen.to_lowercase()) else { return "unchanged: invalid kind".into() };
                    self.todos.set_category(&item.id(), category);
                    self.reveal_todo(&item.id());
                    format!("kind: {}", category.label())
                }
                Some(Entry::Issue { .. }) => "an issue's labels are edited on GitHub".into(),
                None => String::new(),
            },
            Key::Char('m') => match entry {
                Some(Entry::Typed(item)) => {
                    let Some(directory) = screen.outside(&mut || pick_project()).filter(|directory| !directory.is_empty()) else { return "unchanged".into() };
                    let project = project_name(&directory);
                    match self.todos.move_to(&item.id(), &project, &directory) {
                        Ok(()) => {
                            self.build_todo();
                            format!("moved to {project}")
                        }
                        Err(reason) => format!("not moved: {reason}"),
                    }
                }
                Some(Entry::Issue { .. }) => "an issue belongs to its repository".into(),
                None => String::new(),
            },
            Key::Char('f') => {
                let Some(chosen) = screen.outside(&mut || pick_todo_kind(self.todo_category_filter(), true)) else { return String::new() };
                let category = if chosen == "All" { None } else {
                    let Some(category) = todo::Category::parse(&chosen.to_lowercase()) else { return "invalid kind filter".into() };
                    Some(category)
                };
                self.todo_categories.insert(self.todo_owner(), category);
                self.panels[3].selected = 0;
                self.build_todo();
                String::new()
            }
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
            Key::Char('r') => match entry {
                Some(entry) if entry.status() == todo::PROPOSED => "approve it first (y)".into(),
                Some(entry) => self.hand_to_new_session(&entry, screen),
                None => String::new(),
            },
            Key::Char('x') => match entry {
                Some(Entry::Typed(item)) => {
                    let token=self.todos.manual_reservation(&item.id());
                    if let Err(error) = self.todos.remove(&item.id()) { return format!("cannot remove TODO: {error}"); }
                    if let Err(error)=queue::release_manual(&item.cwd(),&item.id(),token.as_deref()) {return error;}
                    "removed".into()
                }
                _ => String::new(),
            },
            Key::Char('d') => entry.map(|entry| self.toggle_todo_status(&entry, "done")).unwrap_or_default(),
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
            Key::Enter => self.handle_todo(Key::Char('i'), screen),
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

    fn toggle_todo_status(&mut self, entry: &Entry, target: &str) -> String {
        let status = if self.todos.status_of(&entry.key()) == target { "todo" } else { target };
        let directory = match entry { Entry::Typed(item) => item.cwd(), Entry::Issue { issue, .. } => self.reviews.checkouts.get(&issue.full_repository).cloned().unwrap_or_default() };
        let mut token = self.todos.manual_reservation(&entry.key());
        if status == "doing" {
            token = Some(match queue::reserve_manual(&directory, &entry.key()) { Ok(token) => token, Err(error) => return error });
            self.todos.record_manual_reservation(&entry.key(), token.as_deref().expect("reservation obtained"));
        }
        if !self.todos.set_status(&entry.key(), status, "") {
            if status == "doing" { if let Err(error) = queue::release_manual(&directory, &entry.key(), token.as_deref()) { return error; } }
            return "cannot mark TODO; check the log".into();
        }
        if status == "doing" { self.todos.record_manual_reservation(&entry.key(), token.as_deref().expect("reservation obtained")); }
        if status != "doing" { if let Err(error) = queue::release_manual(&directory, &entry.key(), token.as_deref()) { return error; } }
        match status { "doing" => "in progress", "done" => "done", _ => "back to do" }.into()
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
        let directory = match start_directory(&directory) { Ok(directory) => directory, Err(error) => return error };
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
        let mut candidates = self.running_sessions.iter().filter(|session| !self.qa_sessions.contains(&session.session) && session.state != "unknown" && match &design_project {
            Some(project) => session.role == "designer" && session.project == *project,
            None => session.cwd == directory && session.role != "designer",
        });
        let viewed = if is_design { None } else { self.viewed_session_for(entry) };
        let running = viewed.or_else(|| candidates.clone().find(|session| !session.is_external()).cloned());
        if running.is_none() && candidates.any(|session| session.is_external()) {
            return "external session: no terminal target; use its own window".into();
        };
        let text = if is_design && running.is_none() { format!("Load the figcraft-design skill first. {text}") } else { text };
        let agent=if running.is_none() {match settings::launch_agent() {Ok(agent)=>Some(agent),Err(error)=>return error}} else {None};
        let token=match queue::reserve_task(&directory,&key,running.as_ref().map(|session|session.session.as_str()),false) {Ok(token)=>token,Err(error)=>return error};
        let outcome = match &running {
            Some(session) => {
                if let Err(error)=queue::launch_reserved(&token,&["say", &session.session, &text],false) {return error;}
                format!("sent to {}", session.project)
            }
            None => {
                let agent = agent.expect("launch agent validated before reservation");
                // Out of sight, as a card of its own (cockpit background): in the session pane
                // it replaced the session on screen, and the Todo panel lost its project.
                if let Err(error)=queue::launch_reserved(&token,&["--agent", &agent, "background", &directory, &text],true) {return error;}
                format!("starting a session in {}", project_name(&directory))
            }
        };
        if !self.todos.set_status(&key, "doing", running.as_ref().map(|s| s.session.as_str()).unwrap_or("")) {
            return format!("{outcome}; cannot save TODO status");
        }
        if let Err(error) = self.record_task_assignment(entry, running.as_ref()) {
            self.continuity_error = Some(error.clone());
            return format!("{outcome}; task trail unavailable: {error}");
        }
        outcome
    }

    /// The item to a new session in a parallel clone of its repository, on a branch named after it
    /// (cockpit parallel), never to a running one: it does not touch the files a session
    /// already works on. Then it is in progress.
    pub fn hand_to_new_session(&mut self, entry: &Entry, _screen: &mut dyn Screen) -> String {
        let directory = match entry {
            Entry::Typed(item) => item.cwd(),
            Entry::Issue { issue, .. } => self.reviews.checkouts.get(&issue.full_repository).cloned().unwrap_or_default(),
        };
        if directory.is_empty() {
            return "no local checkout of this issue's repository: clone it first".into();
        }
        let directory = match start_directory(&directory) { Ok(directory) => directory, Err(error) => return error };
        let agent = match settings::launch_agent() { Ok(agent) => agent, Err(error) => return error };
        let key = entry.key();
        let (branch, text) = todo::handoff_of(entry);
        let token=match queue::reserve_task(&directory,&key,None,false) {Ok(token)=>token,Err(error)=>return error};
        if let Err(error)=queue::launch_reserved(&token,&["--agent", &agent, "parallel", &directory, &branch, &text],true) {return error;}
        if !self.todos.set_status(&key, "doing", "") {
            return "starting a new session; cannot save TODO status".into();
        }
        if let Err(error) = self.record_task_assignment(entry, None) { self.continuity_error = Some(error); }
        format!("starting a new session in a clone of {} (branch {branch})", project_name(&directory))
    }

    /// The session in the session pane, when it can take this item: running, not the designer, and
    /// on the item's project (by name, as the Todo list is narrowed) or the issue's repository.
    fn viewed_session_for(&self, entry: &Entry) -> Option<Session> {
        let viewed = current_session();
        let session = self.running_sessions.iter().find(|s| !s.is_external() && !self.qa_sessions.contains(&s.session)
            && s.session == viewed && s.state != "unknown" && s.role != "designer")?;
        let belongs = match entry {
            Entry::Typed(item) => item.project() == session.project,
            Entry::Issue { issue, .. } => self.reviews.repositories.contains(&issue.full_repository),
        };
        belongs.then(|| session.clone())
    }

    // ---- The whole column ----

    fn panel_hints(&self, index: usize) -> Vec<(&'static str, &'static str)> {
        match index {
            0 => {
                let mut hints = vec![("Esc", "close"), ("p", "pull requests"), ("c", "chief"), ("l", "answer log")];
                if !self.status_selection_lost && let Some(Some(action)) = self.status_actions.get(self.panels[0].selected) {
                    hints.insert(0, ("⏎", action.hint()));
                }
                hints
            },
            1 => self.sessions_hints(),
            2 => vec![("⏎", "review"), ("o", "GitHub"), ("l", "later / bring back"), ("t", "trail"), ("Esc", "close")],
            4 => vec![("y", "approve"), ("n", "decline"), ("⏎", "details")],
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
        let mut hints = match status {
            todo::PROPOSED => vec![("y", "approve"), ("n", "decline"), ("⏎", "details"), ("e", "edit"), ("[ ]", "tabs")],
            "doing" => vec![("d", "done"), ("i", "details"), ("T", "trail"), ("e", "edit"), ("t", "add"), ("[ ]", "tabs")],
            "done" => vec![("⏎", "details"), ("d", "back to to do"), ("t", "add"), ("[ ]", "tabs")],
            _ => vec![("r", "run now"), ("h", "to a session"), ("i", "details"), ("T", "trail"), ("Q", "queue"), ("d", "done"), ("e", "edit"), ("t", "add"), ("[ ]", "tabs")],
        };
        hints.extend([("K", "kind"), ("m", "move"), ("f", "filter kind")]);
        hints
    }

    fn panel_help(index: usize) -> Vec<(&'static str, &'static str)> {
        match index {
            0 => vec![("⏎", "open selected service status, daily digest or clones review"), ("↑↓", "select a service or utility"), ("p", "its pull requests"), ("c", "the Chief: on / off"), ("l", "the Chief's answer log"), ("s", "back to the tab")],
            1 => Self::sessions_help(),
            2 => vec![("o", "open the pull request in the pull requests window"),
                      ("⏎", "a session reviewing it, in its own clone of the repository, using the default agent"),
                      ("l", "later for 20 minutes / bring back now; never marks reviewed"), ("t", "task trail for this pull request"),
                      ("P", "the pull requests window on the viewed session's repository (or your review requests)"),
                      ("✓ ● ✗", "CI passing, running, failing"), ("approved", "review state: approved, changes (requested), review (required)")],
            4 => vec![("y", "approve the selected idea: it moves to its project's To do, without starting work"),
                      ("n", "decline the selected idea; the declined title will not be proposed again"),
                      ("⏎", "the full idea, reason, notes and source session from any project")],
            _ => vec![("⏎", "the item's details: title, notes, why Claude proposed it, who filed it, when"),
                      ("[ ]", "previous / next tab: Open (Proposed, To do and In progress together) · Proposed · To do · In progress · Done"),
                      ("h", "hand the item over: the session on screen if it is on the item's project (a parallel clone too), Claude Code or opencode alike, else the project's running session, or a new one using the default agent"),
                      ("r", "run now: a new session on it using the default agent, in a parallel clone of its repository on a branch named after it (todo-<id>, issue-<n>), never a running one"),
                      ("e", "edit Kind, Title and Notes in one form; Tab / Shift+Tab fields, arrows choose Kind; CMD+S saves, Esc cancels"),
                      ("K", "change kind: Task, Feature, Bug, Improvement, Chore, Research; Design/Blocked routing stays unchanged"),
                      ("m", "move a typed item to another project, picked like a new session's; not while it is in progress"),
                      ("f", "filter by kind, or All (including GitHub issues); each viewed session keeps its filter"),
                      ("y n", "approve or decline an idea Claude proposed; a declined idea is not proposed again"),
                      ("Q", "queue it (again: unqueue): a session of its project idle for 5 min at its prompt takes the first queued item, by priority, one at a time"),
                      ("t", "add an item for the viewed session's project (or pick a project): Kind, Title and Notes; CMD+S saves, Esc cancels"),
                      ("i", "details; i inside details marks in progress / back to do"), ("T", "task trail (also inside details)"),
                      ("d", "mark done, or back to to do; done items stay 7 days, dimmed at the bottom"),
                      ("+", "raise a typed item's priority: ! then !! up to !!!!"), ("-", "lower it"), ("x", "remove a typed item"),
                      ("! ↓", "raised and low priority; an issue's comes from its labels (P0/P1/urgent/high, P3/P4/low)"),
                      ("? • ◐ ✓", "proposed · to do · in progress · done; ◇ an open GitHub issue still to do"),
                      ("", "Agents mark items with cockpit-todo (start, done, add) and propose ideas (propose)")],
        }
    }

    pub(crate) fn subtitle(&self, index: usize) -> String {
        match index {
            0 => self.status_subtitle(),
            1 => String::new(),
            2 => self.reviews_subtitle(),
            4 => "all projects".into(),
            _ => self.todo_subtitle(),
        }
    }

    /// One view at full height under the header: the focused tab (Sessions, Todo, Ideas), or the
    /// Status overlay, whose pull requests take the lower part.
    pub fn layout(&self, height: u16) -> Vec<u16> {
        let available = height.saturating_sub(HEADER_LINES + 1);
        let mut heights = vec![0; PANEL_NAMES.len()];
        if self.is_status_open() {
            let reviews = (self.panels[2].content_height() as u16 + 2).clamp(MIN_PANEL_LINES, (available / 3).max(MIN_PANEL_LINES));
            heights[2] = reviews.min(available / 2);
            heights[0] = available - heights[2];
        } else {
            heights[self.focus] = available;
        }
        heights
    }

    /// Status (usage, spending, pull requests, services) shows over the tabs: `s` opens it.
    pub(crate) fn is_status_open(&self) -> bool {
        matches!(self.focus, 0 | 2)
    }

    pub fn render(&mut self, buffer: &mut Buffer) {
        self.hit_lines.clear();
        self.hit_actions.clear();
        self.tab_ranges.clear();
        let (width, height) = (buffer.area.width, buffer.area.height);
        self.width = width;
        self.panels[1].fixed_top = self.session_summary_lines();
        self.panels[1].fixed_bottom = self.selected_session_detail_lines();
        self.todo_fixed_lines(self.layout(height)[3]);
        self.draw_header(buffer, width);
        let mut top = HEADER_LINES;
        for (index, panel_height) in self.layout(height).into_iter().enumerate() {
            self.draw_panel(buffer, index, top, panel_height, width);
            top += panel_height;
        }
        self.draw_tabs(buffer, HEADER_LINES, width);
        self.draw_footer(buffer, height.saturating_sub(1), width);
        if let Err(error) = &self.continuity {
            put(buffer, height.saturating_sub(1), 0, &format!("Continuity unavailable: {error}"), bold(RED));
        } else if let Some(error) = &self.continuity_error {
            put(buffer, height.saturating_sub(1), 0, &format!("Continuity unavailable: {error}"), bold(RED));
        }
        if let Some(menu) = self.menu.as_mut() {
            menu.draw(buffer);
        }
        if let Some(menu) = self.settings_menu.as_mut() {
            menu.draw(buffer);
        }
        if let Some(view) = self.chief_view.as_mut().filter(|view| !view.suspended) {
            chief_ui::draw(view, buffer);
            self.hit_lines.clear();
            self.hit_actions.clear();
        }
        if let Some(view) = self.team_view.as_ref() {
            draw_team_view(view, buffer);
            self.hit_lines.clear();
            self.hit_actions.clear();
        }
        if let Some(pairing) = self.pairing.as_ref() {
            pairing.draw(buffer);
        }
        if self.help_open {
            self.draw_help(buffer);
        }
        if let Some(details) = self.session_details.as_mut() {
            details.draw(buffer);
            self.hit_lines.clear();
            self.hit_actions.clear();
        }
        if let Some(composer) = self.todo_composer.as_ref() {
            composer.draw(buffer);
            self.hit_lines.clear();
            self.hit_actions.clear();
        }
        if let Some(form) = self.new_session.as_ref() {
            form.draw(buffer);
            self.hit_lines.clear();
            self.hit_actions.clear();
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
        let inner_height = (height - 2) as usize;
        let subtitle_text = self.subtitle(index);
        let counts = if index == 2 { self.review_counts() } else { Vec::new() };
        let inner = (width - 3) as usize;
        // No boxes: the tabs row sits over a tab's first row; pull requests get a section heading in Status.
        let label = if index == 2 { format!("PULL REQUESTS · {}", self.panels[2].items) } else { String::new() };
        if index == 2 {
            put(buffer, top, 2, &label, bold(if focused { BLUE } else { FG_DARK }));
            self.hit_lines.insert(top, (index, None));
        }
        let panel = &mut self.panels[index];
        let fixed_top_height = panel.fixed_top.len().min(if inner_height >= 4 { 2 } else if inner_height >= 2 { 1 } else { 0 });
        let fixed_bottom_height = if inner_height >= fixed_top_height + panel.fixed_bottom.len() + if index == 3 { 4 } else { 2 } { panel.fixed_bottom.len() } else { 0 };
        let scrolling_height = inner_height.saturating_sub(fixed_top_height + fixed_bottom_height);
        panel.keep_selected_visible(scrolling_height);
        let mut title_end = 2 + width_of(&label);
        let counts_width: usize = counts.iter().map(|(text, _)| 1 + width_of(text)).sum();
        if index == 2 && !counts.is_empty() && title_end + 2 + counts_width + 2 <= width as usize {
            put(buffer, top, title_end as u16, " ·", dim(FG_DARK));
            title_end += 2;
            for (text, count_style) in &counts {
                put(buffer, top, title_end as u16, &format!(" {text}"), *count_style);
                title_end += 1 + width_of(text);
            }
        }
        let subtitle = fit(&subtitle_text, (width as usize).saturating_sub(6 + title_end));
        if index == 2 && !subtitle.is_empty() {
            put(buffer, top, title_end as u16, &format!(" · {subtitle} "), dim(FG_DARK));
        }
        for offset in 0..inner_height {
            let row = top + 1 + offset as u16;
            let line = if offset < fixed_top_height {
                panel.fixed_top.get(offset)
            } else if offset >= fixed_top_height + scrolling_height {
                panel.fixed_bottom.get(offset - fixed_top_height - scrolling_height)
            } else {
                panel.lines.get(panel.scroll + offset - fixed_top_height)
            };
            let Some(line) = line else { continue };
            let mut issue_line;
            let line = if index == 3 && let Some(Entry::Issue { issue, .. }) = line.item.and_then(|item| self.todo_entries.get(item)) {
                // Reserve a title excerpt and Q before allocating repository identity space.
                let marker_room = if line.right.iter().any(|part| part.text == "Q") { 3 } else { 0 };
                let identity_room = (width as usize).saturating_sub(4 + 7 + 1 + 12 + marker_room);
                let number = format!("#{}", issue.number);
                let identity = format!("{}{}", fit(&issue.repository, identity_room.saturating_sub(width_of(&number))), number);
                issue_line = line.clone();
                issue_line.segments[2].1 = identity.clone();
                issue_line.segments[3].0 = 8 + width_of(&identity) as u16;
                &issue_line
            } else { line };
            let highlighted = focused && line.item.is_some() && line.item == Some(panel.selected);
            let recolor = |style: Style, fg: u8| -> Style {
                if !highlighted || line.fill.is_some() { return style; }
                if index == 0 { return style.bg(color(BG_HIGHLIGHT)); }
                let linked_member = index == 1 && line.item.and_then(|item| self.sidebar_rows.get(item)).is_some_and(|row| match row {
                    crate::teams::sidebar::SidebarRow::Session { session_id } => matches!(&self.team_links, TeamLinks::Ready(snapshot)
                        if self.sessions.iter().find(|session| session.session == *session_id).is_some_and(|session| crate::teams::sidebar::membership(snapshot, session).is_some())),
                    _ => false,
                });
                if linked_member { return style.fg(color(FG)).bg(color(BG_HIGHLIGHT)).remove_modifier(Modifier::DIM); }
                let preserve_state = index == 1 && (style.fg == Some(color(RED)) || style.fg == Some(color(BLUE)) || style.fg == Some(color(YELLOW)));
                if preserve_state { style.bg(color(BG_HIGHLIGHT)) } else { style.fg(color(fg)).bg(color(BG_HIGHLIGHT)) }
            };
            if highlighted {
                put(buffer, row, 1, &" ".repeat(inner), on(FG, BG_HIGHLIGHT));
            }
            if let Some((column, fill)) = line.fill {
                put(buffer, row, 2 + column, &" ".repeat(inner.saturating_sub(2 + column as usize)), fill);
            }
            if let Some(accent) = line.accent {
                put(buffer, row, 1, "▌", recolor(accent, BLUE));
            }
            let mut right_width = 0;
            let right_room = if index == 1 && line.item.is_some() {
                let title_column = line.segments.iter().map(|(column, _, _)| *column as usize).max().unwrap_or(0);
                session_card_right_room(width, title_column)
            } else if index == 3 && line.item.is_some() {
                let title_column = line.segments.last().map(|segment| segment.0 as usize).unwrap_or(0);
                (width as usize).saturating_sub(4 + title_column + 12 + 2)
            } else {
                inner.saturating_sub(1)
            };
            let right: Vec<_> = line.right.iter().take_while(|part| {
                let needed = width_of(&part.text) + if right_width == 0 { 0 } else { 2 };
                if right_width + needed > right_room { return false; }
                right_width += needed;
                true
            }).collect();
            for (position, (column, text, tint)) in line.segments.iter().enumerate() {
                let room = (width as i64 - 4 - *column as i64 - if right_width > 0 { right_width as i64 + 2 } else { 0 }).max(0) as usize;
                let room = line.segments.get(position + 1).map_or(room, |next| room.min(next.0.saturating_sub(*column) as usize));
                put(buffer, row, 2 + column, &fit(text, room), recolor(*tint, FG));
            }
            let mut column = (width as i64 - 3 - right_width as i64).max(2) as u16;
            for part in right {
                put(buffer, row, column, &part.text, recolor(part.style, FG_DARK));
                let end = column + width_of(&part.text) as u16;
                if let Some(action) = part.action {
                    self.hit_actions.insert(row, (column, end, action));
                }
                column = end + 2;
            }
            self.hit_lines.insert(row, (index, line.item));
        }
        let hidden_below = panel.lines.len() as i64 - panel.scroll as i64 - scrolling_height as i64;
        if hidden_below > 0 && scrolling_height > 0 {
            let note = format!(" ▼ {hidden_below} ");
            put(buffer, top + height - 1, width - 3 - width_of(&note) as u16, &note, dim(FG_DARK));
        }
        if panel.scroll > 0 {
            let note = format!(" ▲ {} ", panel.scroll);
            put(buffer, top, width - 3 - width_of(&note) as u16, &note, dim(FG_DARK));
        }
    }

    /// One chip per plan with a 4-cell bar of what is left ("Claude Max ▮▮▯▯ 34%", red when it runs
    /// low), then ≈ $ today at API prices for all usage, then the clock. Chips give way first.
    fn draw_header(&mut self, buffer: &mut Buffer, width: u16) {
        let mut chips: Vec<(String, Style)> = Vec::new();
        for (provider, status) in [("Claude", &self.claude_status), ("Codex", &self.codex_status)] {
            let (service, error) = status.snapshot();
            let has_outage = service.as_ref().is_some_and(|summary| summary.indicator != "none"
                || !summary.degraded.is_empty() || !summary.incidents.is_empty()) || !error.is_empty();
            let rows: Vec<&usage::QuotaRow> = self.quota_rows.iter().filter(|row| row.label.split(" · ").next() == Some(provider)).collect();
            let Some(tightest) = rows.iter().min_by_key(|row| (match row.state.as_str() {
                "Blocked" | "Exhausted" => 0, "Low" => 1, "Stale" | "Unknown" => 2, _ => 3,
            }, row.remaining.unwrap_or(100))) else { continue };
            let name = if provider == "Claude" && !self.plan.is_empty() { format!("Claude {}", self.plan) } else { provider.to_string() };
            let is_low = matches!(tightest.state.as_str(), "Low" | "Exhausted" | "Blocked");
            // A fuel gauge of what is left, as Status's LEFT column.
            let chip = match tightest.remaining {
                Some(remaining) if matches!(tightest.state.as_str(), "Available" | "Low") => {
                    let left = remaining.clamp(0, 100);
                    let filled = ((left as f64 / 25.0).ceil() as usize).min(4);
                    format!("{name} {}{} {left}%", "▮".repeat(filled), "▯".repeat(4 - filled))
                },
                _ => format!("{name} {}", tightest.state.to_lowercase()),
            };
            let chip = if has_outage { format!("! {chip}") } else { chip };
            chips.push((chip, if is_low || has_outage { bold(RED) } else { style(FG) }));
        }
        let billing = self.costs.billing_totals();
        let total_today = billing.api.today + billing.subscription.today + billing.unknown.today;
        let cost = if billing.recorded_replies > billing.unpriced_replies && total_today > 0.0 { format!("≈ {} today", money(total_today)) } else { String::new() };
        let clock = chrono::Local::now().format("%H:%M").to_string();
        let mut right = width.saturating_sub(2);
        let mut right_parts: Vec<(String, Style)> = vec![(clock, style(FG_DARK))];
        if !cost.is_empty() { right_parts.push((cost, bold(MAGENTA))); }
        let mut column = 2u16;
        let mut chips_end = column;
        for (text, tint) in &chips {
            let needed = width_of(text) as u16;
            let room_for_right: u16 = right_parts.iter().map(|(text, _)| width_of(text) as u16 + 2).sum();
            if column + needed + room_for_right > width.saturating_sub(1) { break; }
            put(buffer, 0, column, text, *tint);
            column += needed + 2;
            chips_end = column.saturating_sub(2);
        }
        if chips_end > 2 { self.hit_actions.insert(0, (2, chips_end, "status")); }
        for (text, tint) in right_parts {
            let needed = width_of(&text) as u16;
            if right < column + needed { break; }
            right -= needed;
            put(buffer, 0, right, &text, tint);
            right = right.saturating_sub(2);
        }
    }

    /// `[1] Sessions 2  [2] Todo 5  [3] Ideas 3`: key caps, badges (amber only for what needs you),
    /// the open tab on the selection background, its subtitle at the right.
    fn draw_tabs(&mut self, buffer: &mut Buffer, row: u16, width: u16) {
        let needing = self.running_sessions.iter().filter(|session| self.is_actionable(session, util::now_secs())).count() + usize::from(!self.held.is_empty());
        let open_todos = self.todo_counts[TODO_TAB_TO_DO] + self.todo_counts[TODO_TAB_IN_PROGRESS];
        let ideas = self.idea_entries.len();
        put(buffer, row, 0, &" ".repeat(width.saturating_sub(1) as usize), Style::default());
        let mut column = 0u16;
        for (key, panel, name, count, badge) in [("1", 1, "Sessions", needing, on(BLACK, YELLOW)), ("2", 3, "Todo", open_todos, on(FG, BG_HIGHLIGHT)),
                                                  ("3", 4, "Ideas", ideas, on(BLACK, MAGENTA))] {
            let is_open = self.focus == panel;
            let ground = if is_open { on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD) } else { style(FG_DARK) };
            let badge_text = if count > 0 { format!(" {count} ") } else { String::new() };
            let tab_width = 1 + 3 + 1 + width_of(name) as u16 + if badge_text.is_empty() { 0 } else { 1 + width_of(&badge_text) as u16 } + 1;
            if column + tab_width >= width { break; }
            let start = column;
            put(buffer, row, column, &" ".repeat(tab_width as usize), if is_open { ground } else { Style::default() });
            put(buffer, row, column + 1, &format!(" {key} "), on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD));
            put(buffer, row, column + 5, name, ground);
            if !badge_text.is_empty() {
                put(buffer, row, column + 6 + width_of(name) as u16, &badge_text, badge.add_modifier(Modifier::BOLD));
            }
            column += tab_width + 1;
            self.tab_ranges.push((start, column, panel));
        }
        // Todo shows its scope and filter on its own first lines.
        let subtitle = if self.is_status_open() { "Status".to_string() } else if self.focus == 3 { String::new() } else { self.subtitle(self.focus) };
        let has_scroll_note = self.panels[self.focus].scroll > 0;
        let room = (width as usize).saturating_sub(column as usize + 3);
        if !subtitle.is_empty() && !has_scroll_note && room > 6 {
            let text = fit(&subtitle, room);
            put(buffer, row, width.saturating_sub(2 + width_of(&text) as u16), &text, dim(FG_DARK));
        }
        self.hit_lines.remove(&row);
    }

    fn draw_footer(&self, buffer: &mut Buffer, row: u16, width: u16) {
        if !self.message.is_empty() {
            put(buffer, row, 2, &self.message, style(if self.is_confirming { BLUE } else { FG }));
            return;
        }
        let mut column = 2u16;
        let details = if self.focus == 3 && self.saved_todo_scope.is_some() { vec![("Esc", "viewed project"), ("Enter", "Details")] }
            else if self.focus == 3 { vec![("Enter", "Details")] } else { Vec::new() };
        for (key, label) in details.into_iter().chain(self.panel_hints(self.focus)).chain(GLOBAL_HINTS) {
            if column as usize + width_of(key) + width_of(label) + 5 > width as usize {
                break;
            }
            // Every key drawn as a key cap.
            put(buffer, row, column, &format!(" {key} "), on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD));
            column += width_of(key) as u16 + 3;
            put(buffer, row, column, label, dim(FG_DARK));
            column += width_of(label) as u16 + 2;
        }
    }

    /// `?`: the open tab's keys, then the keys that work everywhere (minus those the tab already
    /// lists), then, on Sessions, what the card signs mean.
    pub(crate) fn help_sections(&self) -> (String, Vec<(&'static str, Vec<(&'static str, &'static str)>)>) {
        let tab = if self.is_status_open() { 0 } else { self.focus };
        let this_tab = Self::panel_help(tab);
        let everywhere = GLOBAL_HELP.iter().copied().filter(|(key, _)| !this_tab.iter().any(|(own, _)| own == key)).collect();
        let mut sections = vec![("THIS TAB", this_tab), ("EVERYWHERE", everywhere)];
        if tab == 1 { sections.push(("WHAT THE SIGNS MEAN", LEGEND.to_vec())); }
        (format!("Help · {}", PANEL_NAMES[tab]), sections)
    }

    fn draw_help(&mut self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        let (title, sections) = self.help_sections();
        let box_height = height.saturating_sub(HEADER_LINES + 1);
        let last = HEADER_LINES + box_height.saturating_sub(1);
        for line in 0..box_height {
            put(buffer, HEADER_LINES + line, 0, &" ".repeat(width.saturating_sub(1) as usize), on(FG, BLACK));
        }
        let key_width = sections.iter().flat_map(|(_, entries)| entries.iter()).map(|(key, _)| width_of(key)).max().unwrap_or(1) as u16 + 2;
        let label_width = width.saturating_sub(key_width + 5) as usize;
        let mut lines = vec![Line::new(vec![segment(2, title, bold(FG))])];
        for (heading, entries) in sections {
            lines.push(Line::new(vec![segment(2, heading, bold(FG_DARK))]));
            for (key, label) in entries {
                for (index, piece) in approval::wrap(label, label_width.max(8)).into_iter().enumerate() {
                    let mut parts = Vec::new();
                    if index == 0 && !key.is_empty() {
                        parts.push(segment(2, format!(" {key} "), on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD)));
                    }
                    parts.push(segment(2 + key_width + 1, piece, style(FG_DARK)));
                    lines.push(Line::new(parts));
                }
            }
        }
        let visible = last.saturating_sub(HEADER_LINES + 1) as usize;
        self.help_scroll = self.help_scroll.min(lines.len().saturating_sub(visible));
        for (index, line) in lines.iter().skip(self.help_scroll).take(visible).enumerate() {
            for (column, text, tint) in &line.segments {
                put(buffer, HEADER_LINES + 1 + index as u16, *column, text, *tint);
            }
        }
        put(buffer, last, 2, " esc ", on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD));
        put(buffer, last, 8, "close · ↑↓ scroll", dim(FG_DARK));
    }

    /// Asks a y/n question in the footer; true on y.
    pub fn confirm(&mut self, screen: &mut dyn Screen, question: &str) -> bool {
        self.message = format!("{question}  y/n");
        self.is_confirming = true;
        screen.draw(self);
        let answer = screen.next_key(None);
        self.is_confirming = false;
        self.message.clear();
        answer == Some(Key::Char('y'))
    }

    fn handle_composer(&mut self, key: Key) {
        if let Some(mut composer) = self.todo_composer.take() {
            match composer.handle(key) {
                Outcome::Editing => {
                    if let Err(error) = composer.checkpoint() { composer.error = format!("Draft checkpoint failed: {error}"); }
                    self.todo_composer = Some(composer);
                }
                Outcome::Cancel => {
                    if let Err(error) = composer.cancel() {
                        composer.error = format!("Cannot discard draft: {error}");
                        self.todo_composer = Some(composer);
                    }
                },
                Outcome::Save => {
                    match composer.save(&mut self.todos) {
                        Ok(id) => {
                            let viewed = current_session();
                            self.saved_todo_scope = (composer.owner_session != viewed).then(|| SavedTodoScope {
                                viewed_session: viewed, owner_session: composer.owner_session.clone(), project: composer.project.clone(),
                            });
                            if composer.id.is_none() { self.show_todo_tab(TODO_TAB_TO_DO); }
                            self.reveal_todo(&id);
                            self.message = if self.saved_todo_scope.is_some() { "saved · Esc returns to viewed project" } else { "saved" }.into();
                        }
                        Err(error) => { composer.error = error; self.todo_composer = Some(composer); }
                    }
                }
            }
        }
    }

    fn handle_click(&mut self, x: u16, y: u16, double: bool, screen: &mut dyn Screen) {
        if y == HEADER_LINES {
            if let Some((_, _, panel)) = self.tab_ranges.iter().find(|(start, end, _)| (*start..*end).contains(&x)).copied() {
                self.focus_panel(panel);
                return;
            }
        }
        if let Some((start, end, action @ ("next-attention" | "session-details"))) = self.hit_actions.get(&y).copied() {
            if (start..end).contains(&x) {
                self.focus_panel(1);
                self.message = self.handle_sessions(Key::Char(if action == "next-attention" { 'n' } else { 'i' }), screen);
                return;
            }
        }
        if let Some((start, end, "status")) = self.hit_actions.get(&y).copied() {
            if (start..end).contains(&x) {
                self.focus_panel(if self.focus == 0 { self.previous_panel } else { 0 });
                return;
            }
        }
        let Some((panel_index, item)) = self.hit_lines.get(&y).copied() else { return };
        self.focus_panel(panel_index);
        if let Some(item) = item {
            self.panels[panel_index].selected = item;
            if panel_index == 0 { self.status_selection_lost = false; }
            if panel_index == 1 { self.sidebar_selection_lost = false; }
        }
        if let Some((start, end, "effort")) = self.hit_actions.get(&y).copied() {
            if (start..end).contains(&x) {
                if let Some(session) = self.selected_session() {
                    self.message = self.open_effort_menu(session, Some(y));
                }
                return;
            }
        }
        if double && item.is_some() {
            self.message = self.handle_panel(Key::Enter, screen);
        }
    }

    fn handle_panel(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        match self.focus {
            0 => match key {
                Key::Enter => {
                    if self.status_selection_lost { return "Status selection changed: choose a row".into(); }
                    match self.status_actions.get(self.panels[0].selected).copied().flatten() {
                        Some(StatusAction::ClaudeStatus) => {
                            let _ = util::spawn_detached(Command::new("xdg-open").arg(status::PAGE));
                            "opening status.claude.com …".into()
                        },
                        Some(StatusAction::CodexStatus) => {
                            let _ = util::spawn_detached(Command::new("xdg-open").arg(status::CODEX_PAGE));
                            "opening status.openai.com …".into()
                        },
                        Some(StatusAction::Digest) => self.manager_async(&["digest"]),
                        Some(StatusAction::Clones) => { manager_in_background(&["clones"]); String::new() },
                        None => String::new(),
                    }
                },
                Key::Char('c') => self.open_chief_view(),
                Key::Char('l') => self.open_chief_answers(None),
                Key::Char('h') => {
                    let message=self.open_chief_view();
                    self.handle_chief_key(Key::Char('h'));
                    message
                },
                _ => String::new(),
            },
            1 => self.handle_sessions(key, screen),
            2 => self.handle_reviews(key, screen),
            4 => self.handle_ideas(key),
            _ => self.handle_todo(key, screen),
        }
    }

    pub fn handle(&mut self, key: Key, screen: &mut dyn Screen) {
        if key == Key::Resize { return; }
        if matches!(key, Key::FocusGained | Key::FocusLost) {
            self.has_pane_focus = key == Key::FocusGained;
            return;
        }
        self.message.clear();
        if self.todo_composer.is_some() {
            self.handle_composer(key);
            return;
        }
        if self.new_session.is_some() {
            self.message = self.handle_new_session(key);
            return;
        }
        if matches!(key, Key::Paste(_)) && self.chief_view.as_ref().is_none_or(|view|view.suspended) { return; }
        if let Some(todo) = self.session_details.as_ref().and_then(|details| details.todo.clone()) {
            if key == Key::Char('T') {
                self.message = self.open_task_trail(crate::continuity::TrailTarget::Task(todo), screen);
                return;
            }
            if key == Key::Char('i') {
                self.session_details = None;
                let Some(index) = self.todo_entries.iter().position(|entry| crate::continuity::task_key(entry) == todo) else {
                    self.message = "This todo is no longer available".into();
                    return;
                };
                self.focus_panel(3);
                self.panels[3].selected = index;
                let entry = self.todo_entries[index].clone();
                self.message = self.toggle_todo_status(&entry, "doing");
                return;
            }
        }
        if self.session_details.as_ref().is_some_and(|details| !details.actions.is_empty()) && key == Key::Enter {
            self.message = self.pick_detail_action(screen);
            return;
        }
        // Session details: its keys close it and act on the session, as on the card.
        if let Some(details) = self.session_details.as_ref().filter(|details| details.session.is_some()) {
            let is_detail_key = matches!(&key, Key::Char(character) if details.key_groups.iter()
                .any(|(_, keys)| keys.iter().any(|(own, _)| own.starts_with(*character))));
            if is_detail_key {
                let session = details.session.clone();
                let displayed_updates = details.displayed_updates.clone();
                self.session_details = None;
                if self.selected_session().map(|selected| selected.session) != session {
                    self.message = "the selected session changed: press i on it again".into();
                    return;
                }
                self.message = if key == Key::Char('v') {
                    self.review_updates(session.as_deref().expect("session details have a target"), &displayed_updates)
                } else { self.handle_sessions(key, screen) };
                return;
            }
        }
        if let Some(details) = self.session_details.as_mut() {
            match key {
                Key::Down | Key::Char('j') => details.scroll = details.scroll.saturating_add(1),
                Key::PageUp => details.scroll = 0,
                Key::PageDown => details.scroll = usize::MAX,
                Key::Up | Key::Char('k') => details.scroll = details.scroll.saturating_sub(1),
                Key::Char('g') => details.scroll = 0,
                Key::Char('G') => details.scroll = usize::MAX,
                Key::Esc | Key::Enter | Key::Char('i') => self.session_details = None,
                Key::Char('c') => if let Some(session) = details.chief_answers_of.take() {
                    self.session_details = None;
                    self.message = self.open_chief_answers(Some(&session));
                },
                _ => {}
            }
            return;
        }
        if self.chief_view.is_some() && self.handle_chief_key(key.clone()) {
            return;
        }
        if self.team_view.is_some() {
            self.message = self.handle_team_view(key, screen);
            return;
        }
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
            match key {
                Key::Down | Key::Char('j') => self.help_scroll = self.help_scroll.saturating_add(1),
                Key::Up | Key::Char('k') => self.help_scroll = self.help_scroll.saturating_sub(1),
                _ => self.help_open = false,
            }
            return;
        }
        // CMD+1/2/3 from the session arrive as F1-F3 (tmux.conf).
        if let Key::F(number @ 1..=3) = key {
            self.focus_panel(TAB_PANELS[number as usize - 1]);
            return;
        }
        match key {
            Key::Click { x, y, double } => self.handle_click(x, y, double, screen),
            Key::Char('s') => self.focus_panel(if self.is_status_open() { self.previous_panel } else { 0 }),
            Key::Esc if self.is_status_open() => self.focus_panel(self.previous_panel),
            Key::Char('p') if self.focus == 0 => self.focus_panel(2),
            Key::Char(digit @ '1'..='3') => self.focus_panel(TAB_PANELS[digit as usize - '1' as usize]),
            Key::Char('j') | Key::Down => {
                self.panels[self.focus].move_by(1);
                if self.focus == 0 { self.status_selection_lost = false; }
                if self.focus == 1 { self.sidebar_selection_lost = false; }
            },
            // In Sessions `k` opens the selected card's links (Theo, 2026-10-26); ↑ moves up there.
            Key::Char('k') if self.focus != 1 => {
                self.panels[self.focus].move_by(-1);
                if self.focus == 0 { self.status_selection_lost = false; }
            },
            Key::Up => {
                self.panels[self.focus].move_by(-1);
                if self.focus == 0 { self.status_selection_lost = false; }
                if self.focus == 1 { self.sidebar_selection_lost = false; }
            },
            // From any panel, as ← comes back from the session (tmux.conf).
            Key::Right => { let _ = Command::new("cockpit").arg("focus-viewer").status(); }
            Key::Char('?') => { self.help_open = true; self.help_scroll = 0; }
            Key::Char('N') => self.open_new_session(),
            Key::Char('S') => self.settings_menu = Some(SettingsMenu::new()),
            Key::Char('H') => {
                let past = self.past_sessions();
                if let Some(index) = screen.outside(&mut || pick_past_session(&past)).and_then(|chosen| chosen.parse::<usize>().ok()) {
                    let chosen = &past[index].0;
                    self.message = match queue::reserve_resume(&chosen.cwd, &chosen.session) {
                        Ok(token) => match queue::launch_reserved(&token,&["resume", &chosen.cwd, &chosen.session],true) {Ok(())=>"resuming conversation".into(),Err(error)=>error},
                        Err(error) => error,
                    };
                }
            }
            Key::Char('P') => self.message = self.manager_async(&["prs"]),
            Key::Char('W') => self.message = open_in_scratchpad("claude", ""),
            Key::Char('G') => self.message = open_in_scratchpad("greyproxy", "http://localhost:43080/pending"),
            Key::Char('q') | Key::Ctrl('c') => {
                if self.confirm(screen, "close the manager window? sessions keep running") {
                    let _ = Command::new("tmux").arg("detach-client").status();
                }
            }
            _ => self.message = self.handle_panel(key, screen),
        }
    }

    fn launch_design_session(&mut self, directory: &str, canvas_link: &str) -> Result<String, String> {
        let url = sessions::canvas_address(canvas_link).ok_or_else(|| "not a canvas link: claude.ai/artifact/<id>, or a chat link with ?artifact=<id>".to_string())?;
        manager_in_background(&["new", directory, &sessions::design_prompt(&url)]);
        Ok(format!("starting a design session in {} …", project_name(directory)))
    }

    /// `m` on a card: Remote Control (phone / claude.ai/code) on or off.
    fn toggle_remote(&mut self) -> String {
        match self.selected_session().filter(|s| s.state != "unknown") {
            Some(s) if s.is_external() => "external session: no terminal target; use its own window".into(),
            Some(s) if !s.agent().can(Feature::RemoteControl) => format!("Remote Control is not {}'s to do: {} runs {}", s.agent().label, s.project, s.agent().name),
            Some(s) => {
                manager_in_background(&["remote", &s.session]);
                format!("toggling remote control for {} …", s.project)
            }
            None => String::new(),
        }
    }

    /// `d` on a card: drop the messages queued for it, after a yes.
    fn drop_queued(&mut self, screen: &mut dyn Screen) -> String {
        match self.selected_session().filter(|s| s.state != "unknown") {
            Some(s) if prompts::count(&s.session) == 0 => "no prompt queued for it".into(),
            Some(s) if self.confirm(screen, &format!("drop the {} prompt(s) queued for {}?", prompts::count(&s.session), s.project)) => {
                prompts::clear(&s.session);
                "queued prompts dropped".into()
            }
            _ => String::new(),
        }
    }

    fn team_control(action: crate::teams::HostAction) -> Result<Value, crate::teams::TeamError> {
        crate::teams::client::host_request(&crate::teams::HostRequest::Control { action }).and_then(|reply| reply.result)
    }

    fn team_snapshot() -> Result<Value, crate::teams::TeamError> {
        crate::teams::client::host_request(&crate::teams::HostRequest::Snapshot).and_then(|reply| reply.result)
    }

    /// Every 10 s: what in the linked teams waits for Theo, and each member's role, for the cards.
    pub(crate) fn check_team(&mut self) {
        let now = util::now();
        if now - self.team_checked < 10.0 { return; }
        self.team_checked = now;
        self.chief_answers.clear();
        if self.focus == 0 { self.refresh_chief_status(); }
        if self.chief_view.is_some() { self.refresh_chief_view(); }
        util::refresh_hyprland_instance();
        let workspace = util::output_of("hyprctl", &["-j", "activeworkspace"])
            .and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|workspace|workspace["id"].as_i64());
        if let Some(workspace) = workspace {
            let _ = crate::teams::client::host_request(&crate::teams::HostRequest::ChiefFocus {
                viewed:current_session(), workspace_active:workspace == util::CLAUDE_WORKSPACE, gaming:(self.gaming)(),
            });
        }
        if !paths::team_host_socket().exists() {
            self.team_attention = 0;
            self.team_roles.clear();
            self.team_known = false;
            self.team_links = TeamLinks::Unavailable;
            return;
        }
        let Ok(snapshot) = Self::team_snapshot() else {
            self.team_known = false;
            self.team_roles.clear();
            self.team_attention = 0;
            self.team_links = TeamLinks::Unavailable;
            return;
        };
        if crate::teams::sidebar::grouped_rows(&snapshot, &[], &HashSet::new()).is_err() {
            self.team_known = false;
            self.team_roles.clear();
            self.team_attention = 0;
            self.team_links = TeamLinks::Unavailable;
            return;
        }
        self.team_known = true;
        self.chief_answers = snapshot["chief_answers"].as_array().into_iter().flatten().filter_map(|entry| {
            let count = entry["count"].as_u64().filter(|count| *count > 0)? as usize;
            Some((entry["session_id"].as_str()?.to_string(), ChiefAnswers { count, recent: entry["recent"].as_array().cloned().unwrap_or_default() }))
        }).collect();
        self.team_attention = crate::teams::view::TeamView::attention(&snapshot);
        self.qa_sessions = snapshot["members"].as_array().into_iter().flatten()
            .chain(snapshot["spawns"].as_array().into_iter().flatten())
            .filter(|row| row["role"] == "qa")
            .filter_map(|row| row["session_id"].as_str().map(str::to_string)).collect();
        self.team_roles = snapshot["members"].as_array().into_iter().flatten().filter_map(|member| Some((member["session_id"].as_str()?.to_string(),
            format!("team {} · {}", member["role"].as_str()?, member["task_id"].as_str()?)))).collect();
        self.team_links = TeamLinks::Ready(snapshot);
    }

    fn open_team_view(&mut self) -> String {
        match Self::team_snapshot() {
            Ok(snapshot) => {
                self.team_view = Some(crate::teams::view::TeamView::new(snapshot));
                String::new()
            },
            Err(error) => format!("Team view: {}", error.message),
        }
    }

    /// Re-read after every change; the first pending request is selected when `to_request` says so.
    fn refresh_team_view(&mut self, to_request: bool) {
        let snapshot = match Self::team_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if let Some(view) = self.team_view.as_mut() { view.snapshot_error = Some(crate::approval::escape_hiding(&error.message, "", "")); }
                return;
            },
        };
        if let Some(view) = self.team_view.as_mut() {
            view.snapshot = snapshot;
            view.snapshot_error = None;
            view.detail_scroll = 0;
            let rows = view.rows();
            view.selected = if to_request { 0 } else { view.selected.min(rows.len().saturating_sub(1)) };
        }
    }

    fn handle_team_view(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let Some(view) = self.team_view.as_mut() else { return String::new() };
        if view.snapshot_error.is_some() && (key == Key::Char('l') || (matches!(view.selected_row(), Some(crate::teams::view::Row::LinkProposal(_) | crate::teams::view::Row::Link(_)))
            && matches!(key, Key::Char('a' | 'e' | 'x' | 'u')))) {
            return "Collaboration status unavailable: r refresh before changing links".into();
        }
        match key {
            Key::Esc | Key::Char('q') => {
                self.team_view = None;
                String::new()
            },
            Key::Tab => {
                view.is_history = !view.is_history;
                view.selected = 0;
                view.detail_scroll = 0;
                String::new()
            },
            Key::Char('j') | Key::Down => { view.move_by(1); String::new() },
            Key::Char('k') | Key::Up => { view.move_by(-1); String::new() },
            Key::Right => {
                let lines = view.detail().iter().flat_map(|text| wrap_detail_text(text, self.width.saturating_sub(6).max(2) as usize)).count();
                view.detail_scroll = view.detail_scroll.saturating_add(1).min(lines.saturating_sub(1));
                String::new()
            },
            Key::Left => { view.detail_scroll = view.detail_scroll.saturating_sub(1); String::new() },
            Key::Char('r') => {
                self.refresh_team_view(false);
                self.team_view.as_ref().and_then(|view| view.snapshot_error.as_ref())
                    .map(|error| format!("Collaboration snapshot unavailable: {error}")).unwrap_or_default()
            },
            Key::Char('n') => self.create_team_request(screen),
            Key::Char('l') => self.create_link_request(screen),
            _ => match view.selected_row() {
                Some(row) => self.team_row_action(row, key, screen),
                None => String::new(),
            },
        }
    }

    fn team_row_action(&mut self, row: crate::teams::view::Row, key: Key, screen: &mut dyn Screen) -> String {
        use crate::teams::{HostAction, TaskKey, view::Row};
        let token = || format!("cockpit-{}", util::token_hex(8));
        let view = self.team_view.as_ref().expect("the Team view is open");
        let outcome = match (row, key) {
            (Row::LinkProposal(index), key @ Key::Char('a' | 'e' | 'x')) => return self.link_request_action(index, key, screen),
            (Row::Link(index), Key::Char('u')) if view.link(index)["state"] == "active" => {
                let link_id = view.link(index)["link_id"].as_str().unwrap_or("").to_string();
                if !self.confirm(screen, &format!("revoke link {link_id}? queued messages and safe retries are cancelled; submitted messages cannot be recalled")) { return String::new() }
                return self.finish_link_action(HostAction::LinkRevoke { request_id:token(), link_id }, "link revoked; queued messages and safe retries cancelled");
            },
            (Row::Proposal(index), Key::Char('a')) => return self.approve_team_request(index, screen),
            (Row::Proposal(index), Key::Char('e')) => return self.edit_team_request(index, screen),
            (Row::Proposal(index), Key::Char('x')) => {
                let proposal = view.proposal(index).clone();
                if !self.confirm(screen, "reject this team request? no links are made") { return String::new() }
                Self::team_control(HostAction::Reject { request_id: token(), proposal_id: proposal["proposal_id"].as_str().unwrap_or("").into(),
                    expected_revision: proposal["revision"].as_i64().unwrap_or(0) }).map(|_| "request rejected: no links were made".to_string())
            },
            (Row::Member(index), Key::Char('u')) => {
                let member = view.member(index).clone();
                let text = |key: &str| member[key].as_str().unwrap_or("").to_string();
                if !self.confirm(screen, &format!("unlink {} {}? its pending messages and retries are cancelled", text("role"), text("session_id"))) {
                    return String::new();
                }
                Self::team_control(HostAction::Unlink { request_id: token(), task: TaskKey { project: text("project"), task_id: text("task_id") },
                    session_id: text("session_id") }).map(|_| "unlinked: it can no longer message the team".to_string())
            },
            (Row::Escalation(index), Key::Char('s')) => {
                let thread_id = view.escalation(index)["thread_id"].as_str().unwrap_or("").to_string();
                let note = input_line(screen, "resolution note for the team (empty cancels)> ");
                if note.trim().is_empty() { return String::new() }
                Self::team_control(HostAction::Resolve { request_id: token(), thread_id, note }).map(|_| "escalation resolved".to_string())
            },
            (Row::FailedStart(index), Key::Char('d')) => {
                let spawn_token = view.snapshot["spawns"][index]["token"].as_str().unwrap_or("").to_string();
                if !self.confirm(screen, "dismiss this team start? it is not started again either way") { return String::new() }
                Self::team_control(HostAction::Dismiss { request_id: token(), token: spawn_token }).map(|_| "dismissed".to_string())
            },
            (Row::Uncertain(index), Key::Char(choice @ ('h' | 'c'))) => {
                let message_id = view.message(index)["message_id"].as_str().unwrap_or("").to_string();
                let (decision, question) = if choice == 'h' { ("handled", "it reached the session: mark it handled?") }
                                           else { ("cancel", "cancel this delivery? it is not sent again") };
                if !self.confirm(screen, question) { return String::new() }
                Self::team_control(HostAction::Reconcile { request_id: token(), message_id, decision: decision.into() })
                    .map(|_| format!("delivery {decision}"))
            },
            (Row::FailedDelivery(index), Key::Char('c')) => {
                let message_id = view.message(index)["message_id"].as_str().unwrap_or("").to_string();
                if !self.confirm(screen, "cancel this failed delivery? it is not sent again") { return String::new() }
                Self::team_control(HostAction::Reconcile { request_id: token(), message_id, decision: "cancel".into() })
                    .map(|_| "failed delivery cancelled".to_string())
            },
            _ => return String::new(),
        };
        self.refresh_team_view(false);
        outcome.unwrap_or_else(|error| error.message)
    }

    fn finish_link_action(&mut self, action: crate::teams::HostAction, success: &str) -> String {
        let outcome = Self::team_control(action);
        self.refresh_team_view(false);
        match outcome {
            Ok(receipt) => {
                if let Some(view) = self.team_view.as_mut() {
                    if receipt["status"] == "pending" { view.is_history = false; }
                    if let Some(index) = view.rows().iter().position(|row| match row {
                        crate::teams::view::Row::LinkProposal(index) => view.link_proposal(*index)["proposal_id"] == receipt["proposal_id"],
                        crate::teams::view::Row::Link(index) => view.link(*index)["link_id"] == receipt["link_id"],
                        _ => false,
                    }) {
                        view.selected = index;
                    }
                }
                success.into()
            },
            Err(error) => format!("Link action {}: {}. Refresh to inspect current status; no automatic retry.",
                if error.code == crate::teams::ErrorCode::Unavailable { "outcome unknown" } else { "failed" },
                crate::approval::escape_hiding(&error.message, "", "")),
        }
    }

    fn link_request_action(&mut self, index: usize, key: Key, screen: &mut dyn Screen) -> String {
        use crate::teams::HostAction;
        let proposal = self.team_view.as_ref().expect("collaboration view open").link_proposal(index).clone();
        if proposal["status"] != "pending" { return String::new() }
        let Some(proposal_id) = proposal["proposal_id"].as_str() else { return "Link request identity unavailable".into() };
        let Some(expected_revision) = proposal["revision"].as_i64() else { return "Link request revision unavailable".into() };
        let request_id = format!("cockpit-{}", util::token_hex(8));
        let (action, success) = match key {
            Key::Char('a') => {
                let Some(mapping_hash) = proposal["mapping_hash"].as_str() else { return "Link request hash unavailable".into() };
                if !self.confirm(screen, "approve exactly this link revision/hash, pair, TODO and purpose? messaging only; no team role") { return String::new() }
                (HostAction::LinkApprove {request_id,proposal_id:proposal_id.into(),expected_revision,mapping_hash:mapping_hash.into()},
                    "link approved: messaging-only for this exact pair and shared TODO")
            },
            Key::Char('x') => {
                if !self.confirm(screen, "reject this exact link request? no messaging permission is granted") { return String::new() }
                (HostAction::LinkReject {request_id,proposal_id:proposal_id.into(),expected_revision},"link request rejected")
            },
            Key::Char('e') => {
                let Some(requester) = proposal["sender"].as_str() else { return "Link requester unavailable".into() };
                let replacement = match self.pick_link(screen,Some(requester)) {
                    Ok(Some((_, replacement))) => replacement, Ok(None) => return String::new(), Err(error) => return error,
                };
                (HostAction::LinkEdit {request_id,proposal_id:proposal_id.into(),expected_revision,replacement},
                    "edited: the new revision needs its own approval (a)")
            },
            _ => return String::new(),
        };
        self.finish_link_action(action,success)
    }

    /// Only the host's verified live registry is offered, never free-form participant IDs.
    fn pick_link(&mut self, screen: &mut dyn Screen, requester: Option<&str>) -> Result<Option<(String, crate::teams::LinkProposal)>, String> {
        use crate::teams::{LinkProposal, TaskKey};
        let snapshot = Self::team_snapshot().map_err(|error| error.message)?;
        let sessions = snapshot["sessions"].as_array().ok_or("verified live sessions unavailable")?;
        let display = |value: &str| crate::approval::escape_hiding(value, "", "").replace(['\n','\r','\t'], " ");
        let choices = |exclude: Option<&str>| sessions.iter().filter(|session| session["session_id"].as_str() != exclude)
            .filter_map(|session| Some(format!("{}\t{} · {} · {}", session["session_id"].as_str()?,
                display(session["agent"].as_str()?), display(session["project"].as_str()?), display(session["label"].as_str().unwrap_or(""))))).collect::<Vec<_>>();
        let requester = if let Some(requester) = requester { requester.to_string() } else {
            let Some(picked) = screen.outside(&mut || fzf("link first session> ", &choices(None), &["--delimiter", "\t", "--with-nth", "1.."])) else { return Ok(None) };
            picked.split('\t').next().unwrap_or("").to_string()
        };
        let first = sessions.iter().find(|session| session["session_id"] == requester).ok_or("requester is not a verified live session")?;
        let Some(picked) = screen.outside(&mut || fzf("link peer session> ", &choices(Some(&requester)), &["--delimiter", "\t", "--with-nth", "1.."])) else { return Ok(None) };
        let peer_id = picked.split('\t').next().unwrap_or("");
        let peer = sessions.iter().find(|session| session["session_id"] == peer_id && peer_id != requester).ok_or("peer is not a distinct verified live session")?;
        let owners: Vec<String> = [first, peer].iter().filter_map(|session| session["project"].as_str()).map(|project| format!("{project}\t{project}")).collect();
        let Some(owner) = screen.outside(&mut || fzf("shared TODO owning project> ", &owners, &["--delimiter", "\t", "--with-nth", "2.."])) else { return Ok(None) };
        let owner = owner.split('\t').next().unwrap_or("");
        if ![first, peer].iter().any(|session| session["project"] == owner) { return Err("TODO owner must be one of the verified participant projects".into()) }
        let task_id = input_line(screen,"shared local TODO id (empty cancels)> ");
        if task_id.trim().is_empty() { return Ok(None) }
        let purpose = input_line(screen,"link purpose (empty cancels)> ");
        if purpose.trim().is_empty() { return Ok(None) }
        Ok(Some((requester,LinkProposal {submission_key:format!("host-{}",util::token_hex(8)),
            task:TaskKey {project:owner.into(),task_id:task_id.trim().into()},peer_session_id:peer_id.into(),purpose})))
    }

    fn create_link_request(&mut self, screen: &mut dyn Screen) -> String {
        let (requester_session_id, proposal) = match self.pick_link(screen,None) {
            Ok(Some(request)) => request, Ok(None) => return String::new(), Err(error) => return error,
        };
        self.finish_link_action(crate::teams::HostAction::LinkPropose {request_id:format!("cockpit-{}",util::token_hex(8)),requester_session_id,proposal},
            "link request created: review its exact mapping, then a approves it")
    }

    fn approve_team_request(&mut self, index: usize, screen: &mut dyn Screen) -> String {
        use crate::teams::{HostAction, Member, Proposal, Role, TaskKey};
        let view = self.team_view.as_ref().expect("the Team view is open");
        let proposal = view.proposal(index).clone();
        let mapping = view.mapping(index);
        let text = |key: &str| proposal[key].as_str().unwrap_or("").to_string();
        if !self.confirm(screen, "approve exactly the request shown?") { return String::new() }
        let approval = HostAction::Approve { request_id: format!("cockpit-{}", util::token_hex(8)), proposal_id: text("proposal_id"),
            expected_revision: proposal["revision"].as_i64().unwrap_or(0), mapping_hash: text("mapping_hash") };
        match Self::team_control(approval) {
            Ok(_) => {
                self.refresh_team_view(false);
                "team approved: only these sessions can message each other about this task".into()
            },
            Err(error) if error.message.contains("runtime changed") => {
                // Restarted members: the same task, sessions and roles, bound to their new launches.
                let members = mapping.iter().filter_map(|member| Some(Member {
                    session_id: member["session_id"].as_str()?.to_string(),
                    role: serde_json::from_value::<Role>(member["role"].clone()).ok()?,
                })).collect();
                let replacement = Proposal { submission_key: format!("rebind-{}", util::token_hex(8)),
                    task: TaskKey { project: text("project"), task_id: text("task_id") }, members, reason: text("reason") };
                let edit = HostAction::Edit { request_id: format!("cockpit-{}", util::token_hex(8)), proposal_id: text("proposal_id"),
                    expected_revision: proposal["revision"].as_i64().unwrap_or(0), replacement };
                match Self::team_control(edit) {
                    Ok(_) => {
                        self.refresh_team_view(true);
                        "the sessions restarted: review the same request again, then a approves it".into()
                    },
                    Err(error) => error.message,
                }
            },
            Err(error) => error.message,
        }
    }

    /// Task id, then a session per role among the verified live ones of one project ("-" for none).
    fn pick_team(&mut self, screen: &mut dyn Screen) -> Result<Option<(crate::teams::TaskKey, Vec<crate::teams::Member>)>, String> {
        use crate::teams::{Member, Role, TaskKey};
        let task_id = input_line(screen, "task id for the team (todo id or owner/repo#N, empty cancels)> ");
        if task_id.trim().is_empty() { return Ok(None) }
        let sessions = self.team_view.as_ref().expect("the Team view is open").sessions().to_vec();
        let (mut project, mut members): (Option<String>, Vec<Member>) = (None, Vec::new());
        for (role, name) in [(Role::Designer, "designer"), (Role::Developer, "developer"), (Role::Qa, "qa")] {
            let choices: Vec<String> = sessions.iter()
                .filter(|session| project.as_ref().is_none_or(|project| session["project"] == project.as_str()))
                .filter(|session| !members.iter().any(|member| session["session_id"] == member.session_id.as_str()))
                .map(|session| format!("{}\t{}  {}  {}", session["session_id"].as_str().unwrap_or(""), session["agent"].as_str().unwrap_or(""),
                    session["project_name"].as_str().unwrap_or(""), session["label"].as_str().unwrap_or("")))
                .chain(std::iter::once("-\t(no one)".to_string())).collect();
            let Some(picked) = screen.outside(&mut || fzf(&format!("{name}> "), &choices, &["--delimiter", "\t", "--with-nth", "2.."])) else { return Ok(None) };
            let id = picked.split('\t').next().unwrap_or("").to_string();
            if id == "-" { continue }
            let session = sessions.iter().find(|session| session["session_id"] == id.as_str()).ok_or("that session is not a verified live one")?;
            project.get_or_insert_with(|| session["project"].as_str().unwrap_or("").to_string());
            members.push(Member { session_id: id, role });
        }
        if members.len() < 2 { return Err("a team needs at least two members".into()) }
        Ok(Some((TaskKey { project: project.unwrap_or_default(), task_id: task_id.trim().into() }, members)))
    }

    fn create_team_request(&mut self, screen: &mut dyn Screen) -> String {
        use crate::teams::{HostAction, Proposal};
        let (task, members) = match self.pick_team(screen) { Ok(Some(team)) => team, Ok(None) => return String::new(), Err(error) => return error };
        let proposal = Proposal { submission_key: format!("host-{}", util::token_hex(8)), task, members, reason: "Created by Theo in the cockpit".into() };
        match Self::team_control(HostAction::Propose { request_id: format!("cockpit-{}", util::token_hex(8)), proposal }) {
            Ok(_) => {
                self.refresh_team_view(true);
                "request created: review its exact mapping, then a approves it".into()
            },
            Err(error) => error.message,
        }
    }

    fn edit_team_request(&mut self, index: usize, screen: &mut dyn Screen) -> String {
        use crate::teams::{HostAction, Proposal};
        let proposal = self.team_view.as_ref().expect("the Team view is open").proposal(index).clone();
        let (task, members) = match self.pick_team(screen) { Ok(Some(team)) => team, Ok(None) => return String::new(), Err(error) => return error };
        let replacement = Proposal { submission_key: format!("edit-{}", util::token_hex(8)), task, members,
            reason: proposal["reason"].as_str().unwrap_or("").to_string() };
        let edit = HostAction::Edit { request_id: format!("cockpit-{}", util::token_hex(8)), proposal_id: proposal["proposal_id"].as_str().unwrap_or("").into(),
            expected_revision: proposal["revision"].as_i64().unwrap_or(0), replacement };
        match Self::team_control(edit) {
            Ok(_) => {
                self.refresh_team_view(true);
                "edited: the new revision needs its own approval (a)".into()
            },
            Err(error) => error.message,
        }
    }

    /// Past sessions not running now, newest first, with their titles.
    /// Claude's past sessions and opencode's, newest first, the running ones left out.
    pub(crate) fn past_sessions(&mut self) -> Vec<(PastSession, String)> {
        self.history.refresh();
        let running: Vec<&str> = self.running_sessions.iter().map(|s| s.session.as_str()).collect();
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

fn provider_status_lines(status: &ProviderStatus, room: usize) -> Vec<Line> {
    let (current, error) = status.snapshot();
    let label = status.provider.label().trim_end_matches(" Status");
    let column = width_of(label) + 2;
    let mut lines = Vec::new();
    let Some(current) = current else {
        let text = if !error.is_empty() { error } else if status.provider.url().is_empty() { "not checked".into() } else { format!("checking {} …", status.provider.host()) };
        for (index, text) in wrap_detail_text(&text, room.saturating_sub(column).max(2)).into_iter().enumerate() {
            let mut segments = Vec::new();
            if index == 0 { segments.push(segment(0, label, style(FG))); }
            segments.push(segment(column as u16, text, dim(FG_DARK)));
            lines.push(Line::new(segments));
        }
        return lines;
    };
    let tint = bold(claude_status_color(&current.indicator));
    let description = if current.indicator == "none" && current.degraded.is_empty() && current.incidents.is_empty() {
        "Operational".into()
    } else {
        current.description
    };
    let description = format!("● {description}{}", if error.is_empty() { "" } else { " (stale)" });
    for (index, text) in wrap_detail_text(&description, room.saturating_sub(column).max(2)).into_iter().enumerate() {
        let mut segments = Vec::new();
        if index == 0 { segments.push(segment(0, label, style(FG))); }
        segments.push(segment(column as u16, text, tint));
        lines.push(Line::new(segments));
    }
    for detail in current.degraded.iter().take(3).map(|(name, state)| format!("{name}: {state}")).chain(current.incidents.into_iter().take(2)) {
        for text in wrap_detail_text(&detail, room.saturating_sub(2).max(2)) {
            lines.push(Line::new(vec![segment(2, text, tint)]));
        }
    }
    if !error.is_empty() {
        for text in wrap_detail_text(&error, room.saturating_sub(2).max(2)) {
            lines.push(Line::new(vec![segment(2, text, style(YELLOW))]));
        }
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
/// A session starting or restarting in the viewer pane: showing another card would replace the pane
/// and kill it. Only for as long as cockpit' loading screen waits (60 s), so a state file
/// left behind cannot stop the column from following.
fn is_viewer_loading() -> bool {
    fs::metadata(paths::loading_state()).and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age < Duration::from_secs(60)))
}

/// The Team view over the column: its rows, then the selected one in full (wrapped, never cut).
fn draw_team_view(view: &crate::teams::view::TeamView, buffer: &mut Buffer) {
    let (width, height) = (buffer.area.width, buffer.area.height);
    if width < 12 || height < 8 { return; }
    let (top, bottom) = (HEADER_LINES, height - 2);
    let inner = width.saturating_sub(4) as usize;
    let body = inner.saturating_sub(2).max(2);
    for row in top..=bottom {
        put(buffer, row, 1, &" ".repeat((width - 2) as usize), on(FG, BLACK));
        put(buffer, row, 1, "│", style(BLUE));
        put(buffer, row, width - 2, "│", style(BLUE));
    }
    put(buffer, top, 1, &format!("┌{}┐", "─".repeat(inner)), style(BLUE));
    let title = if view.is_history { "Team · link and message history" } else { "Team · requests, members and links" };
    put(buffer, top, 3, &fit(title, inner.saturating_sub(2)), bold(BLUE));
    let rows = view.rows();
    let list_height = (rows.len() as u16).min((bottom - top) / 3).max(1);
    let mut line = top + 1;
    if view.snapshot_error.is_some() {
        put(buffer, line, 3, &fit("Collaboration unavailable · stale snapshot (r refresh)", body), style(YELLOW));
        line += 1;
    }
    if rows.is_empty() {
        put(buffer, line, 3, &fit("nothing here", body), dim(FG_DARK));
    }
    let first = view.selected.saturating_sub(list_height as usize - 1);
    for (index, row) in rows.iter().enumerate().skip(first).take(list_height as usize) {
        let selected = index == view.selected;
        put(buffer, line, 3, &fit(&view.row_text(*row), body), if selected { on(FG, BG_HIGHLIGHT) } else { style(FG) });
        line += 1;
    }
    line += 1;
    for text in view.detail().iter().flat_map(|text| wrap_detail_text(text, body)).skip(view.detail_scroll) {
        if line >= bottom - 1 { break; }
        put(buffer, line, 3, &text, style(FG));
        line += 1;
    }
    put(buffer, bottom - 1, 3, &fit("Tab history · j/k rows · ←/→ detail · n team · l link · r refresh · Esc close", body), dim(FG_DARK));
    put(buffer, bottom, 1, &format!("└{}┘", "─".repeat(inner)), style(BLUE));
}

/// Every picker keeps its list's order unless `extra` turns sorting back on (a later --sort wins).
pub(crate) fn fzf_arguments<'a>(prompt: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    ["--prompt", prompt, "--height", "100%", "--no-sort"].into_iter().chain(extra.iter().copied()).collect()
}

fn fzf(prompt: &str, lines: &[String], extra: &[&str]) -> Option<String> {
    let mut child = Command::new("fzf").args(fzf_arguments(prompt, extra))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(lines.join("\n").as_bytes());
    }
    let output = child.wait_with_output().ok()?;
    let chosen = String::from_utf8_lossy(&output.stdout).trim_end_matches('\n').to_string();
    (output.status.success() && !chosen.is_empty()).then_some(chosen)
}

fn session_key_requires_terminal(key: Key) -> bool {
    matches!(key, Key::Enter | Key::Char('r' | 'R' | 'e' | '$' | 'C' | 'x' | 'z' | 'm'))
}

/// The card keys that need a feature of the session's agent, and what they do, named for a
/// refusal (cockpit refuses them too, and Remote Control).
fn key_feature(key: Key, agent: &agents::Agent) -> Option<(Feature, &'static str)> {
    match key {
        Key::Char('z') => Some((Feature::Sleep, "sleeping")),
        Key::Char('e') if agent.can(Feature::Variants) => Some((Feature::Variants, "the model variant picker")),
        Key::Char('e') => Some((Feature::Effort, "the effort level")),
        Key::Char('$') => Some((Feature::Billing, "billing to API credits")),
        Key::Char('C') => Some((Feature::Compact, "/compact")),
        Key::Char('m') => Some((Feature::RemoteControl, "Remote Control")),
        _ => None,
    }
}

/// The cockpit command in `arguments`, past a leading --agent <name>.
fn command_name<'a>(arguments: &[&'a str]) -> &'a str {
    match arguments {
        ["--agent", _, command, ..] => command,
        _ => arguments[0],
    }
}

fn pick_project() -> Option<String> {
    fzf("project> ", &sessions::project_choices(), &[])
}

fn pick_todo_kind(current: Option<todo::Category>, include_all: bool) -> Option<String> {
    let mut choices: Vec<String> = todo::Category::ALL.iter().map(|category| category.label().to_string()).collect();
    if include_all { choices.insert(0, "All".into()); }
    if let Some(category) = current {
        let index = choices.iter().position(|label| label == category.label()).unwrap();
        let selected = choices.remove(index);
        choices.insert(0, selected);
    }
    fzf(if include_all { "filter kind> " } else { "kind> " }, &choices, &[])
}

/// The agent a new session runs (agents::AGENTS); None when the pick is cancelled, and then
/// nothing starts.
fn pick_agent() -> Option<String> {
    let names: Vec<String> = agents::names().into_iter().map(String::from).collect();
    fzf("agent> ", &names, &[]).filter(|agent| agents::find(agent).is_some())
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
    let mut lines = body.lines();
    let title = lines.next()?.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let notes = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    Some((title, notes))
}

fn todo_context(item: &todo::Item) -> String {
    let category = item.category().map(|category| category.label().to_string()).unwrap_or_else(|| format!("Unknown: {}", item.0["category"]));
    let source = if !item.source().is_empty() { "Claude" } else if item.status() == todo::PROPOSED { "unknown session" } else { "you" };
    let routing = if item.is_design() { " · Design".to_string() }
        else if item.is_blocked_report() { format!(" · setup report from {}", item.reporter()) }
        else if !item.link().is_empty() { format!(" · from a {} session ({})", item.reporter(), item.link()) }
        else { " · standard routing".to_string() };
    format!("{category} · {} · {source}{routing}", item.status())
}

/// What Enter shows for an item or an issue.
pub(crate) fn todo_details(entry: &Entry) -> String {
    match entry {
        Entry::Typed(item) => {
            let status = match item.status().as_str() { todo::PROPOSED => "proposed by Claude", "doing" => "in progress", "done" => "done", _ => "to do" };
            let created = chrono::DateTime::from_timestamp(item.created(), 0).map(|time| time.with_timezone(&chrono::Local).format("%d/%m %H:%M").to_string()).unwrap_or_default();
            let filed = if !item.source().is_empty() { format!("Claude ({})", item.source()) }
                else if item.status() == todo::PROPOSED { "an unknown session".into() } else { "you".into() };
            let mut text = format!("{}\n\n{} · {status} · filed by {filed} on {created}\n", item.text(), item.project());
            let category = item.category().map(|category| category.label().to_string())
                .unwrap_or_else(|| format!("Unknown: {}", item.0["category"]));
            text.push_str(&format!("Kind: {category}\n"));
            text.push_str(&format!("{}\n", todo_context(item)));
            if !item.why().is_empty() {
                text.push_str(&format!("\nWhy Claude proposed it\n{}\n", item.why()));
            }
            text.push_str(&handoff_details(item));
            let notes = item.notes();
            text.push_str(&format!("\nNotes\n{}\n", if notes.is_empty() { "(none: e adds some)" } else { notes.as_str() }));
            for (number, file) in item.images() {
                text.push_str(&format!("{} {}\n", crate::todo_images::marker(number), crate::todo_images::path_of(&file).display()));
            }
            text.push_str(if item.status() == todo::PROPOSED { "\nIn the column: y approve · n decline · e edit\n" } else { "\nIn the column: e edit · h hand to a session · d done\n" });
            text
        }
        Entry::Issue { issue, status, .. } => format!("{}#{} {}\n\n{status} · labels: {}\n\n{}\n", issue.repository, issue.number, issue.title,
                                                       issue.labels.join(", "), issue.url),
    }
}

/// Who handed an item over and what it waits on, for Theo to follow.
fn handoff_details(item: &todo::Item) -> String {
    let mut text = String::new();
    if !item.queued_by().is_empty() {
        text.push_str(&format!("Queued by {}\n", item.queued_by()));
    }
    let after = item.after();
    if !after.is_empty() {
        text.push_str(&format!("\nWaits on (joins the queue once they are done; h hands it anyway)\n{}\n", after.join(", ")));
    }
    for input in item.inputs() {
        let field = |key: &str| input.get(key).and_then(serde_json::Value::as_str).unwrap_or("").to_string();
        text.push_str(&format!("\nDone before it: {} in {} ({})\n{}\n", field("id"), field("project"), field("text"),
                               if field("result").is_empty() { "(no summary)".to_string() } else { field("result") }));
    }
    if !item.result().is_empty() {
        text.push_str(&format!("\nResult, from its session\n{}\n", item.result()));
    }
    text
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
                    KeyCode::PageUp => Key::PageUp,
                    KeyCode::PageDown => Key::PageDown,
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
            Event::Paste(text) => return Some(Key::Paste(text)),
            Event::FocusLost => return Some(Key::FocusLost),
            Event::Resize(..) => return Some(Key::Resize),
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
