//! The action panels (Sessions, Pull requests, Todo, Ideas), on-demand Status, the header and footer,
//! the effort and settings menus, the help, and every key.

use crate::agents::{self, Feature};
use crate::continuity::Continuity;
mod chief_tab;
mod chief_ui;
mod continuity_ui;
mod ideas_tab;
mod input;
mod menus;
mod new_session;
pub mod refresh;
mod reviews_tab;
mod session_refresh;
mod sessions_tab;
mod status_panel;
use input::input_line;
pub use input::{Key, read_key};
#[cfg(test)]
pub(crate) use input::{LineEdit, edit_line};
use menus::Pairing;
#[cfg(test)]
pub(crate) use menus::SettingsRow;
pub use menus::{EffortMenu, SettingsMenu};
mod render;
mod teams_tab;
mod todo_tab;
use crate::costs::Costs;
use crate::errors::ErrorReporter;
use crate::paths;
use crate::prompts;
use crate::queue::{self, QUEUE_CHECK_S};
use crate::reviews::{self, Review, Reviews};
use crate::sessions::{
    self, CONTEXT_COMPACT_AT, History, PastSession, Restorable, Session, ViewState,
};
use crate::settings::{self, LAUNCH_SETTINGS, SETTINGS};
use crate::sleep;
use crate::status::{self, Provider, ProviderStatus};
use crate::todo::{self, Entry, TodoList};
use crate::todo_composer::{Outcome, TodoComposer};
#[cfg(test)]
pub(crate) use crate::todo_editor::text::{editor_text, parse_editor_text};
use crate::usage::{self, BudgetAlerts, Refresher, Window};
use crate::util::{
    self, age, current_session, manager, manager_in_background, money, now, project_name,
    start_directory,
};
use crate::{approval, remote};
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
pub(crate) use todo_tab::todo_details;
use todo_tab::{TODO_TAB_IN_PROGRESS, TODO_TAB_OPEN, TODO_TAB_TO_DO};
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

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const PANEL_NAMES: [&str; 7] = [
    "Status",
    "Sessions",
    "Pull requests",
    "Todo",
    "Ideas",
    "Teams",
    "Chief",
];
/// The tabs' panels, in the order of their keys 1 to 5.
const TAB_PANELS: [usize; 5] = [1, 3, 4, TEAMS_PANEL, CHIEF_PANEL];
pub(crate) const TEAMS_PANEL: usize = 5;
pub(crate) const CHIEF_PANEL: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusAction {
    ClaudeStatus,
    CodexStatus,
    Digest,
    Clones,
}

impl StatusAction {
    fn hint(self) -> &'static str {
        match self {
            Self::ClaudeStatus => "Claude status page",
            Self::CodexStatus => "Codex status page",
            Self::Digest => "daily digest",
            Self::Clones => "clones review",
        }
    }
}
const GLOBAL_HINTS: [(&str, &str); 2] = [("s", "status"), ("?", "help")];
type HelpSection = (&'static str, Vec<(&'static str, &'static str)>);

/// ⌘ in key hints: the physical Command key, which keyd turns into Ctrl (system/etc/keyd).
const GLOBAL_HELP: &[(&str, &str)] = &[
    ("1 2 3 4 5", "Sessions · Todo · Ideas · Teams · Chief"),
    ("s", "Status: usage, spending, Chief, PRs, phone"),
    ("?", "this help (for the tab you're on)"),
    ("esc", "close / back"),
    ("N", "new session: blank or Claude Design canvas"),
    ("H", "resume a past session, searched by project and prompt"),
    (
        "S",
        "settings: default agent, models, on/off choices, phone pairing",
    ),
    (
        "P W G",
        "windows on top: pull requests, Claude on the web, greyproxy",
    ),
    ("⌘1 ⌘2 ⌘3", "a tab from inside the session"),
    ("→ ←", "into the session and back"),
    ("q", "close the column; sessions keep running"),
];
/// What the signs on a session card mean: Sessions help's last section.
const LEGEND: &[(&str, &str)] = &[
    ("!", "needs you"),
    ("◐", "working"),
    ("✓", "finished"),
    ("☾", "asleep"),
    ("↳", "waits on a todo"),
    ("⇄", "linked"),
    ("◆", "the Chief"),
    ("▶", "your approved command runs"),
    ("‖", "later: its ask is hidden"),
    ("●", "finished with a result you haven't opened"),
    ("○", "idle: waiting for your next message"),
    ("⟳", "restart needed (or restarting)"),
    ("⚲", "pinned"),
    ("↑2", "unpushed commits"),
    ("✉2", "queued messages"),
    ("⬆", "update available"),
    ("$", "billed to API credits"),
    ("·", "state unknown: no report"),
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
    match percentage {
        None => FG_DARK,
        Some(p) if p >= 80.0 => RED,
        Some(p) if p >= 50.0 => YELLOW,
        Some(_) => GREEN,
    }
}

fn claude_status_color(indicator: &str) -> u8 {
    match indicator {
        "none" => GREEN,
        "minor" => YELLOW,
        "major" | "critical" => RED,
        "maintenance" => CYAN,
        _ => YELLOW,
    }
}

pub fn width_of(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// The text cut to `available` cells, with an ellipsis when cut.
pub fn fit(text: &str, available: usize) -> String {
    if width_of(text) <= available {
        return text.to_string();
    }
    if available <= 1 {
        return String::new();
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
    Part {
        text: text.into(),
        style,
        action: None,
    }
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
        Self {
            segments,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub fn texts(&self) -> String {
        self.segments
            .iter()
            .map(|(_, text, _)| text.as_str())
            .collect()
    }
}

fn segment(column: u16, text: impl Into<String>, style: Style) -> (u16, String, Style) {
    (column, text.into(), style)
}

/// Where a session stands for Theo: the session list's groups, in their order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Attention {
    NeedsYou,
    Working,
    Quiet,
}

impl Attention {
    fn heading(self) -> &'static str {
        match self {
            Attention::NeedsYou => "NEEDS YOU",
            Attention::Working => "WORKING",
            Attention::Quiet => "QUIET",
        }
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
    let right = if notice.key.is_empty() {
        Vec::new()
    } else {
        vec![part(notice.key, dim(FG_DARK))]
    };
    Line {
        segments: vec![
            segment(0, notice.glyph, bold(notice.tint)),
            segment(2, notice.text.clone(), style(FG)),
        ],
        right,
        accent: Some(bold(notice.tint)),
        ..Line::default()
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// Requests greyproxy holds, as a NEEDS YOU card: only Theo can allow or deny them. Its entries
/// read `<sandbox> → host:port` (sessions::greyproxy_held).
fn greyproxy_card(held: &[String], since: i64) -> Vec<Line> {
    let accent = Some(bold(YELLOW));
    let sandboxes: BTreeSet<&str> = held
        .iter()
        .map(|entry| entry.split_once(" → ").map_or("", |(sandbox, _)| sandbox))
        .collect();
    let why = match sandboxes.iter().next() {
        Some(sandbox) if sandboxes.len() == 1 && !sandbox.is_empty() => {
            let destinations: Vec<&str> = held
                .iter()
                .filter_map(|entry| entry.split_once(" → ").map(|(_, destination)| destination))
                .collect();
            why_segments(sandbox, &destinations.join(", "))
        }
        _ => why_segments("greyproxy", &held.join(", ")),
    };
    vec![
        Line {
            segments: vec![
                segment(0, "!", bold(YELLOW)),
                segment(
                    2,
                    format!(
                        "greyproxy held {} request{}",
                        held.len(),
                        plural(held.len())
                    ),
                    bold(FG),
                ),
            ],
            right: vec![part(util::age(since), dim(FG_DARK))],
            accent,
            ..Line::default()
        },
        Line {
            segments: why,
            accent,
            ..Line::default()
        },
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
    let text = if count.is_empty() {
        ask.to_string()
    } else {
        format!("{ask}   {count}")
    };
    Line {
        segments: vec![segment(
            2,
            format!(" {text}"),
            bar.add_modifier(Modifier::BOLD),
        )],
        right: vec![part(format!("{key_hint} "), bar)],
        item,
        accent: Some(bold(YELLOW)),
        fill: Some((2, bar)),
    }
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

enum TeamLinks {
    Loading,
    Ready(Value),
    Unavailable,
}

impl PanelState {
    fn content_height(&self) -> usize {
        self.fixed_top.len() + self.lines.len() + self.fixed_bottom.len()
    }

    fn clamp(&mut self) {
        self.selected = if self.items == 0 {
            0
        } else {
            self.selected.min(self.items - 1)
        };
    }

    fn move_by(&mut self, delta: i64) {
        self.selected = if self.items == 0 {
            0
        } else {
            (self.selected as i64 + delta).clamp(0, self.items as i64 - 1) as usize
        };
    }

    fn keep_selected_visible(&mut self, inner_height: usize) {
        // A collapsed panel shows no line: scrolling it would flip between 0 and 1 at every frame.
        if inner_height == 0 {
            self.scroll = 0;
            return;
        }
        let target = self
            .lines
            .iter()
            .position(|line| line.item == Some(self.selected))
            .unwrap_or(0);
        if target < self.scroll {
            self.scroll = target.saturating_sub(1);
        } else if target >= self.scroll + inner_height {
            self.scroll = target + 1 - inner_height;
        }
    }
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
    /// Session details: the project the first paragraph (why) starts with, drawn bold blue before the dim rest.
    lead: Option<String>,
}

/// Session details' keys, grouped as frame 2200 draws them; they work on the selected card too.
const SESSION_DETAIL_KEYS: [(&str, &[(&str, &str)]); 3] = [
    (
        "WORK",
        &[
            ("e", "effort"),
            ("$", "billing: subscription / API"),
            ("C", "/compact"),
            ("c", "Chief answers"),
            ("k", "links"),
            ("d", "drop queued"),
            ("u", "updates"),
            ("Q", "queue explanations"),
            ("v", "mark displayed updates reviewed"),
        ],
    ),
    (
        "CODE",
        &[
            ("g", "lazygit"),
            ("U", "push / apply"),
            ("+", "parallel clone"),
            ("o", "design canvas"),
            ("t", "task trail"),
        ],
    ),
    (
        "SESSION",
        &[
            ("I", "images"),
            ("m", "remote control"),
            ("w", "my note"),
            ("z", "sleep"),
            ("R", "restart"),
            ("x", "end"),
        ],
    ),
];

enum DetailRow {
    Heading(String),
    Text(String),
    Keys(Vec<(&'static str, &'static str)>),
}

fn is_detail_heading(line: &str) -> bool {
    let heading = line.split(" (").next().unwrap_or(line);
    heading.len() > 2
        && heading
            .chars()
            .all(|character| character.is_ascii_uppercase() || " &".contains(character))
}

#[derive(Clone)]
enum DetailAction {
    Reviewed {
        session: String,
        identities: Vec<String>,
    },
    OpenSession(String),
    OpenTask(String),
    OpenPr(String),
    ResumeSession {
        session: String,
        cwd: String,
    },
}

impl SessionDetails {
    fn draw(&mut self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        if width < 8 || height < 6 {
            return;
        }
        let top = HEADER_LINES;
        let bottom = height - 2;
        let inner = width.saturating_sub(4) as usize;
        let body_width = inner.saturating_sub(2).max(2);
        let body_height = (bottom.saturating_sub(top + 3) as usize).max(1);
        let rows = self.rows(body_width);
        self.scroll = self.scroll.min(rows.len().saturating_sub(body_height));
        for row in top..=bottom {
            put(buffer, row, 1, &" ".repeat((width - 2) as usize), style(FG));
            put(buffer, row, 1, "│", style(BLUE));
            put(buffer, row, width - 2, "│", style(BLUE));
        }
        put(
            buffer,
            top,
            1,
            &format!("┌{}┐", "─".repeat(inner)),
            style(BLUE),
        );
        put(
            buffer,
            top,
            3,
            &fit(&self.title, inner.saturating_sub(2)),
            bold(BLUE),
        );
        let why_rows = if self.lead.is_some() {
            rows.iter()
                .position(|row| matches!(row, DetailRow::Text(text) if text.is_empty()))
                .unwrap_or(0)
        } else {
            0
        };
        for (offset, line) in rows.iter().enumerate().skip(self.scroll).take(body_height) {
            let row = top + 1 + (offset - self.scroll) as u16;
            match line {
                DetailRow::Heading(text) => {
                    put(buffer, row, 3, text, bold(FG_DARK));
                }
                DetailRow::Text(text) if offset < why_rows => {
                    let project = self
                        .lead
                        .as_deref()
                        .filter(|project| offset == 0 && text.starts_with(*project))
                        .unwrap_or("");
                    put(buffer, row, 3, &text[..project.len()], bold(BLUE));
                    put(
                        buffer,
                        row,
                        3 + width_of(project) as u16,
                        &text[project.len()..],
                        style(FG_DARK),
                    );
                }
                DetailRow::Text(text) => {
                    put(buffer, row, 3, text, style(FG));
                }
                DetailRow::Keys(keys) => {
                    let mut column = 3;
                    for (key, label) in keys {
                        put(
                            buffer,
                            row,
                            column,
                            &format!(" {key} "),
                            on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD),
                        );
                        column += width_of(key) as u16 + 3;
                        put(buffer, row, column, label, style(FG_DARK));
                        column += width_of(label) as u16 + 2;
                    }
                }
            }
        }
        let progress = format!(
            "{}–{} / {}",
            self.scroll + 1,
            (self.scroll + body_height).min(rows.len()),
            rows.len()
        );
        if bottom - 2 > top + body_height as u16 {
            put(
                buffer,
                bottom - 2,
                3,
                &fit(&progress, body_width),
                dim(FG_DARK),
            );
        }
        let footer = if self.session.is_some() {
            "esc back · the same keys work on the selected card without opening i"
        } else if self.actions.is_empty() {
            "↑↓ scroll  g/G top/end  Esc close"
        } else {
            "↑↓ scroll  Enter actions  Esc close"
        };
        put(
            buffer,
            bottom - 1,
            3,
            &fit(footer, body_width),
            dim(FG_DARK),
        );
        put(
            buffer,
            bottom,
            1,
            &format!("└{}┘", "─".repeat(inner)),
            style(BLUE),
        );
    }

    /// The text wrapped to `width` (capitalised lines become headings), then each key group
    /// flowed into as few rows of key caps as fit.
    fn rows(&self, width: usize) -> Vec<DetailRow> {
        let mut rows: Vec<DetailRow> = wrap_detail_text(&self.text, width)
            .into_iter()
            .map(|line| {
                if is_detail_heading(&line) {
                    DetailRow::Heading(line)
                } else {
                    DetailRow::Text(line)
                }
            })
            .collect();
        // One blank line, not two, between the text and the first group of keys.
        while self.session.is_some()
            && rows.len() > 1
            && rows[rows.len() - 2..]
                .iter()
                .all(|row| matches!(row, DetailRow::Text(text) if text.is_empty()))
        {
            rows.pop();
        }
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
            if !current.is_empty() {
                rows.push(DetailRow::Keys(current));
            }
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
    pub panels: [PanelState; 7],
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
    /// When Theo last saw his habits, read with chief_status: the 5 Chief tab's "habits (N new)".
    chief_habits_seen_ms: Option<i64>,
    /// Theo's answers to a restarted Chief's test message, which the tab's short-lived views share.
    chief_check_answers: crate::chief::view::CheckAnswers,
    /// Team requests, escalations and uncertain deliveries waiting for Theo; session -> "team role · task".
    team_attention: usize,
    team_roles: HashMap<String, String>,
    qa_sessions: HashSet<String>,
    /// Whether the last team check succeeded: until then anyone may be a member, and no one sleeps.
    team_known: bool,
    pub(crate) team_checked: f64,
    team_links: TeamLinks,
    /// The 4 Teams tab: what its list and, inside a team, its rows show (teams_tab).
    pub(crate) teams_tab: teams_tab::TeamsTab,
    /// The 5 Chief tab's rows that act: needs-you decisions and today's answers (chief_tab).
    chief_tab_rows: Vec<chief_tab::ChiefRow>,
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
    /// The todos the Chief asked a fresh session for (chief_session_starts), read again at most
    /// every queue check: they start like handed work, queued or not.
    chief_requested: HashSet<String>,
    pub(crate) chief_requests_checked: f64,
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
    pub(crate) plan: String,
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
            Ok(Some(count)) => (
                format!("recovered {count} TODO draft(s) in their original projects"),
                false,
            ),
            Ok(None) => (String::new(), true),
            Err(error) => (
                format!("TODO draft recovery failed; drafts kept: {error}"),
                false,
            ),
        };
        Self {
            continuity: Continuity::load(&paths::continuity()),
            continuity_error: None,
            status_actions: Vec::new(),
            status_selection_lost: false,
            history: History::default(),
            view: ViewState::load(),
            usage: Refresher::new(),
            alerts: BudgetAlerts::new(util::send_notice),
            costs: Costs::default(),
            reviews: Reviews::default(),
            todos,
            errors: ErrorReporter::new(),
            claude_status: ProviderStatus::default(),
            codex_status: ProviderStatus::new(Provider::Codex),
            panels: Default::default(),
            focus: 1,
            previous_panel: 1,
            help_open: false,
            help_scroll: 0,
            has_pane_focus: true,
            message,
            exiting: Vec::new(),
            restarting_cards: HashMap::new(),
            labelled: Default::default(),
            menu: None,
            session_details: None,
            todo_composer: None,
            new_session: None,
            settings_menu: None,
            team_view: None,
            chief_view: None,
            team_attention: 0,
            team_roles: HashMap::new(),
            qa_sessions: HashSet::new(),
            team_known: false,
            team_checked: 0.0,
            team_links: TeamLinks::Loading,
            teams_tab: Default::default(),
            chief_tab_rows: Vec::new(),
            sidebar_rows: Vec::new(),
            tab_ranges: Vec::new(),
            collapsed_teams: HashSet::new(),
            sidebar_selection_lost: false,
            sidebar_session_tasks: HashMap::new(),
            pairing: None,
            restore: Vec::new(),
            held: Vec::new(),
            held_since: None,
            held_checked: 0.0,
            notices: Vec::new(),
            is_confirming: false,
            queue_checked: 0.0,
            handed_refused: HashSet::new(),
            chief_requested: HashSet::new(),
            chief_requests_checked: 0.0,
            sleep_checked: 0.0,
            sleep_asked: HashMap::new(),
            gaming: util::gaming,
            disk_usage: util::fullest_disk,
            disk: None,
            disk_checked: 0.0,
            restore_checked: 0.0,
            start_files: Vec::new(),
            width: DEFAULT_COLUMN_WIDTH,
            sessions: Vec::new(),
            running_sessions: Vec::new(),
            windows: Vec::new(),
            quota_rows: Vec::new(),
            review_rows: Vec::new(),
            todo_entries: Vec::new(),
            idea_entries: Vec::new(),
            session_task_titles: HashMap::new(),
            session_branches: HashMap::new(),
            session_queued: HashMap::new(),
            chief_answers: HashMap::new(),
            chief_status: Err("Not read yet".into()),
            chief_habits_seen_ms: None,
            chief_check_answers: Default::default(),
            session_roots: HashMap::new(),
            todo_scope: String::new(),
            shown_session: String::new(),
            todo_tab: TODO_TAB_OPEN,
            todo_tabs: HashMap::new(),
            todo_categories: HashMap::new(),
            saved_todo_scope: None,
            todo_jump: None,
            todo_counts: [0; 5],
            prompts_sent: HashMap::new(),
            subscription_held: HashSet::new(),
            subscription_directories: HashSet::new(),
            subscription_error: None,
            craft_roots: HashMap::new(),
            hit_lines: HashMap::new(),
            hit_actions: HashMap::new(),
            subscription_polled_at: None,
            subscription_waiting_shown: HashMap::new(),
            gathered: None,
            pending: Vec::new(),
            plan: String::new(),
            craft_checked: 0.0,
            running_copies_unpushed: HashMap::new(),
            running_copies_checked: 0.0,
            todo_recovery_pending,
        }
    }

    /// A panel taking the focus shows from its top: while small it scrolled to keep its selection
    /// in view, and that scroll would otherwise stay once it is big again.
    pub fn focus_panel(&mut self, index: usize) {
        if matches!(index, 0 | CHIEF_PANEL) {
            self.refresh_chief_status();
        }
        if index == CHIEF_PANEL {
            // Back on the Chief: what was left open there (Tab away from a review) shows again.
            if self.chief_view.as_ref().is_some_and(|view| view.suspended) {
                self.chief_view.as_mut().expect("checked above").suspended = false;
                self.refresh_chief_view();
            }
            self.build_chief_tab();
        }
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
                    if count > 0 {
                        self.message =
                            format!("recovered {count} TODO draft(s) in their original projects");
                    }
                }
                Err(error) => {
                    self.todo_recovery_pending = false;
                    self.message = format!("TODO draft recovery failed; drafts kept: {error}");
                }
            }
        }
        self.todos.apply_updates();
        self.check_team();
        self.errors.tick(|| {
            sessions::reported()
                .into_iter()
                .filter(|session| !self.qa_sessions.contains(&session.session))
                .collect()
        });
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
        let started = Command::new("cockpit")
            .args(arguments)
            .env("COCKPIT_LOG_STDERR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
        match started {
            Ok(child) => {
                self.pending
                    .push((command_name(arguments).to_string(), child));
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
            return format!(
                "{} runs in its own terminal: the cockpit could not wake it",
                session.project
            );
        }
        if !self.team_known {
            return format!(
                "cannot tell yet whether {} is in a team: try again in a moment",
                session.project
            );
        }
        let started = Command::new("cockpit")
            .args(["sleep", &session.session])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn();
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
                    if command == "sleep"
                        && status.code() == Some(SLEEP_REFUSED)
                        && !last.is_empty()
                    {
                        self.message = last;
                        continue;
                    }
                    util::log_event(
                        "ERROR",
                        &format!(
                            "cockpit {command}: {}",
                            if last.is_empty() { "failed" } else { &last }
                        ),
                    );
                    self.message = if last.is_empty() {
                        format!("cockpit {command} failed")
                    } else {
                        last
                    };
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
        if self
            .subscription_polled_at
            .is_some_and(|last| now - last < 60.0)
            || self
                .pending
                .iter()
                .any(|(command, _)| command == "subscription-sweep")
        {
            return;
        }
        self.subscription_polled_at = Some(now);
        let error = self.manager_async(&["subscription-sweep"]);
        if !error.is_empty() {
            util::log_event("WARN", &error);
        }
    }

    fn show_subscription_waiting(&mut self, routes: &[crate::subscription_fallback::Route]) {
        let waiting: HashMap<String, String> = routes
            .iter()
            .filter_map(|route| Some((route.original.session.clone(), route.waiting.clone()?)))
            .collect();
        use crate::subscription_fallback::Phase;
        for route in routes
            .iter()
            .filter(|route| matches!(route.phase, Phase::Active | Phase::Pending(_)))
        {
            let Some(reason) = &route.waiting else {
                continue;
            };
            if self.subscription_waiting_shown.get(&route.original.session) == Some(reason) {
                continue;
            }
            self.message = format!(
                "{}: subscription switch waiting: {reason}",
                project_name(&util::repository_root(&route.directory))
            );
        }
        self.subscription_waiting_shown = waiting;
    }

    pub fn dispatch_subscription_routes(&mut self) {
        use crate::subscription_fallback::{self, Direction, Phase};
        let result = (|| -> Result<(), String> {
            let routes = subscription_fallback::read_routes(&paths::subscription_routes())?;
            self.subscription_held.clear();
            self.subscription_directories.clear();
            for route in routes.iter().filter(|route| {
                matches!(route.original.selection.agent.as_str(), "claude" | "codex")
            }) {
                if matches!(route.phase, Phase::Cancelled | Phase::Completed) {
                    continue;
                }
                self.subscription_held
                    .extend(route.retired_sessions.iter().cloned());
                if route.original.session != route.current.session {
                    self.subscription_held
                        .insert(route.original.session.clone());
                }
                match &route.phase {
                    Phase::Pending(pending) => {
                        self.subscription_held.insert(route.current.session.clone());
                        self.subscription_directories
                            .insert(route.directory.clone());
                        if pending.direction == Direction::Return {
                            self.subscription_held
                                .insert(route.original.session.clone());
                        }
                        if let Some(replacement) = &pending.replacement {
                            self.subscription_held.insert(replacement.session.clone());
                        }
                    }
                    Phase::Paused(_) => {
                        self.subscription_held.insert(route.current.session.clone());
                        self.subscription_directories
                            .insert(route.directory.clone());
                    }
                    Phase::Active => {
                        if route.awaiting_initial_admission() {
                            self.subscription_held.insert(route.current.session.clone());
                            self.subscription_directories
                                .insert(route.directory.clone());
                        }
                        for retired in &route.retired_sessions {
                            prompts::transfer_subscription_prompts(
                                retired,
                                &route.current.session,
                            )?;
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
                util::log_event(
                    "WARN",
                    &format!("automatic native actions paused: subscription routing: {error}"),
                );
                self.message = format!("Subscription routing paused: {error}");
            }
            self.subscription_error = error;
        }
    }

    fn subscription_holds(&self, session: &Session) -> bool {
        matches!(session.agent.as_str(), "claude" | "codex")
            && (self.subscription_error.is_some()
                || self.subscription_held.contains(&session.session))
    }

    /// A session craft-open asked to restart (its figcraft-app server had exited at its start) is
    /// restarted once its turn ends, in a tmux session of its own; its queued prompt then waits for
    /// the restarted session. A terminal session is its terminal's: it is only told in the log.
    fn dispatch_restarts(&mut self) {
        let ready: Vec<Session> = self
            .running_sessions
            .iter()
            .filter(|session| {
                !self.subscription_holds(session)
                    && !session.is_external()
                    && session.state == "waiting"
                    && prompts::restart_wanted(&session.session)
            })
            .cloned()
            .collect();
        for session in ready {
            prompts::restart_done(&session.session);
            if session.terminal {
                util::log_event(
                    "WARN",
                    &format!(
                        "{} waited for a restart at the end of its turn, but runs in its own terminal: restart it there",
                        session.project
                    ),
                );
                continue;
            }
            manager_in_background(&["restart", &session.session, "", "detached"]);
            self.prompts_sent
                .insert(session.session.clone(), util::now() as i64);
            util::log_event(
                "INFO",
                &format!("{} restarted at the end of its turn", session.project),
            );
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
                Err(error) => {
                    util::log_event("WARN", &format!("native message delivery paused: {error}"));
                    continue;
                }
            }
            let Some((text, path)) = prompts::oldest(&session.session) else {
                continue;
            };
            let error = manager(&["say", &session.session, &text]);
            if !error.is_empty() {
                util::log_event(
                    "WARN",
                    &format!(
                        "queued prompt for {} not sent yet: {error}",
                        session.project
                    ),
                );
                continue;
            }
            let _ = std::fs::remove_file(&path);
            self.prompts_sent
                .insert(session.session.clone(), util::now() as i64);
            util::log_event(
                "INFO",
                &format!(
                    "queued prompt sent to {} at the end of its turn",
                    session.project
                ),
            );
            crate::audit::record(
                "column",
                "prompt.sent",
                "ok",
                serde_json::json!({"session": session.session, "project": session.project,
                                                                     "text": crate::audit::fingerprint(&text)}),
            );
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
            util::log_event(
                "INFO",
                &format!("{} asleep for over a week, forgotten", note.project),
            );
        }
        if !self.team_known {
            return;
        }
        let context = sleep::SleepContext {
            now: now as i64,
            enabled: settings::is_on("idle-sleep"),
            after_s: sleep::after_minutes() * 60,
            viewed: current_session(),
        };
        let eligible: Vec<Session> = self
            .running_sessions
            .iter()
            .filter(|session| !self.subscription_holds(session))
            .filter(|session| self.sleep_asked.get(&session.session) != Some(&session.since))
            .filter(|session| {
                sleep::block(
                    session,
                    &context,
                    self.todos.busy(&session.session),
                    prompts::count(&session.session) > 0,
                )
                .is_none()
            })
            .cloned()
            .collect();
        self.sleep_asked.retain(|asked, _| {
            self.running_sessions
                .iter()
                .any(|session| &session.session == asked)
        });
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
        if now - self.chief_requests_checked >= QUEUE_CHECK_S {
            self.chief_requests_checked = now;
            self.chief_requested = chief_requested_starts();
        }
        if self.todos.queued.is_empty() && self.chief_requested.is_empty() {
            return;
        }
        let is_check_due = now - self.queue_checked >= QUEUE_CHECK_S;
        if is_check_due {
            self.handed_refused.clear();
        }
        let handed = self.handed_work();
        // The context asks the daemon for a full team snapshot and runs hyprctl: not on every tick
        // when there is nothing to start.
        if handed.is_empty() && !is_check_due {
            return;
        }
        let context = self.queue_context();
        self.start_sessions_for_handed_work(&context, handed);
        if !is_check_due {
            return;
        }
        self.queue_checked = now;
        let idle: Vec<Session> = self
            .all_sessions()
            .into_iter()
            .filter(|session| !self.subscription_holds(session))
            .filter(|session| {
                queue::block(
                    session,
                    &context,
                    self.todos.busy(&session.session),
                    prompts::count(&session.session) > 0,
                )
                .is_none()
            })
            .collect();
        for session in idle {
            self.give_queued_work(&session, now);
        }
    }

    /// Gives the idle session the first of its project's queued items the host lets it take. A
    /// refused item (still reserved, or another team's task) does not hold back the ones after it;
    /// a refusal of the session itself ends the pass for it.
    fn give_queued_work(&mut self, session: &Session, now: f64) {
        for entry in queue::candidates(
            session,
            &self.todos,
            &self.reviews.issues(),
            &self.reviews.checkouts,
        ) {
            let (key, text) = match &entry {
                Entry::Typed(item) => (item.id(), todo::prompt_of(&entry)),
                Entry::Issue { issue, .. } => (issue.key(), todo::prompt_of(&entry)),
            };
            let short_session = &session.session[..8.min(session.session.len())];
            let token = match queue::reserve_task(&session.cwd, &key, Some(&session.session), true)
            {
                Ok(token) => token,
                Err(error) => {
                    util::log_event(
                        "WARN",
                        &format!(
                            "queue: {key} not given to {} ({short_session}): {error}",
                            session.project
                        ),
                    );
                    if error.starts_with(queue::SESSION_NOT_READY) {
                        return;
                    }
                    continue;
                }
            };
            if let Err(error) = queue::launch_reserved(
                &token,
                &["say", &session.session, &todo::with_todo_note(&text, &key)],
                false,
            ) {
                util::log_event(
                    "WARN",
                    &format!("queue: cannot give {key} to {}: {error}", session.project),
                );
                return;
            }
            if !self.todos.set_status(&key, "doing", &session.session) {
                self.message = format!("queue: sent {key}, but cannot save its status");
                return;
            }
            if let Err(error) = self.record_task_assignment(&entry, Some(session)) {
                self.continuity_error = Some(error);
            }
            util::log_event(
                "INFO",
                &format!(
                    "queue: {key} given to {} ({short_session}), idle for {} min",
                    session.project,
                    (now as i64 - session.since) / 60
                ),
            );
            self.message = format!("queue: gave {} its next item", session.project);
            return;
        }
    }

    /// Handed work and setup blockers get fresh background sessions on every tick, before idle
    /// sessions can absorb them. Theo's own Q items keep waiting for a session he starts, unless the
    /// Chief asked for one (queued or not).
    /// The handed items that could start a session now: not refused since the last queue check,
    /// and not in a project whose subscription handoff is under way.
    fn handed_work(&self) -> Vec<Entry> {
        let mut handed: Vec<Entry> =
            self.todos
                .items
                .iter()
                .filter(|item| {
                    let is_handed = self.todos.is_queued(&item.id())
                        && (!item.queued_by().is_empty() || item.is_blocked_report());
                    item.status() == "todo"
                        && (is_handed || self.chief_requested.contains(&item.id()))
                        && !item.is_waiting()
                        && !item.is_design()
                        && !self.handed_refused.contains(&item.id())
                })
                .filter(|item| {
                    let project = project_name(&util::repository_root(&item.cwd()));
                    self.subscription_error.is_none()
                        && !self.subscription_directories.iter().any(|directory| {
                            project_name(&util::repository_root(directory)) == project
                        })
                })
                .map(|item| Entry::Typed(item.clone()))
                .collect();
        handed.sort_by(todo::queue_order);
        handed
    }

    fn start_sessions_for_handed_work(
        &mut self,
        context: &queue::QueueContext,
        handed: Vec<Entry>,
    ) {
        if !context.enabled || context.gaming || !context.host_available {
            return;
        }
        for entry in handed {
            let Entry::Typed(item) = &entry else { continue };
            let project = project_name(&util::repository_root(&item.cwd()));
            let directory = match start_directory(&item.cwd()) {
                Ok(directory) => directory,
                Err(error) => {
                    util::log_event(
                        "WARN",
                        &format!("queue: cannot start a session for {}: {error}", item.id()),
                    );
                    self.handed_refused.insert(item.id());
                    continue;
                }
            };
            let agent = match settings::launch_agent() {
                Ok(agent) => agent,
                Err(error) => {
                    util::log_event(
                        "WARN",
                        &format!("queue: cannot start a session for {}: {error}", item.id()),
                    );
                    return;
                }
            };
            let token = match queue::reserve_task(&directory, &item.id(), None, true) {
                Ok(token) => token,
                Err(error) => {
                    util::log_event(
                        "WARN",
                        &format!(
                            "queue: {} not started, retried at the next queue check: {error}",
                            item.id()
                        ),
                    );
                    self.handed_refused.insert(item.id());
                    continue;
                }
            };
            if let Err(error) = queue::launch_reserved(
                &token,
                &[
                    "--agent",
                    &agent,
                    "background",
                    &directory,
                    &todo::with_todo_note(&todo::prompt_of(&entry), &item.id()),
                ],
                true,
            ) {
                util::log_event("WARN", &error);
                self.handed_refused.insert(item.id());
                continue;
            }
            if !self.todos.set_status(&item.id(), "doing", "") {
                self.message = format!(
                    "queue: started session, but cannot save status for {}",
                    item.id()
                );
                continue;
            }
            if let Err(error) = self.record_task_assignment(&entry, None) {
                self.continuity_error = Some(error);
            }
            let is_chief_requested = self.chief_requested.contains(&item.id());
            let started_by = if is_chief_requested {
                "the Chief asked".to_string()
            } else {
                item.queued_by()
            };
            util::log_event(
                "INFO",
                &format!(
                    "queue: started a fresh session in {project} for {} ({started_by})",
                    item.id()
                ),
            );
            crate::audit::record(
                "column",
                "queue.session_started",
                "ok",
                serde_json::json!({"id": item.id(), "project": project,
                "queued_by": item.queued_by(), "chief_requested": is_chief_requested}),
            );
            self.message = format!("queue: started a session in {project} for handed work");
        }
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
        self.build_teams();
        if self.focus == CHIEF_PANEL {
            self.build_chief_tab();
        }
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
        Some(if today >= 60.0 {
            format!("⏱ {} today · {}", short(today), short(all_time))
        } else {
            format!("⏱ {}", short(all_time))
        })
    }

    /// The viewed session's state goes to the manager (its title line's colour) when it changes.
    fn push_viewed_state(&mut self) {
        let viewed = current_session();
        let state = self
            .running_sessions
            .iter()
            .find(|s| s.session == viewed)
            .map(|session| {
                if session.needs_input {
                    "needs-you".into()
                } else {
                    session.state.clone()
                }
            })
            .unwrap_or_default();
        if (viewed.clone(), state.clone()) == self.labelled {
            return;
        }
        self.labelled = (viewed.clone(), state.clone());
        if !viewed.is_empty() && !state.is_empty() {
            manager_in_background(&["label-state", &state, &viewed]);
        }
    }

    // ---- Status ----

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
                self.exiting.push(Exiting {
                    session: session.session.clone(),
                    tmux_pane: session.tmux_pane.clone(),
                    project: session.project.clone(),
                    child,
                    ended_at: None,
                });
                format!("exiting {} …", session.project)
            }
            Err(error) => format!("⚠ cannot exit {}: {error}", session.project),
        }
    }

    pub(crate) fn selected_session(&self) -> Option<Session> {
        if self.sidebar_selection_lost {
            return None;
        }
        let crate::teams::sidebar::SidebarRow::Session { session_id } =
            self.sidebar_rows.get(self.panels[1].selected)?
        else {
            return None;
        };
        self.sessions
            .iter()
            .find(|session| session.session == *session_id)
            .cloned()
    }

    fn team_is_inactive(
        &self,
        snapshot: &Value,
        task: &crate::teams::TaskKey,
        sessions: &[Session],
    ) -> bool {
        let is_done = self.todos.items.iter().any(|item| {
            item.id() == task.task_id
                && item.status() == "done"
                && !item.cwd().is_empty()
                && (task.project == format!("path:{}", item.cwd())
                    || crate::teams::sidebar::list(snapshot, "sessions")
                        .iter()
                        .any(|session| {
                            session["project"] == task.project && session["cwd"] == item.cwd()
                        }))
        });
        is_done
            && crate::teams::sidebar::attention(snapshot, task).is_none()
            && !sessions.iter().any(|session| {
                crate::teams::sidebar::member_task(snapshot, session).as_ref() == Some(task)
                    && session.asleep.is_none()
                    && (session.needs_attention() || sessions::activity_label(session) != "Ready")
            })
    }

    fn team_task_title(&self, snapshot: &Value, task: &crate::teams::TaskKey) -> String {
        use crate::teams::sidebar;
        let belongs = |cwd: &str| {
            !cwd.is_empty()
                && (task.project == format!("path:{cwd}")
                    || sidebar::list(snapshot, "sessions")
                        .iter()
                        .any(|session| session["project"] == task.project && session["cwd"] == cwd))
        };
        if let Some(item) = self
            .todos
            .items
            .iter()
            .find(|item| item.id() == task.task_id && belongs(&item.cwd()))
            && !item.text().trim().is_empty()
        {
            return sidebar::shown(&item.text());
        }
        if let Some(issue) = self.reviews.issues().iter().find(|issue| {
            issue.key() == task.task_id
                && self
                    .reviews
                    .checkouts
                    .get(&issue.full_repository)
                    .is_some_and(|cwd| belongs(cwd))
        }) {
            return sidebar::shown(&issue.title);
        }
        sidebar::shown(&task.task_id)
    }

    fn select_sidebar_target(&mut self, target: crate::teams::sidebar::SidebarRow) -> bool {
        use crate::teams::sidebar::{self, SidebarRow};
        let task = match &target {
            SidebarRow::Team { task } => Some(task.clone()),
            SidebarRow::Session { session_id } => self
                .sessions
                .iter()
                .find(|session| session.session == *session_id)
                .and_then(|session| match &self.team_links {
                    TeamLinks::Ready(snapshot) => sidebar::member_task(snapshot, session),
                    _ => None,
                }),
            _ => None,
        };
        if let Some(task) = task {
            self.collapsed_teams.remove(&sidebar::task_key(&task));
        }
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
            TeamLinks::Ready(snapshot) => {
                sidebar::grouped_rows(snapshot, &self.sessions, &HashSet::new()).unwrap_or_default()
            }
            _ => self
                .sessions
                .iter()
                .map(|session| SidebarRow::Session {
                    session_id: session.session.clone(),
                })
                .collect(),
        };
        let current = self.sidebar_rows.get(self.panels[1].selected);
        let selected_index = current.and_then(|current| rows.iter().position(|row| row == current));
        let candidates: Vec<_> = rows.iter().enumerate().filter(|(_, row)| match row {
            SidebarRow::Session { session_id } => self.sessions.iter().find(|session| session.session == *session_id).is_some_and(|session| self.is_actionable(session, util::now_secs())),
            SidebarRow::Team { task } => matches!(&self.team_links, TeamLinks::Ready(snapshot) if sidebar::attention(snapshot, task).is_some()
                && !self.sessions.iter().any(|session| sidebar::member_task(snapshot, session).as_ref() == Some(task) && self.is_actionable(session, util::now_secs()))),
            _ => false,
        }).collect();
        let Some((_, target)) = candidates
            .iter()
            .find(|(index, _)| selected_index.is_none_or(|selected| *index > selected))
            .or(candidates.first())
        else {
            if !self.held.is_empty() {
                return "only greyproxy needs you: G reviews its held requests".into();
            }
            return "nothing needs you".into();
        };
        let target = (*target).clone();
        if !self.select_sidebar_target(target.clone()) {
            return "Selected session unavailable; choose a row".into();
        }
        if let SidebarRow::Session { session_id } = target {
            let attended = self.manager_async(&["attend", &session_id]);
            self.mark_result_seen(&session_id).unwrap_or(attended)
        } else {
            String::new()
        }
    }

    fn open_session_details(&mut self) {
        let Some(selected) = self.selected_session() else {
            return;
        };
        let session = &selected;
        let now = util::now_secs();
        let card = self.session_card(session, now);
        let original = self
            .session_task_titles
            .get(&session.session)
            .map(String::as_str)
            .unwrap_or(&session.project);
        let branch = self
            .session_branches
            .get(&session.session)
            .map(String::as_str)
            .unwrap_or("");
        let shown = self.session_display_title(session);
        let todo = self
            .todos
            .items
            .iter()
            .find(|item| item.session() == session.session)
            .map(|item| item.id())
            .or_else(|| {
                branch
                    .strip_prefix("todo-")
                    .filter(|id| self.todos.items.iter().any(|item| item.id() == *id))
                    .map(str::to_string)
            });
        let mut text = if shown == session.project {
            session.project.clone()
        } else {
            format!("{} › {shown}", session.project)
        };
        if let Some(todo) = &todo {
            text += &format!(" · todo {todo}");
        }
        text += "\n";
        if original != self.session_display_title(session) && original != session.project {
            text += &format!("Task: {original}\n");
        }
        text += "\nWHERE & WHO\n";
        let mut place = Vec::new();
        if sessions::clone_name(&session.cwd).is_some() || !branch.is_empty() {
            if let Some(clone) = sessions::clone_name(&session.cwd) {
                place.push(format!("clone {clone}"));
            }
            if !branch.is_empty() {
                place.push(format!("branch {branch}"));
            }
        } else if !session.cwd.is_empty() {
            place.push(session.cwd.clone());
        }
        if session.unpushed > 0 {
            place.push(format!("↑{} unpushed", session.unpushed));
        }
        if !place.is_empty() {
            text += &format!("{}\n", place.join(" · "));
        }
        let facts = &session.facts;
        let mut who = vec![session.agent().label.to_string()];
        if !facts.model.is_empty() {
            who.push(facts.model.clone());
        }
        if !facts.effort.is_empty() {
            who.push(format!("effort {}", facts.effort));
        }
        if let Some(context) = facts.context {
            who.push(format!("ctx {}%", context.round()));
        }
        if session.since > 0 {
            who.push(format!(
                "since {}",
                chrono::DateTime::from_timestamp(session.since, 0)
                    .map(|time| time
                        .with_timezone(&chrono::Local)
                        .format("%H:%M")
                        .to_string())
                    .unwrap_or_default()
            ));
        }
        let plan = if session.credits {
            "API credits".to_string()
        } else if session.agent().name == "claude" && !self.plan.is_empty() {
            format!("Claude {} (subscription)", self.plan)
        } else {
            "subscription".to_string()
        };
        let cost = self
            .costs
            .session_today(&session.session)
            .map(|cost| format!(" · ≈ ${cost:.2} at API prices today"))
            .unwrap_or_default();
        text += &format!("{}\nBilling: {plan}{cost}\n\nAROUND IT\n", who.join(" · "));
        let mut around = Vec::new();
        if let Some(answers) = self.chief_answers.get(&session.session) {
            around.push(format!("◆ Chief answered {}", answers.count));
        }
        if let Some(team) = self.team_roles.get(&session.session) {
            around.push(format!("⇄ {team}"));
        }
        let queued = self
            .session_queued
            .get(&session.session)
            .copied()
            .unwrap_or(0);
        if queued > 0 {
            around.push(format!(
                "✉ {queued} queued message{}",
                if queued == 1 { "" } else { "s" }
            ));
        }
        if let Some(background) = sessions::background_badge(session) {
            around.push(format!("◷ {background}"));
        }
        if facts.remote {
            around.push("⇅ remote control on".into());
        }
        text += &format!(
            "{}\n\nMY NOTE (never sent to the agent)\n",
            if around.is_empty() {
                "nothing else around it".into()
            } else {
                around.join(" · ")
            }
        );
        let note = self
            .continuity
            .as_ref()
            .ok()
            .and_then(|continuity| continuity.note(&session.session).map(str::to_string));
        text += &format!(
            "{}\n\n",
            note.unwrap_or_else(|| "none · w writes one".into())
        );
        if session.needs_attention() {
            let request = if !session.full_request.is_empty() {
                session.full_request.clone()
            } else if !session.request.is_empty() {
                session.request.clone()
            } else {
                sessions::activity_text(session)
            };
            text += &format!("ITS ASK\n{request}\n\n");
        }
        if !session.outcome.is_empty() {
            text += &format!("LAST TURN\n{}\n\n", session.outcome);
        }
        if let Some(answers) = self.chief_answers.get(&session.session) {
            let room = (self.width as usize).saturating_sub(6).max(2);
            text += "CHIEF ANSWERS\n";
            for answer in answers.recent.iter().take(3) {
                for line in crate::chief::view::recent_answer_lines(answer, room) {
                    text += &format!("{line}\n");
                }
            }
            text += "c all answers\n\n";
        }
        if let TeamLinks::Ready(snapshot) = &self.team_links
            && let Some(task) = crate::teams::sidebar::member_task(snapshot, session)
        {
            let member = crate::teams::sidebar::membership(snapshot, session)
                .expect("task came from membership");
            text += &format!(
                "Session: {}\nAgent: {}\nTeam: {} / {}\nRole: {}\n{}\n",
                session.session,
                session.agent,
                task.project,
                task.task_id,
                member["role"].as_str().unwrap_or("Member"),
                crate::teams::sidebar::handoff_summary(snapshot, &task)
            );
            text = crate::approval::escape_hiding(&text, "", "");
        }
        if session.last_result.is_none() && !session.reports_turns {
            text += "Result tracking unavailable; restart to enable\n\n";
        }
        let agent = session.agent();
        let key_groups = SESSION_DETAIL_KEYS
            .iter()
            .map(|(heading, keys)| {
                (
                    *heading,
                    keys.iter()
                        .copied()
                        .filter(|(key, _)| {
                            let key = Key::Char(key.chars().next().unwrap());
                            !(session.is_external() && session_key_requires_terminal(key.clone()))
                                && (key != Key::Char('c')
                                    || self.chief_answers.contains_key(&session.session))
                                && key_feature(key, agent)
                                    .is_none_or(|(feature, _)| agent.can(feature))
                        })
                        .collect(),
                )
            })
            .collect();
        let displayed_updates = match &self.continuity {
            Ok(state) => state
                .updates()
                .into_iter()
                .filter(|update| update.session == session.session)
                .map(|update| {
                    text += &format!("\nUNSEEN UPDATE\n{}\n", update.text);
                    update.identity
                })
                .collect(),
            Err(error) => {
                text += &format!("\nUpdates unavailable: {error}\n");
                Vec::new()
            }
        };
        self.session_details = Some(SessionDetails {
            title: format!("{} {}", card.glyph, card.doing),
            text,
            session: Some(session.session.clone()),
            key_groups,
            displayed_updates,
            lead: Some(session.project.clone()),
            ..Default::default()
        });
    }

    /// The selected session's keys, most pressing first: what its card asks for (an approval, a
    /// restart, /compact, a push…), then the everyday keys. The footer cuts the end of the list
    /// when the column is narrow, so the order is what keeps the useful keys on screen.
    /// What the running copy a card's clone applies to holds and GitHub lacks; 0 for other cards.
    fn running_copy_unpushed(&self, cwd: &str) -> usize {
        sessions::running_copy_of(cwd)
            .and_then(|live| self.running_copies_unpushed.get(&live).copied())
            .unwrap_or(0)
    }

    /// Restart every session sitting at its prompt now. One that works or asks for a permission
    /// would lose its turn: it restarts once that turn ends (dispatch_restarts). The session that
    /// asked for the apply is always one of those, still in its turn when the apply lands.
    pub(crate) fn restart_all(&mut self, screen: &mut dyn Screen) -> String {
        // Only the sessions started before a change they do not have yet (the "⟳ restart" badge);
        // a terminal session is its terminal's and is left alone.
        let stale: Vec<&Session> = self
            .running_sessions
            .iter()
            .filter(|s| {
                !s.is_external()
                    && !s.terminal
                    && !sessions::changed_since_start(s, &self.start_files).is_empty()
            })
            .collect();
        let ready: Vec<Session> = stale
            .iter()
            .filter(|s| s.state == "waiting")
            .map(|s| (*s).clone())
            .collect();
        let busy: Vec<Session> = stale
            .iter()
            .filter(|s| s.state == "working" || s.state == "needs-you")
            .map(|s| (*s).clone())
            .collect();
        if ready.is_empty() && busy.is_empty() {
            return "no session needs a restart".into();
        }
        let plural = |count: usize| if count > 1 { "s" } else { "" };
        let now_part = if ready.is_empty() {
            String::new()
        } else {
            format!("restart {} session{} now", ready.len(), plural(ready.len()))
        };
        let later_part = if busy.is_empty() {
            String::new()
        } else {
            format!(
                "{} busy one{} at the end of its turn",
                busy.len(),
                plural(busy.len())
            )
        };
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
                    util::log_event(
                        "WARN",
                        &format!(
                            "restart-all could not queue {} for the end of its turn: {error}",
                            session.project
                        ),
                    );
                    not_queued.push(format!("{} ({error})", session.project));
                }
            }
        }
        if !ready.is_empty() {
            let mut arguments = vec!["restart-all"];
            arguments.extend(ready.iter().map(|s| s.session.as_str()));
            manager_in_background(&arguments);
        }
        let mut message = if ready.is_empty() {
            String::new()
        } else {
            format!("restarting {} …", ready.len())
        };
        if !queued.is_empty() {
            message += &format!(
                "{}at the end of their turn: {}",
                if message.is_empty() { "" } else { " " },
                queued.join(", ")
            );
        }
        if !not_queued.is_empty() {
            message += &format!(
                "{}not queued, restart by hand: {}",
                if message.is_empty() { "" } else { " " },
                not_queued.join(", ")
            );
        }
        message
    }

    // ---- Pull requests ----

    // ---- Todo ----

    // ---- The whole column ----

    fn panel_hints(&self, index: usize) -> Vec<(&'static str, &'static str)> {
        match index {
            0 => {
                let mut hints = vec![("Esc", "close"), ("p", "pull requests"), ("5", "Chief tab")];
                if !self.status_selection_lost
                    && let Some(Some(action)) = self.status_actions.get(self.panels[0].selected)
                {
                    hints.insert(0, ("⏎", action.hint()));
                }
                hints
            }
            1 => self.sessions_hints(),
            2 => vec![
                ("⏎", "review"),
                ("o", "GitHub"),
                ("l", "later / bring back"),
                ("t", "trail"),
                ("Esc", "close"),
            ],
            4 => vec![("y", "approve"), ("n", "decline"), ("i", "details")],
            TEAMS_PANEL => self.teams_hints(),
            CHIEF_PANEL => self.chief_tab_hints(),
            _ => self.todo_hints(),
        }
    }

    fn panel_help(index: usize) -> Vec<(&'static str, &'static str)> {
        match index {
            0 => vec![
                (
                    "⏎",
                    "open selected service status, daily digest or clones review",
                ),
                ("↑↓", "select a service or utility"),
                ("p", "its pull requests"),
                ("5", "the Chief tab: on / off, answers, habits"),
                ("s", "back to the tab"),
            ],
            1 => Self::sessions_help(),
            2 => vec![
                ("o", "open the pull request in the pull requests window"),
                (
                    "⏎",
                    "a session reviewing it, in its own clone of the repository, using the default agent",
                ),
                (
                    "l",
                    "later for 20 minutes / bring back now; never marks reviewed",
                ),
                ("t", "task trail for this pull request"),
                (
                    "P",
                    "the pull requests window on the viewed session's repository (or your review requests)",
                ),
                ("✓ ● ✗", "CI passing, running, failing"),
                (
                    "approved",
                    "review state: approved, changes (requested), review (required)",
                ),
            ],
            4 => vec![
                (
                    "y",
                    "approve the selected idea: it moves to its project's To do, without starting work",
                ),
                (
                    "n",
                    "decline the selected idea; the declined title will not be proposed again",
                ),
                (
                    "i ⏎",
                    "the full idea, reason, notes and source session from any project",
                ),
            ],
            TEAMS_PANEL => teams_tab::help(),
            CHIEF_PANEL => chief_tab::help(),
            _ => vec![
                (
                    "⏎",
                    "the item's details: title, notes, why Claude proposed it, who filed it, when",
                ),
                (
                    "[ ]",
                    "previous / next tab: Open (Proposed, To do and In progress together) · Proposed · To do · In progress · Done",
                ),
                (
                    "h",
                    "hand the item over: the session on screen if it is on the item's project (a parallel clone too), Claude Code or opencode alike, else the project's running session, or a new one using the default agent",
                ),
                (
                    "r",
                    "run now: a new session on it using the default agent, in a parallel clone of its repository on a branch named after it (todo-<id>, issue-<n>), never a running one",
                ),
                (
                    "e",
                    "edit Kind, Title and Notes in one form; Tab / Shift+Tab fields, arrows choose Kind; CMD+S saves, Esc cancels",
                ),
                (
                    "K",
                    "change kind: Task, Feature, Bug, Improvement, Chore, Research; Design/Blocked routing stays unchanged",
                ),
                (
                    "m",
                    "move a typed item to another project, picked like a new session's; not while it is in progress",
                ),
                (
                    "f",
                    "filter by kind, or All (including GitHub issues); each viewed session keeps its filter",
                ),
                (
                    "y n",
                    "approve or decline an idea Claude proposed; a declined idea is not proposed again",
                ),
                (
                    "Q",
                    "queue it (again: unqueue): a session of its project idle for 5 min at its prompt takes the first queued item, by priority, one at a time",
                ),
                (
                    "t",
                    "add an item for the viewed session's project (or pick a project): Kind, Title and Notes; CMD+S saves, Esc cancels",
                ),
                (
                    "i",
                    "details; i inside details marks in progress / back to do",
                ),
                ("T", "task trail (also inside details)"),
                (
                    "d",
                    "mark done, or back to to do; done items stay 7 days, dimmed at the bottom",
                ),
                ("+", "raise a typed item's priority: ! then !! up to !!!!"),
                ("-", "lower it"),
                ("x", "remove a typed item"),
                (
                    "! ↓",
                    "raised and low priority; an issue's comes from its labels (P0/P1/urgent/high, P3/P4/low)",
                ),
                (
                    "? • ◐ ✓",
                    "proposed · to do · in progress · done; ◇ an open GitHub issue still to do",
                ),
                (
                    "",
                    "Agents mark items with cockpit-todo (start, done, add) and propose ideas (propose)",
                ),
            ],
        }
    }

    pub(crate) fn subtitle(&self, index: usize) -> String {
        match index {
            0 => self.status_subtitle(),
            1 => String::new(),
            2 => self.reviews_subtitle(),
            4 => "all projects".into(),
            TEAMS_PANEL | CHIEF_PANEL => String::new(),
            _ => self.todo_subtitle(),
        }
    }

    /// Status (usage, spending, pull requests, services) shows over the tabs: `s` opens it.
    pub(crate) fn is_status_open(&self) -> bool {
        matches!(self.focus, 0 | 2)
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

    fn handle_click(&mut self, x: u16, y: u16, double: bool, screen: &mut dyn Screen) {
        if y == HEADER_LINES
            && let Some((_, _, panel)) = self
                .tab_ranges
                .iter()
                .find(|(start, end, _)| (*start..*end).contains(&x))
                .copied()
        {
            self.focus_panel(panel);
            return;
        }
        if let Some((start, end, action @ ("next-attention" | "session-details"))) =
            self.hit_actions.get(&y).copied()
            && (start..end).contains(&x)
        {
            self.focus_panel(1);
            self.message = self.handle_sessions(
                Key::Char(if action == "next-attention" { 'n' } else { 'i' }),
                screen,
            );
            return;
        }
        if let Some((start, end, "status")) = self.hit_actions.get(&y).copied()
            && (start..end).contains(&x)
        {
            self.focus_panel(if self.focus == 0 {
                self.previous_panel
            } else {
                0
            });
            return;
        }
        let Some((panel_index, item)) = self.hit_lines.get(&y).copied() else {
            return;
        };
        self.focus_panel(panel_index);
        if let Some(item) = item {
            self.panels[panel_index].selected = item;
            if panel_index == 0 {
                self.status_selection_lost = false;
            }
            if panel_index == 1 {
                self.sidebar_selection_lost = false;
            }
        }
        if let Some((start, end, "effort")) = self.hit_actions.get(&y).copied()
            && (start..end).contains(&x)
        {
            if let Some(session) = self.selected_session() {
                self.message = self.open_effort_menu(session, Some(y));
            }
            return;
        }
        if double && item.is_some() {
            self.message = self.handle_panel(Key::Enter, screen);
        }
    }

    fn handle_panel(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        match self.focus {
            0 => match key {
                Key::Enter => {
                    if self.status_selection_lost {
                        return "Status selection changed: choose a row".into();
                    }
                    match self
                        .status_actions
                        .get(self.panels[0].selected)
                        .copied()
                        .flatten()
                    {
                        Some(StatusAction::ClaudeStatus) => {
                            let _ =
                                util::spawn_detached(Command::new("xdg-open").arg(status::PAGE));
                            "opening status.claude.com …".into()
                        }
                        Some(StatusAction::CodexStatus) => {
                            let _ = util::spawn_detached(
                                Command::new("xdg-open").arg(status::CODEX_PAGE),
                            );
                            "opening status.openai.com …".into()
                        }
                        Some(StatusAction::Digest) => self.manager_async(&["digest"]),
                        Some(StatusAction::Clones) => {
                            manager_in_background(&["clones"]);
                            String::new()
                        }
                        None => String::new(),
                    }
                }
                _ => String::new(),
            },
            1 => self.handle_sessions(key, screen),
            2 => self.handle_reviews(key, screen),
            4 => self.handle_ideas(key),
            TEAMS_PANEL => self.handle_teams(key, screen),
            CHIEF_PANEL => self.handle_chief_tab(key),
            _ => self.handle_todo(key, screen),
        }
    }

    pub fn handle(&mut self, key: Key, screen: &mut dyn Screen) {
        if key == Key::Resize {
            return;
        }
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
        if matches!(key, Key::Paste(_))
            && self.chief_view.as_ref().is_none_or(|view| view.suspended)
        {
            return;
        }
        if let Some(todo) = self
            .session_details
            .as_ref()
            .and_then(|details| details.todo.clone())
        {
            if key == Key::Char('T') {
                self.message =
                    self.open_task_trail(crate::continuity::TrailTarget::Task(todo), screen);
                return;
            }
            if key == Key::Char('i') {
                self.session_details = None;
                let Some(index) = self
                    .todo_entries
                    .iter()
                    .position(|entry| crate::continuity::task_key(entry) == todo)
                else {
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
        if self
            .session_details
            .as_ref()
            .is_some_and(|details| !details.actions.is_empty())
            && key == Key::Enter
        {
            self.message = self.pick_detail_action(screen);
            return;
        }
        // Session details: its keys close it and act on the session, as on the card.
        if let Some(details) = self
            .session_details
            .as_ref()
            .filter(|details| details.session.is_some())
        {
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
                    self.review_updates(
                        session.as_deref().expect("session details have a target"),
                        &displayed_updates,
                    )
                } else {
                    self.handle_sessions(key, screen)
                };
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
                Key::Char('c') => {
                    if let Some(session) = details.chief_answers_of.take() {
                        self.session_details = None;
                        self.message = self.open_chief_answers(Some(&session));
                    }
                }
                _ => {}
            }
            return;
        }
        if self.chief_view.is_some() && self.handle_chief_key(key.clone()) {
            return;
        }
        if self.team_view.as_ref().is_some_and(|view| !view.hidden) {
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
        // CMD+1/2/3 from the session arrive as F1-F3 (tmux.conf); F4 and F5 open Teams and Chief.
        if let Key::F(number @ 1..=5) = key {
            self.focus_panel(TAB_PANELS[number as usize - 1]);
            return;
        }
        match key {
            Key::Click { x, y, double } => self.handle_click(x, y, double, screen),
            Key::Char('s') => self.focus_panel(if self.is_status_open() {
                self.previous_panel
            } else {
                0
            }),
            Key::Esc if self.is_status_open() => self.focus_panel(self.previous_panel),
            Key::Char('p') if self.focus == 0 => self.focus_panel(2),
            // "4 team details" on a team block: the Teams tab opens on that team.
            Key::Char('4') if self.focus == 1 && self.selected_sidebar_task().is_some() => {
                let task = self.selected_sidebar_task().expect("checked above");
                self.message = self.open_teams_tab_on(&task);
            }
            Key::Char(digit @ '1'..='5') => {
                self.focus_panel(TAB_PANELS[digit as usize - '1' as usize])
            }
            Key::Char('j') | Key::Down => {
                self.panels[self.focus].move_by(1);
                if self.focus == 0 {
                    self.status_selection_lost = false;
                }
                if self.focus == 1 {
                    self.sidebar_selection_lost = false;
                }
            }
            // In Sessions `k` opens the selected card's links (Theo, 2026-10-26); ↑ moves up there.
            Key::Char('k') if self.focus != 1 => {
                self.panels[self.focus].move_by(-1);
                if self.focus == 0 {
                    self.status_selection_lost = false;
                }
            }
            Key::Up => {
                self.panels[self.focus].move_by(-1);
                if self.focus == 0 {
                    self.status_selection_lost = false;
                }
                if self.focus == 1 {
                    self.sidebar_selection_lost = false;
                }
            }
            // From any panel, as ← comes back from the session (tmux.conf).
            Key::Right => {
                let _ = Command::new("cockpit").arg("focus-viewer").status();
            }
            Key::Char('?') => {
                self.help_open = true;
                self.help_scroll = 0;
            }
            Key::Char('N') => self.open_new_session(),
            Key::Char('S') => self.settings_menu = Some(SettingsMenu::new()),
            Key::Char('H') => {
                let past = self.past_sessions();
                if let Some(index) = screen
                    .outside(&mut || pick_past_session(&past))
                    .and_then(|chosen| chosen.parse::<usize>().ok())
                {
                    let chosen = &past[index].0;
                    self.message = match queue::reserve_resume(&chosen.cwd, &chosen.session) {
                        Ok(token) => match queue::launch_reserved(
                            &token,
                            &["resume", &chosen.cwd, &chosen.session],
                            true,
                        ) {
                            Ok(()) => "resuming conversation".into(),
                            Err(error) => error,
                        },
                        Err(error) => error,
                    };
                }
            }
            Key::Char('P') => self.message = self.manager_async(&["prs"]),
            Key::Char('W') => self.message = open_in_scratchpad("claude", ""),
            Key::Char('G') => {
                self.message = open_in_scratchpad("greyproxy", "http://localhost:43080/pending")
            }
            Key::Char('q') | Key::Ctrl('c') => {
                if self.confirm(screen, "close the manager window? sessions keep running") {
                    let _ = Command::new("tmux").arg("detach-client").status();
                }
            }
            _ => self.message = self.handle_panel(key, screen),
        }
    }

    fn launch_design_session(
        &mut self,
        directory: &str,
        canvas_link: &str,
    ) -> Result<String, String> {
        let url = sessions::canvas_address(canvas_link).ok_or_else(|| {
            "not a canvas link: claude.ai/artifact/<id>, or a chat link with ?artifact=<id>"
                .to_string()
        })?;
        manager_in_background(&["new", directory, &sessions::design_prompt(&url)]);
        Ok(format!(
            "starting a design session in {} …",
            project_name(directory)
        ))
    }

    /// `m` on a card: Remote Control (phone / claude.ai/code) on or off.
    fn toggle_remote(&mut self) -> String {
        match self.selected_session().filter(|s| s.state != "unknown") {
            Some(s) if s.is_external() => {
                "external session: no terminal target; use its own window".into()
            }
            Some(s) if !s.agent().can(Feature::RemoteControl) => format!(
                "Remote Control is not {}'s to do: {} runs {}",
                s.agent().label,
                s.project,
                s.agent().name
            ),
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
            Some(s)
                if self.confirm(
                    screen,
                    &format!(
                        "drop the {} prompt(s) queued for {}?",
                        prompts::count(&s.session),
                        s.project
                    ),
                ) =>
            {
                prompts::clear(&s.session);
                "queued prompts dropped".into()
            }
            _ => String::new(),
        }
    }

    fn team_control(action: crate::teams::HostAction) -> Result<Value, crate::teams::TeamError> {
        crate::teams::client::host_request(&crate::teams::HostRequest::Control { action })
            .and_then(|reply| reply.result)
    }

    fn team_snapshot() -> Result<Value, crate::teams::TeamError> {
        crate::teams::client::host_request(&crate::teams::HostRequest::Snapshot)
            .and_then(|reply| reply.result)
    }

    /// Every 10 s: what in the linked teams waits for Theo, and each member's role, for the cards.
    pub(crate) fn check_team(&mut self) {
        let now = util::now();
        if now - self.team_checked < 10.0 {
            return;
        }
        self.team_checked = now;
        self.chief_answers.clear();
        // The full Chief snapshot is read only while it shows; the tab's badge comes with the team snapshot.
        if matches!(self.focus, 0 | CHIEF_PANEL) {
            self.refresh_chief_status();
        }
        if self.chief_view.is_some() {
            self.refresh_chief_view();
        }
        util::refresh_hyprland_instance();
        let workspace = util::output_of("hyprctl", &["-j", "activeworkspace"])
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|workspace| workspace["id"].as_i64());
        if let Some(workspace) = workspace {
            let _ = crate::teams::client::host_request(&crate::teams::HostRequest::ChiefFocus {
                viewed: current_session(),
                workspace_active: workspace == util::CLAUDE_WORKSPACE,
                gaming: (self.gaming)(),
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
        self.chief_answers = snapshot["chief_answers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let count = entry["count"].as_u64().filter(|count| *count > 0)? as usize;
                Some((
                    entry["session_id"].as_str()?.to_string(),
                    ChiefAnswers {
                        count,
                        recent: entry["recent"].as_array().cloned().unwrap_or_default(),
                    },
                ))
            })
            .collect();
        self.team_attention = crate::teams::view::TeamView::attention(&snapshot);
        self.qa_sessions = snapshot["members"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(snapshot["spawns"].as_array().into_iter().flatten())
            .filter(|row| row["role"] == "qa")
            .filter_map(|row| row["session_id"].as_str().map(str::to_string))
            .collect();
        self.team_roles = snapshot["members"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|member| {
                Some((
                    member["session_id"].as_str()?.to_string(),
                    format!(
                        "team {} · {}",
                        member["role"].as_str()?,
                        member["task_id"].as_str()?
                    ),
                ))
            })
            .collect();
        self.team_links = TeamLinks::Ready(snapshot);
    }

    fn open_team_view(&mut self) -> String {
        match Self::team_snapshot() {
            Ok(snapshot) => {
                self.team_view = Some(crate::teams::view::TeamView::new(snapshot));
                String::new()
            }
            Err(error) => format!("Team view: {}", error.message),
        }
    }

    /// Re-read after every change; the first pending request is selected when `to_request` says so.
    fn refresh_team_view(&mut self, to_request: bool) {
        let snapshot = match Self::team_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if let Some(view) = self.team_view.as_mut() {
                    view.snapshot_error =
                        Some(crate::approval::escape_hiding(&error.message, "", ""));
                }
                return;
            }
        };
        if let Some(view) = self.team_view.as_mut() {
            view.snapshot = snapshot;
            view.snapshot_error = None;
            view.detail_scroll = 0;
            let rows = view.rows();
            view.selected = if to_request {
                0
            } else {
                view.selected.min(rows.len().saturating_sub(1))
            };
        }
    }

    fn handle_team_view(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let Some(view) = self.team_view.as_mut() else {
            return String::new();
        };
        if view.snapshot_error.is_some()
            && (key == Key::Char('l')
                || (matches!(
                    view.selected_row(),
                    Some(
                        crate::teams::view::Row::LinkProposal(_) | crate::teams::view::Row::Link(_)
                    )
                ) && matches!(key, Key::Char('a' | 'e' | 'x' | 'u'))))
        {
            return "Collaboration status unavailable: r refresh before changing links".into();
        }
        match key {
            Key::Esc | Key::Char('q') => {
                self.team_view = None;
                String::new()
            }
            Key::Tab => {
                view.is_history = !view.is_history;
                view.selected = 0;
                view.detail_scroll = 0;
                String::new()
            }
            Key::Char('j') | Key::Down => {
                view.move_by(1);
                String::new()
            }
            Key::Char('k') | Key::Up => {
                view.move_by(-1);
                String::new()
            }
            Key::Right => {
                let lines = view
                    .detail()
                    .iter()
                    .flat_map(|text| {
                        wrap_detail_text(text, self.width.saturating_sub(6).max(2) as usize)
                    })
                    .count();
                view.detail_scroll = view
                    .detail_scroll
                    .saturating_add(1)
                    .min(lines.saturating_sub(1));
                String::new()
            }
            Key::Left => {
                view.detail_scroll = view.detail_scroll.saturating_sub(1);
                String::new()
            }
            Key::Char('r') => {
                self.refresh_team_view(false);
                self.team_view
                    .as_ref()
                    .and_then(|view| view.snapshot_error.as_ref())
                    .map(|error| format!("Collaboration snapshot unavailable: {error}"))
                    .unwrap_or_default()
            }
            Key::Char('n') => self.create_team_request(screen),
            Key::Char('l') => self.create_link_request(screen),
            _ => match view.selected_row() {
                Some(row) => self.team_row_action(row, key, screen),
                None => String::new(),
            },
        }
    }

    fn team_row_action(
        &mut self,
        row: crate::teams::view::Row,
        key: Key,
        screen: &mut dyn Screen,
    ) -> String {
        use crate::teams::{HostAction, TaskKey, view::Row};
        let token = || format!("cockpit-{}", util::token_hex(8));
        let view = self.team_view.as_ref().expect("the Team view is open");
        let outcome = match (row, key) {
            (Row::LinkProposal(index), key @ Key::Char('a' | 'e' | 'x')) => {
                return self.link_request_action(index, key, screen);
            }
            (Row::Link(index), Key::Char('u')) if view.link(index)["state"] == "active" => {
                let link_id = view.link(index)["link_id"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                if !self.confirm(screen, &format!("revoke link {link_id}? queued messages and safe retries are cancelled; submitted messages cannot be recalled")) { return String::new() }
                return self.finish_link_action(
                    HostAction::LinkRevoke {
                        request_id: token(),
                        link_id,
                    },
                    "link revoked; queued messages and safe retries cancelled",
                );
            }
            (Row::Proposal(index), Key::Char('a')) => {
                return self.approve_team_request(index, screen);
            }
            (Row::Proposal(index), Key::Char('e')) => return self.edit_team_request(index, screen),
            (Row::Proposal(index), Key::Char('x')) => {
                let proposal = view.proposal(index).clone();
                if !self.confirm(screen, "reject this team request? no links are made") {
                    return String::new();
                }
                Self::team_control(HostAction::Reject {
                    request_id: token(),
                    proposal_id: proposal["proposal_id"].as_str().unwrap_or("").into(),
                    expected_revision: proposal["revision"].as_i64().unwrap_or(0),
                })
                .map(|_| "request rejected: no links were made".to_string())
            }
            (Row::Member(index), Key::Char('u')) => {
                let member = view.member(index).clone();
                let text = |key: &str| member[key].as_str().unwrap_or("").to_string();
                if !self.confirm(
                    screen,
                    &format!(
                        "unlink {} {}? its pending messages and retries are cancelled",
                        text("role"),
                        text("session_id")
                    ),
                ) {
                    return String::new();
                }
                Self::team_control(HostAction::Unlink {
                    request_id: token(),
                    task: TaskKey {
                        project: text("project"),
                        task_id: text("task_id"),
                    },
                    session_id: text("session_id"),
                })
                .map(|_| "unlinked: it can no longer message the team".to_string())
            }
            (Row::Escalation(index), Key::Char('s')) => {
                let thread_id = view.escalation(index)["thread_id"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let note = input_line(screen, "resolution note for the team (empty cancels)> ");
                if note.trim().is_empty() {
                    return String::new();
                }
                Self::team_control(HostAction::Resolve {
                    request_id: token(),
                    thread_id,
                    note,
                })
                .map(|_| "escalation resolved".to_string())
            }
            (Row::FailedStart(index), Key::Char('d')) => {
                let spawn_token = view.snapshot["spawns"][index]["token"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                if !self.confirm(
                    screen,
                    "dismiss this team start? it is not started again either way",
                ) {
                    return String::new();
                }
                Self::team_control(HostAction::Dismiss {
                    request_id: token(),
                    token: spawn_token,
                })
                .map(|_| "dismissed".to_string())
            }
            (Row::Uncertain(index), Key::Char(choice @ ('h' | 'c'))) => {
                let message_id = view.message(index)["message_id"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                let (decision, question) = if choice == 'h' {
                    ("handled", "it reached the session: mark it handled?")
                } else {
                    ("cancel", "cancel this delivery? it is not sent again")
                };
                if !self.confirm(screen, question) {
                    return String::new();
                }
                Self::team_control(HostAction::Reconcile {
                    request_id: token(),
                    message_id,
                    decision: decision.into(),
                })
                .map(|_| format!("delivery {decision}"))
            }
            (Row::FailedDelivery(index), Key::Char('c')) => {
                let message_id = view.message(index)["message_id"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                if !self.confirm(screen, "cancel this failed delivery? it is not sent again") {
                    return String::new();
                }
                Self::team_control(HostAction::Reconcile {
                    request_id: token(),
                    message_id,
                    decision: "cancel".into(),
                })
                .map(|_| "failed delivery cancelled".to_string())
            }
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
                    if receipt["status"] == "pending" {
                        view.is_history = false;
                    }
                    if let Some(index) = view.rows().iter().position(|row| match row {
                        crate::teams::view::Row::LinkProposal(index) => {
                            view.link_proposal(*index)["proposal_id"] == receipt["proposal_id"]
                        }
                        crate::teams::view::Row::Link(index) => {
                            view.link(*index)["link_id"] == receipt["link_id"]
                        }
                        _ => false,
                    }) {
                        view.selected = index;
                    }
                }
                success.into()
            }
            Err(error) => format!(
                "Link action {}: {}. Refresh to inspect current status; no automatic retry.",
                if error.code == crate::teams::ErrorCode::Unavailable {
                    "outcome unknown"
                } else {
                    "failed"
                },
                crate::approval::escape_hiding(&error.message, "", "")
            ),
        }
    }

    fn link_request_action(&mut self, index: usize, key: Key, screen: &mut dyn Screen) -> String {
        use crate::teams::HostAction;
        let proposal = self
            .team_view
            .as_ref()
            .expect("collaboration view open")
            .link_proposal(index)
            .clone();
        if proposal["status"] != "pending" {
            return String::new();
        }
        let Some(proposal_id) = proposal["proposal_id"].as_str() else {
            return "Link request identity unavailable".into();
        };
        let Some(expected_revision) = proposal["revision"].as_i64() else {
            return "Link request revision unavailable".into();
        };
        let request_id = format!("cockpit-{}", util::token_hex(8));
        let (action, success) = match key {
            Key::Char('a') => {
                let Some(mapping_hash) = proposal["mapping_hash"].as_str() else {
                    return "Link request hash unavailable".into();
                };
                if !self.confirm(screen, "approve exactly this link revision/hash, pair, TODO and purpose? messaging only; no team role") { return String::new() }
                (
                    HostAction::LinkApprove {
                        request_id,
                        proposal_id: proposal_id.into(),
                        expected_revision,
                        mapping_hash: mapping_hash.into(),
                    },
                    "link approved: messaging-only for this exact pair and shared TODO",
                )
            }
            Key::Char('x') => {
                if !self.confirm(
                    screen,
                    "reject this exact link request? no messaging permission is granted",
                ) {
                    return String::new();
                }
                (
                    HostAction::LinkReject {
                        request_id,
                        proposal_id: proposal_id.into(),
                        expected_revision,
                    },
                    "link request rejected",
                )
            }
            Key::Char('e') => {
                let Some(requester) = proposal["sender"].as_str() else {
                    return "Link requester unavailable".into();
                };
                let replacement = match self.pick_link(screen, Some(requester)) {
                    Ok(Some((_, replacement))) => replacement,
                    Ok(None) => return String::new(),
                    Err(error) => return error,
                };
                (
                    HostAction::LinkEdit {
                        request_id,
                        proposal_id: proposal_id.into(),
                        expected_revision,
                        replacement,
                    },
                    "edited: the new revision needs its own approval (a)",
                )
            }
            _ => return String::new(),
        };
        self.finish_link_action(action, success)
    }

    /// Only the host's verified live registry is offered, never free-form participant IDs.
    fn pick_link(
        &mut self,
        screen: &mut dyn Screen,
        requester: Option<&str>,
    ) -> Result<Option<(String, crate::teams::LinkProposal)>, String> {
        use crate::teams::{LinkProposal, TaskKey};
        let snapshot = Self::team_snapshot().map_err(|error| error.message)?;
        let sessions = snapshot["sessions"]
            .as_array()
            .ok_or("verified live sessions unavailable")?;
        let display = |value: &str| {
            crate::approval::escape_hiding(value, "", "").replace(['\n', '\r', '\t'], " ")
        };
        let choices = |exclude: Option<&str>| {
            sessions
                .iter()
                .filter(|session| session["session_id"].as_str() != exclude)
                .filter_map(|session| {
                    Some(format!(
                        "{}\t{} · {} · {}",
                        session["session_id"].as_str()?,
                        display(session["agent"].as_str()?),
                        display(session["project"].as_str()?),
                        display(session["label"].as_str().unwrap_or(""))
                    ))
                })
                .collect::<Vec<_>>()
        };
        let requester = if let Some(requester) = requester {
            requester.to_string()
        } else {
            let Some(picked) = screen.outside(&mut || {
                fzf(
                    "link first session> ",
                    &choices(None),
                    &["--delimiter", "\t", "--with-nth", "1.."],
                )
            }) else {
                return Ok(None);
            };
            picked.split('\t').next().unwrap_or("").to_string()
        };
        let first = sessions
            .iter()
            .find(|session| session["session_id"] == requester)
            .ok_or("requester is not a verified live session")?;
        let Some(picked) = screen.outside(&mut || {
            fzf(
                "link peer session> ",
                &choices(Some(&requester)),
                &["--delimiter", "\t", "--with-nth", "1.."],
            )
        }) else {
            return Ok(None);
        };
        let peer_id = picked.split('\t').next().unwrap_or("");
        let peer = sessions
            .iter()
            .find(|session| session["session_id"] == peer_id && peer_id != requester)
            .ok_or("peer is not a distinct verified live session")?;
        let owners: Vec<String> = [first, peer]
            .iter()
            .filter_map(|session| session["project"].as_str())
            .map(|project| format!("{project}\t{project}"))
            .collect();
        let Some(owner) = screen.outside(&mut || {
            fzf(
                "shared TODO owning project> ",
                &owners,
                &["--delimiter", "\t", "--with-nth", "2.."],
            )
        }) else {
            return Ok(None);
        };
        let owner = owner.split('\t').next().unwrap_or("");
        if ![first, peer]
            .iter()
            .any(|session| session["project"] == owner)
        {
            return Err("TODO owner must be one of the verified participant projects".into());
        }
        let task_id = input_line(screen, "shared local TODO id (empty cancels)> ");
        if task_id.trim().is_empty() {
            return Ok(None);
        }
        let purpose = input_line(screen, "link purpose (empty cancels)> ");
        if purpose.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some((
            requester,
            LinkProposal {
                submission_key: format!("host-{}", util::token_hex(8)),
                task: TaskKey {
                    project: owner.into(),
                    task_id: task_id.trim().into(),
                },
                peer_session_id: peer_id.into(),
                purpose,
            },
        )))
    }

    fn create_link_request(&mut self, screen: &mut dyn Screen) -> String {
        let (requester_session_id, proposal) = match self.pick_link(screen, None) {
            Ok(Some(request)) => request,
            Ok(None) => return String::new(),
            Err(error) => return error,
        };
        self.finish_link_action(
            crate::teams::HostAction::LinkPropose {
                request_id: format!("cockpit-{}", util::token_hex(8)),
                requester_session_id,
                proposal,
            },
            "link request created: review its exact mapping, then a approves it",
        )
    }

    fn approve_team_request(&mut self, index: usize, screen: &mut dyn Screen) -> String {
        use crate::teams::{HostAction, Member, Proposal, Role, TaskKey};
        let view = self.team_view.as_ref().expect("the Team view is open");
        let proposal = view.proposal(index).clone();
        let mapping = view.mapping(index);
        let text = |key: &str| proposal[key].as_str().unwrap_or("").to_string();
        if !self.confirm(screen, "approve exactly the request shown?") {
            return String::new();
        }
        let approval = HostAction::Approve {
            request_id: format!("cockpit-{}", util::token_hex(8)),
            proposal_id: text("proposal_id"),
            expected_revision: proposal["revision"].as_i64().unwrap_or(0),
            mapping_hash: text("mapping_hash"),
        };
        match Self::team_control(approval) {
            Ok(_) => {
                self.refresh_team_view(false);
                "team approved: only these sessions can message each other about this task".into()
            }
            Err(error) if error.message.contains("runtime changed") => {
                // Restarted members: the same task, sessions and roles, bound to their new launches.
                let members = mapping
                    .iter()
                    .filter_map(|member| {
                        Some(Member {
                            session_id: member["session_id"].as_str()?.to_string(),
                            role: serde_json::from_value::<Role>(member["role"].clone()).ok()?,
                        })
                    })
                    .collect();
                let replacement = Proposal {
                    submission_key: format!("rebind-{}", util::token_hex(8)),
                    task: TaskKey {
                        project: text("project"),
                        task_id: text("task_id"),
                    },
                    members,
                    reason: text("reason"),
                };
                let edit = HostAction::Edit {
                    request_id: format!("cockpit-{}", util::token_hex(8)),
                    proposal_id: text("proposal_id"),
                    expected_revision: proposal["revision"].as_i64().unwrap_or(0),
                    replacement,
                };
                match Self::team_control(edit) {
                    Ok(_) => {
                        self.refresh_team_view(true);
                        "the sessions restarted: review the same request again, then a approves it"
                            .into()
                    }
                    Err(error) => error.message,
                }
            }
            Err(error) => error.message,
        }
    }

    /// Task id, then a session per role among the verified live ones of one project ("-" for none).
    fn pick_team(
        &mut self,
        screen: &mut dyn Screen,
    ) -> Result<Option<(crate::teams::TaskKey, Vec<crate::teams::Member>)>, String> {
        use crate::teams::{Member, Role, TaskKey};
        let task_id = input_line(
            screen,
            "task id for the team (todo id or owner/repo#N, empty cancels)> ",
        );
        if task_id.trim().is_empty() {
            return Ok(None);
        }
        let sessions = self
            .team_view
            .as_ref()
            .expect("the Team view is open")
            .sessions()
            .to_vec();
        let (mut project, mut members): (Option<String>, Vec<Member>) = (None, Vec::new());
        for (role, name) in [
            (Role::Designer, "designer"),
            (Role::Developer, "developer"),
            (Role::Qa, "qa"),
        ] {
            let choices: Vec<String> = sessions
                .iter()
                .filter(|session| {
                    project
                        .as_ref()
                        .is_none_or(|project| session["project"] == project.as_str())
                })
                .filter(|session| {
                    !members
                        .iter()
                        .any(|member| session["session_id"] == member.session_id.as_str())
                })
                .map(|session| {
                    format!(
                        "{}\t{}  {}  {}",
                        session["session_id"].as_str().unwrap_or(""),
                        session["agent"].as_str().unwrap_or(""),
                        session["project_name"].as_str().unwrap_or(""),
                        session["label"].as_str().unwrap_or("")
                    )
                })
                .chain(std::iter::once("-\t(no one)".to_string()))
                .collect();
            let Some(picked) = screen.outside(&mut || {
                fzf(
                    &format!("{name}> "),
                    &choices,
                    &["--delimiter", "\t", "--with-nth", "2.."],
                )
            }) else {
                return Ok(None);
            };
            let id = picked.split('\t').next().unwrap_or("").to_string();
            if id == "-" {
                continue;
            }
            let session = sessions
                .iter()
                .find(|session| session["session_id"] == id.as_str())
                .ok_or("that session is not a verified live one")?;
            project.get_or_insert_with(|| session["project"].as_str().unwrap_or("").to_string());
            members.push(Member {
                session_id: id,
                role,
            });
        }
        if members.len() < 2 {
            return Err("a team needs at least two members".into());
        }
        Ok(Some((
            TaskKey {
                project: project.unwrap_or_default(),
                task_id: task_id.trim().into(),
            },
            members,
        )))
    }

    fn create_team_request(&mut self, screen: &mut dyn Screen) -> String {
        use crate::teams::{HostAction, Proposal};
        let (task, members) = match self.pick_team(screen) {
            Ok(Some(team)) => team,
            Ok(None) => return String::new(),
            Err(error) => return error,
        };
        let proposal = Proposal {
            submission_key: format!("host-{}", util::token_hex(8)),
            task,
            members,
            reason: "Created by Theo in the cockpit".into(),
        };
        match Self::team_control(HostAction::Propose {
            request_id: format!("cockpit-{}", util::token_hex(8)),
            proposal,
        }) {
            Ok(_) => {
                self.refresh_team_view(true);
                "request created: review its exact mapping, then a approves it".into()
            }
            Err(error) => error.message,
        }
    }

    fn edit_team_request(&mut self, index: usize, screen: &mut dyn Screen) -> String {
        use crate::teams::{HostAction, Proposal};
        let proposal = self
            .team_view
            .as_ref()
            .expect("the Team view is open")
            .proposal(index)
            .clone();
        let (task, members) = match self.pick_team(screen) {
            Ok(Some(team)) => team,
            Ok(None) => return String::new(),
            Err(error) => return error,
        };
        let replacement = Proposal {
            submission_key: format!("edit-{}", util::token_hex(8)),
            task,
            members,
            reason: proposal["reason"].as_str().unwrap_or("").to_string(),
        };
        let edit = HostAction::Edit {
            request_id: format!("cockpit-{}", util::token_hex(8)),
            proposal_id: proposal["proposal_id"].as_str().unwrap_or("").into(),
            expected_revision: proposal["revision"].as_i64().unwrap_or(0),
            replacement,
        };
        match Self::team_control(edit) {
            Ok(_) => {
                self.refresh_team_view(true);
                "edited: the new revision needs its own approval (a)".into()
            }
            Err(error) => error.message,
        }
    }

    /// Past sessions not running now, newest first, with their titles.
    /// Claude's past sessions and opencode's, newest first, the running ones left out.
    pub(crate) fn past_sessions(&mut self) -> Vec<(PastSession, String)> {
        self.history.refresh();
        let running: Vec<&str> = self
            .running_sessions
            .iter()
            .map(|s| s.session.as_str())
            .collect();
        let mut past: Vec<(PastSession, String)> = self
            .history
            .entries
            .iter()
            .map(|entry| (entry.clone(), self.history.title(&entry.session)))
            .collect();
        match sessions::opencode_past_sessions(&paths::opencode_database()) {
            Ok(opencode) => past.extend(opencode),
            Err(error) => util::log_event(
                "ERROR",
                &format!(
                    "H: opencode's past sessions left out ({}): {error}",
                    paths::opencode_database().display()
                ),
            ),
        }
        past.retain(|(entry, _)| !running.contains(&entry.session.as_str()));
        past.sort_by_key(|entry| std::cmp::Reverse(entry.0.since));
        past
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn provider_status_lines(status: &ProviderStatus, room: usize) -> Vec<Line> {
    let (current, error) = status.snapshot();
    let label = status.provider.label().trim_end_matches(" Status");
    let column = width_of(label) + 2;
    let mut lines = Vec::new();
    let Some(current) = current else {
        let text = if !error.is_empty() {
            error
        } else if status.provider.url().is_empty() {
            "not checked".into()
        } else {
            format!("checking {} …", status.provider.host())
        };
        for (index, text) in wrap_detail_text(&text, room.saturating_sub(column).max(2))
            .into_iter()
            .enumerate()
        {
            let mut segments = Vec::new();
            if index == 0 {
                segments.push(segment(0, label, style(FG)));
            }
            segments.push(segment(column as u16, text, dim(FG_DARK)));
            lines.push(Line::new(segments));
        }
        return lines;
    };
    let tint = bold(claude_status_color(&current.indicator));
    let description = if current.indicator == "none"
        && current.degraded.is_empty()
        && current.incidents.is_empty()
    {
        "Operational".into()
    } else {
        current.description
    };
    let description = format!(
        "● {description}{}",
        if error.is_empty() { "" } else { " (stale)" }
    );
    for (index, text) in wrap_detail_text(&description, room.saturating_sub(column).max(2))
        .into_iter()
        .enumerate()
    {
        let mut segments = Vec::new();
        if index == 0 {
            segments.push(segment(0, label, style(FG)));
        }
        segments.push(segment(column as u16, text, tint));
        lines.push(Line::new(segments));
    }
    for detail in current
        .degraded
        .iter()
        .take(3)
        .map(|(name, state)| format!("{name}: {state}"))
        .chain(current.incidents.into_iter().take(2))
    {
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

/// The todos the Chief asked a fresh session for; none when the team store is missing or unreadable.
fn chief_requested_starts() -> HashSet<String> {
    let requested =
        crate::teams::store::open_existing(&crate::paths::team_database()).and_then(|connection| {
            match connection {
                Some(connection) => crate::chief::store::open_session_start_tasks(
                    &connection,
                    chrono::Utc::now().timestamp_millis(),
                ),
                None => Ok(HashSet::new()),
            }
        });
    requested.unwrap_or_else(|error| {
        util::log_event(
            "WARN",
            &format!(
                "queue: cannot read the Chief's session start requests: {}",
                error.message
            ),
        );
        HashSet::new()
    })
}

/// Ends a session gracefully through the manager, or closes its terminal window when it never
/// had a tmux pane, or else tells the daemon it is gone.
/// A session outside tmux (exit_session handles the others): its terminal window, or its card.
fn kill_session(session: &Session) -> String {
    if !session.kitty_pid.is_empty() {
        util::refresh_hyprland_instance();
        let close = format!(
            "hl.dsp.window.close({{ window = \"pid:{}\" }})",
            session.kitty_pid
        );
        let _ = Command::new("hyprctl").args(["dispatch", &close]).output();
        return format!(
            "no tmux session for {}: closed its terminal window instead",
            session.project
        );
    }
    // Nothing to kill: if its hook is still alive it comes back, the sign it lives elsewhere.
    let report = serde_json::json!({"session": session.session, "state": "gone", "project": session.project});
    let _ = Command::new("osd-show")
        .arg(format!("session {report}"))
        .status();
    format!(
        "nothing to kill for {}: removed it from the list",
        session.project
    )
}

/// `I` on a card: the images its latest reply names, copied privately (the phone's rules: inside
/// its folder, raster by their bytes, 10 MB) and opened in one floating loupe on workspace 6. The
/// copies sit alone in a folder of their own, so loupe's arrows browse exactly them.
fn open_reply_images(session: &Session) -> String {
    let images = match remote::latest_reply_images(&session.session, &session.cwd) {
        Ok(images) => images,
        Err(error) => return error,
    };
    if images.is_empty() {
        return "No images in its latest reply.".into();
    }
    let refused: Vec<String> = images
        .iter()
        .filter_map(|(_, image)| image.as_ref().err().cloned())
        .collect();
    let readable: Vec<(&'static str, Vec<u8>)> = images
        .into_iter()
        .filter_map(|(_, image)| image.ok())
        .collect();
    if readable.is_empty() {
        return refused.join("; ");
    }
    let first = match copy_privately(&readable) {
        Ok(first) => first,
        Err(error) => return format!("cannot copy the images: {error}"),
    };
    let first = first.to_string_lossy();
    // The path reaches a shell through Hyprland's exec: only characters sh takes literally.
    if !first
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "/._-".contains(character))
    {
        return format!("cannot open {first:?} safely");
    }
    util::refresh_hyprland_instance();
    let launch = format!("hl.exec_cmd(\"loupe {first}\", {{ workspace = \"6\", float = true }})");
    let count = readable.len();
    let opening = format!(
        "opening {count} image{} from {}",
        if count == 1 { "" } else { "s" },
        session.project
    );
    match Command::new("hyprctl").args(["eval", &launch]).output() {
        Ok(output) if output.status.success() && refused.is_empty() => opening,
        Ok(output) if output.status.success() => format!("{opening}; {}", refused.join("; ")),
        Ok(output) => {
            let text = if output.stderr.is_empty() {
                output.stdout
            } else {
                output.stderr
            };
            format!(
                "hyprctl: {}",
                String::from_utf8_lossy(&text)
                    .trim()
                    .chars()
                    .take(60)
                    .collect::<String>()
            )
        }
        Err(error) => format!("hyprctl: {error}"),
    }
}

/// The images written as 01.png, 02.jpg… in a new 0700 folder under the runtime directory; the
/// first one's path. Folders from earlier presses go after a day.
fn copy_privately(images: &[(&'static str, Vec<u8>)]) -> io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let root = paths::runtime_dir().join("cockpit-images");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&root)?;
    for entry in fs::read_dir(&root)?.flatten() {
        let is_old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| {
                modified
                    .elapsed()
                    .is_ok_and(|age| age > Duration::from_secs(86_400))
            });
        if is_old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    let folder = root.join(util::token_hex(8));
    fs::DirBuilder::new().mode(0o700).create(&folder)?;
    let mut paths = Vec::new();
    for (index, (content_type, bytes)) in images.iter().enumerate() {
        let extension = content_type
            .trim_start_matches("image/")
            .replace("jpeg", "jpg");
        let path = folder.join(format!("{:02}.{extension}", index + 1));
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?
            .write_all(bytes)?;
        paths.push(path);
    }
    Ok(paths.swap_remove(0))
}

/// hyprctl eval of OPEN_IN_SCRATCHPAD (scratchpads.lua): a cockpit web window, on a page.
pub fn scratchpad_call(name: &str, page: &str) -> Result<Vec<String>, String> {
    let address = regex::Regex::new(r"^https?://[\x21-\x7e]+$").unwrap();
    // The page reaches a shell through Hyprland's exec (scratchpads.lua): no quote, backslash, $
    // or backtick, which Lua's %q does not neutralise for sh.
    if !page.is_empty() && (!address.is_match(page) || page.contains(['"', '\\', '$', '`'])) {
        return Err(format!("not a page address: {page:?}"));
    }
    let arguments = if page.is_empty() {
        format!("\"{name}\"")
    } else {
        format!("\"{name}\", \"{page}\"")
    };
    Ok(vec![
        "hyprctl".into(),
        "eval".into(),
        format!("OPEN_IN_SCRATCHPAD({arguments})"),
    ])
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
            let text = if output.stderr.is_empty() {
                output.stdout
            } else {
                output.stderr
            };
            format!(
                "hyprctl: {}",
                String::from_utf8_lossy(&text)
                    .trim()
                    .chars()
                    .take(60)
                    .collect::<String>()
            )
        }
        Err(error) => format!("hyprctl: {error}"),
    }
}

/// fzf over lines; the chosen line, or None.
/// A session starting or restarting in the viewer pane: showing another card would replace the pane
/// and kill it. Only for as long as cockpit' loading screen waits (60 s), so a state file
/// left behind cannot stop the column from following.
fn is_viewer_loading() -> bool {
    fs::metadata(paths::loading_state())
        .and_then(|metadata| metadata.modified())
        .is_ok_and(|modified| {
            modified
                .elapsed()
                .is_ok_and(|age| age < Duration::from_secs(60))
        })
}

/// The Team view over the column: its rows, then the selected one in full (wrapped, never cut).
fn draw_team_view(view: &crate::teams::view::TeamView, buffer: &mut Buffer) {
    let (width, height) = (buffer.area.width, buffer.area.height);
    if width < 12 || height < 8 {
        return;
    }
    let (top, bottom) = (HEADER_LINES, height - 2);
    let inner = width.saturating_sub(4) as usize;
    let body = inner.saturating_sub(2).max(2);
    for row in top..=bottom {
        put(buffer, row, 1, &" ".repeat((width - 2) as usize), style(FG));
        put(buffer, row, 1, "│", style(BLUE));
        put(buffer, row, width - 2, "│", style(BLUE));
    }
    put(
        buffer,
        top,
        1,
        &format!("┌{}┐", "─".repeat(inner)),
        style(BLUE),
    );
    let title = if view.is_history {
        "Team · link and message history"
    } else {
        "Team · requests, members and links"
    };
    put(
        buffer,
        top,
        3,
        &fit(title, inner.saturating_sub(2)),
        bold(BLUE),
    );
    let rows = view.rows();
    let list_height = (rows.len() as u16).min((bottom - top) / 3).max(1);
    let mut line = top + 1;
    if view.snapshot_error.is_some() {
        put(
            buffer,
            line,
            3,
            &fit(
                "Collaboration unavailable · stale snapshot (r refresh)",
                body,
            ),
            style(YELLOW),
        );
        line += 1;
    }
    if rows.is_empty() {
        put(buffer, line, 3, &fit("nothing here", body), dim(FG_DARK));
    }
    let first = view.selected.saturating_sub(list_height as usize - 1);
    for (index, row) in rows
        .iter()
        .enumerate()
        .skip(first)
        .take(list_height as usize)
    {
        let selected = index == view.selected;
        put(
            buffer,
            line,
            3,
            &fit(&view.row_text(*row), body),
            if selected {
                on(FG, BG_HIGHLIGHT)
            } else {
                style(FG)
            },
        );
        line += 1;
    }
    line += 1;
    for text in view
        .detail()
        .iter()
        .flat_map(|text| wrap_detail_text(text, body))
        .skip(view.detail_scroll)
    {
        if line >= bottom - 1 {
            break;
        }
        put(buffer, line, 3, &text, style(FG));
        line += 1;
    }
    put(
        buffer,
        bottom - 1,
        3,
        &fit(
            "Tab history · j/k rows · ←/→ detail · n team · l link · r refresh · Esc close",
            body,
        ),
        dim(FG_DARK),
    );
    put(
        buffer,
        bottom,
        1,
        &format!("└{}┘", "─".repeat(inner)),
        style(BLUE),
    );
}

/// Every picker keeps its list's order unless `extra` turns sorting back on (a later --sort wins).
pub(crate) fn fzf_arguments<'a>(prompt: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    ["--prompt", prompt, "--height", "100%", "--no-sort"]
        .into_iter()
        .chain(extra.iter().copied())
        .collect()
}

fn fzf(prompt: &str, lines: &[String], extra: &[&str]) -> Option<String> {
    let mut child = Command::new("fzf")
        .args(fzf_arguments(prompt, extra))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(lines.join("\n").as_bytes());
    }
    let output = child.wait_with_output().ok()?;
    let chosen = String::from_utf8_lossy(&output.stdout)
        .trim_end_matches('\n')
        .to_string();
    (output.status.success() && !chosen.is_empty()).then_some(chosen)
}

fn session_key_requires_terminal(key: Key) -> bool {
    matches!(
        key,
        Key::Enter | Key::Char('r' | 'R' | 'e' | '$' | 'C' | 'x' | 'z' | 'm')
    )
}

/// The card keys that need a feature of the session's agent, and what they do, named for a
/// refusal (cockpit refuses them too, and Remote Control).
fn key_feature(key: Key, agent: &agents::Agent) -> Option<(Feature, &'static str)> {
    match key {
        Key::Char('z') => Some((Feature::Sleep, "sleeping")),
        Key::Char('e') if agent.can(Feature::Variants) => {
            Some((Feature::Variants, "the model variant picker"))
        }
        Key::Char('e') => Some((Feature::Effort, "the effort level")),
        Key::Char('$') => Some((Feature::Billing, "billing to API credits")),
        Key::Char('C') => Some((Feature::Compact, "/compact")),
        Key::Char('m') => Some((Feature::RemoteControl, "Remote Control")),
        _ => None,
    }
}

/// The cockpit command in `arguments`, past a leading `--agent <name>`.
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
    let mut choices: Vec<String> = todo::Category::ALL
        .iter()
        .map(|category| category.label().to_string())
        .collect();
    if include_all {
        choices.insert(0, "All".into());
    }
    if let Some(category) = current {
        let index = choices
            .iter()
            .position(|label| label == category.label())
            .unwrap();
        let selected = choices.remove(index);
        choices.insert(0, selected);
    }
    fzf(
        if include_all {
            "filter kind> "
        } else {
            "kind> "
        },
        &choices,
        &[],
    )
}

/// The agent a new session runs (agents::AGENTS); None when the pick is cancelled, and then
/// nothing starts.
fn pick_agent() -> Option<String> {
    let names: Vec<String> = agents::names().into_iter().map(String::from).collect();
    fzf("agent> ", &names, &[]).filter(|agent| agents::find(agent).is_some())
}

/// The index of the chosen past session.
fn pick_past_session(past: &[(PastSession, String)]) -> Option<String> {
    let lines: Vec<String> = past
        .iter()
        .enumerate()
        .map(|(index, (entry, title))| {
            let when = chrono::DateTime::from_timestamp(entry.since, 0)
                .map(|t| {
                    t.with_timezone(&chrono::Local)
                        .format("%d %b %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            let badge = agents::of_session_id(&entry.session)
                .and_then(|agent| agent.badge)
                .map(|badge| format!("{badge}  "))
                .unwrap_or_default();
            format!(
                "{index}\t{when}  {:<18} {badge}{}",
                entry.project,
                if title.is_empty() {
                    &entry.prompt
                } else {
                    title
                }
            )
        })
        .collect();
    let chosen = fzf(
        "resume> ",
        &lines,
        &["--delimiter", "\t", "--with-nth", "2.."],
    )?;
    chosen.split('\t').next().map(str::to_string)
}

/// Who handed an item over and what it waits on, for Theo to follow.
fn handoff_details(item: &todo::Item) -> String {
    let mut text = String::new();
    if !item.queued_by().is_empty() {
        text.push_str(&format!("Queued by {}\n", item.queued_by()));
    }
    let after = item.after();
    if !after.is_empty() {
        text.push_str(&format!(
            "\nWaits on (joins the queue once they are done; h hands it anyway)\n{}\n",
            after.join(", ")
        ));
    }
    for input in item.inputs() {
        let field = |key: &str| {
            input
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        text.push_str(&format!(
            "\nDone before it: {} in {} ({})\n{}\n",
            field("id"),
            field("project"),
            field("text"),
            if field("result").is_empty() {
                "(no summary)".to_string()
            } else {
                field("result")
            }
        ));
    }
    if !item.result().is_empty() {
        text.push_str(&format!("\nResult, from its session\n{}\n", item.result()));
    }
    text
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
        assert_eq!(fit("x", 1), "x");
        assert_eq!(fit("xy", 1), "");
    }

    #[test]
    fn a_page_address_is_checked_before_it_reaches_lua() {
        assert!(scratchpad_call("prs", "https://github.com/o/r/pull/7").is_ok());
        assert!(scratchpad_call("prs", "https://x\"); os.execute(\"rm").is_err());
    }
}
