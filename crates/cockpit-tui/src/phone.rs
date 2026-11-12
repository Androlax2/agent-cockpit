//! The phone layout (#855). Below the owner's `tui.phone_below_columns` the daemon shows the
//! column alone (`manager-fit`), and the TUI draws one column: what waits on the owner first,
//! each request a full-width card with its command whole, then one line per session. A
//! session opened fills the screen below a bar of the column, read-only: the viewer drops
//! what is typed into it, and the column keeps the keys. `i` types a line into it, sent with
//! `session-type`; Esc, leaving the session or a minute without a key reads only again.
//!
//! Decisions go through the same pane, keys and commands as on the desktop; the phone only
//! adds that `y` and `x` wait until the card's end is on screen.

use std::time::{Duration, Instant};

use cockpit_protocol::Command;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::Key;
use crate::app::{App, Focus};
use crate::board::Board;
use crate::pane::{Shown, Typing};
use crate::render::{
    bug_reports_card, columns, guide_lines, network_lines, pane_keys, reveal_hint, run_lines,
    scroll_to_show, status_line, truncated, typed_input_lines, wrapped,
};

/// Typing stops after this long without a key: a phone left in a pocket reads only again.
pub const PHONE_TYPING_TIMEOUT: Duration = Duration::from_secs(60);

/// The indent of every row a card's line wraps onto.
const WRAP_INDENT: &str = "  ";

/// The session open full screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpenSession {
    pub(crate) session: String,
    typing: Option<TypedLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypedLine {
    text: String,
    last_key: Instant,
}

/// What a key did in the open session.
pub(crate) enum OpenOutcome {
    Back,
    Help,
    Handled {
        used: &'static str,
        send: Option<Box<Command>>,
    },
}

impl OpenSession {
    pub(crate) fn read_only(session: String) -> OpenSession {
        OpenSession {
            session,
            typing: None,
        }
    }

    /// The line being typed; `None` while read-only, which typing is again once it timed out.
    pub(crate) fn typed(&self, now: Instant) -> Option<&str> {
        self.typing
            .as_ref()
            .filter(|typing| now < typing.last_key + PHONE_TYPING_TIMEOUT)
            .map(|typing| typing.text.as_str())
    }

    pub(crate) fn key(&mut self, key: Key, now: Instant) -> OpenOutcome {
        if self.typed(now).is_none() {
            self.typing = None;
        }
        let Some(typing) = &mut self.typing else {
            return match key {
                Key::Char('i') => {
                    self.typing = Some(TypedLine {
                        text: String::new(),
                        last_key: now,
                    });
                    handled("phone-type", None)
                }
                Key::Esc | Key::Char('q') => OpenOutcome::Back,
                Key::Char('?') => OpenOutcome::Help,
                _ => handled("phone-read-only", None),
            };
        };
        typing.last_key = now;
        match key {
            Key::Esc => {
                self.typing = None;
                handled("phone-read-only-again", None)
            }
            Key::Char(character) => {
                typing.text.push(character);
                handled("phone-typing", None)
            }
            Key::Backspace => {
                typing.text.pop();
                handled("phone-typing", None)
            }
            Key::Enter => {
                let text = std::mem::take(&mut typing.text);
                let session = self.session.clone();
                handled(
                    "phone-type-line",
                    Some(Command::SessionType { session, text }),
                )
            }
            _ => handled("phone-typing", None),
        }
    }
}

fn handled(used: &'static str, send: Option<Command>) -> OpenOutcome {
    OpenOutcome::Handled {
        used,
        send: send.map(Box::new),
    }
}

pub(crate) fn draw(app: &App, frame: &mut Frame<'_>, now: Instant) {
    match &app.phone_open {
        Some(open) => draw_bar(app, open, frame, frame.area(), now),
        None => draw_list(app, frame, frame.area(), now),
    }
}

/// The column above the open session: which session and whether typing reaches it, the keys
/// or the typed line, then what waits on the owner or the last failure.
fn draw_bar(app: &App, open: &OpenSession, frame: &mut Frame<'_>, area: Rect, now: Instant) {
    let width = usize::from(area.width);
    let project = app
        .board
        .sessions
        .get(&open.session)
        .and_then(|session| app.board.project_of(session))
        .unwrap_or("-");
    let typed = open.typed(now);
    let is_running = app.is_phone_session_running();
    let mode = match (is_running, typed) {
        (false, _) => "no terminal",
        (true, Some(_)) => "TYPING",
        (true, None) => "read-only",
    };
    let second = match typed {
        _ if !is_running => "Any key goes back".to_owned(),
        None => "i type   Esc back   ? help".to_owned(),
        Some(typed) => {
            // The end of the line stays in sight as it grows past the width.
            let room = width.saturating_sub(3);
            let skip = typed.chars().count().saturating_sub(room);
            format!("> {}_", typed.chars().skip(skip).collect::<String>())
        }
    };
    let waiting = waiting_count(&app.board);
    let third = match (&app.failure, typed) {
        (Some(failure), _) => format!("{}: {}", failure.kind, failure.problem),
        (None, Some(_)) => "Enter sends   Esc read-only".to_owned(),
        (None, None) if waiting > 0 => format!("! {waiting} waiting on you: Esc shows them"),
        (None, None) => String::new(),
    };
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let lines = vec![
        Line::from(truncated(
            &format!("{} · {project} · {mode}", open.session),
            width,
        ))
        .style(bold),
        Line::from(truncated(&second, width)),
        Line::from(truncated(&third, width)),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

/// The cards, the sessions, the status line when there is something to say, the keys.
fn draw_list(app: &App, frame: &mut Frame<'_>, area: Rect, now: Instant) {
    let width = usize::from(area.width);
    let document = document(app, width);
    let status = phone_status(app);
    let [body, status_area, hint] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(u16::from(status.is_some())),
        Constraint::Length(1),
    ])
    .areas(area);
    let height = usize::from(body.height);
    let offset = if is_reading_card(app) {
        app.phone_scroll
            .min(document.head_rows.saturating_sub(height))
    } else {
        document
            .selected_rows
            .as_ref()
            .map_or(0, |rows| scroll_to_show(rows, height))
    };
    frame.render_widget(
        Paragraph::new(document.lines).scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0)),
        body,
    );
    if let Some(status) = status {
        frame.render_widget(Paragraph::new(truncated(&status, width)), status_area);
    }
    let hint_text = fitting(&hints(app, width, usize::from(area.height), now), width);
    frame.render_widget(
        Paragraph::new(Line::from(hint_text).style(Style::default().add_modifier(Modifier::DIM))),
        hint,
    );
}

/// What the status line says, when it says more than where help is.
fn phone_status(app: &App) -> Option<String> {
    (app.confirm.is_some() || app.failure.is_some() || app.feed_problem.is_some())
        .then(|| status_line(app))
}

/// Whether the owner is reading the card at the top: the pane has focus and shows one.
fn is_reading_card(app: &App) -> bool {
    app.focus == Focus::Pane && app.pane.shown().is_some()
}

/// The rows the list's body has on a screen `height` rows high.
pub(crate) fn body_height(app: &App, height: usize) -> usize {
    height.saturating_sub(1 + usize::from(phone_status(app).is_some()))
}

/// How many rows the card the keys decide takes at `width`.
pub(crate) fn head_card_rows(app: &App, width: usize) -> usize {
    document(app, width).head_rows
}

/// The key hints, longest first: the first that fits the width is shown.
fn hints(app: &App, width: usize, height: usize, now: Instant) -> Vec<String> {
    if is_reading_card(app) {
        if let Some((typing, _)) = app.pane.typing() {
            let short = "Enter sends   Esc cancels".to_owned();
            if typing != Typing::Input {
                return vec![short];
            }
            let reveal = reveal_hint(app.pane.is_input_revealed());
            return vec![format!("Enter sends   {reveal}   Esc cancels"), short];
        }
        if app.pane.in_grace(now) {
            return vec![
                "Read it first: keys work again in a moment".to_owned(),
                "Read it first".to_owned(),
            ];
        }
        let is_seen = app.phone_scroll + body_height(app, height) >= head_card_rows(app, width);
        if !is_seen {
            return vec![
                "j scrolls: the keys wait for the end".to_owned(),
                "j scrolls to the end".to_owned(),
            ];
        }
        let keys = pane_keys(&app.pane);
        return vec![format!("{keys}   Tab sessions"), keys.to_owned()];
    }
    let card = if app.pane.shown().is_some() {
        "   Tab card"
    } else {
        ""
    };
    vec![
        format!("Enter open   n next{card}   ? help"),
        format!("Enter open{card}   ? help"),
        "? help".to_owned(),
    ]
}

struct Document {
    lines: Vec<Line<'static>>,
    /// The rows of the card the keys decide, from the top: 0 when none is shown.
    head_rows: usize,
    selected_rows: Option<std::ops::Range<usize>>,
}

/// The whole column: the request the pane shows, whole; the others waiting; the agents'
/// questions and dialogs; one line per session.
fn document(app: &App, width: usize) -> Document {
    let board = &app.board;
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let head = app.pane.shown();
    if let Some(shown) = head {
        let mut card = card_lines(board, shown);
        card.extend(typed_input_lines(&app.pane));
        push_card(&mut lines, &card, width, bold);
    }
    let head_rows = lines.len();
    for shown in crate::pane::waiting(board).filter(|waiting| Some(waiting) != head) {
        push_blank(&mut lines);
        push_card(
            &mut lines,
            &card_lines(board, &shown),
            width,
            Style::default(),
        );
    }
    let listed = board.listed_sessions();
    for session in &listed {
        let Some(question) = question_card(board, &session.id) else {
            continue;
        };
        push_blank(&mut lines);
        push_card(&mut lines, &[question], width, Style::default());
    }
    push_blank(&mut lines);
    if let Some(text) = bug_reports_card(board) {
        push_card(&mut lines, &[text], width, bold);
    }
    let mut selected_rows = None;
    for group in board.groups() {
        let project = group.project.unwrap_or("(no project)");
        for session in group.sessions {
            let is_selected = app.selected.as_deref() == Some(session.id.as_str());
            let style = if is_selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            if is_selected {
                selected_rows = Some(lines.len()..lines.len() + 1);
            }
            let text = session_line(board, session, project, is_selected);
            lines.push(Line::from(truncated(&text, width)).style(style));
        }
    }
    let finished = board.finished_sessions().len();
    if finished > 0 {
        let text = format!("Finished ({finished}), folded: X exits them");
        lines.push(Line::from(truncated(&text, width)));
    }
    if listed.is_empty() && finished == 0 {
        push_card(
            &mut lines,
            &["No session yet: N starts one".to_owned()],
            width,
            Style::default(),
        );
    }
    Document {
        lines,
        head_rows,
        selected_rows,
    }
}

/// A waiting request's lines, its command whole: never folded behind "more" on the phone.
fn card_lines(board: &Board, shown: &Shown) -> Vec<String> {
    match shown {
        Shown::Run { id, .. } => board
            .runs
            .iter()
            .find(|run| run.id == *id)
            .map(|run| run_lines(run, true))
            .unwrap_or_default(),
        Shown::Guide { id, .. } => board
            .guides
            .iter()
            .find(|guide| guide.id == *id)
            .map(guide_lines)
            .unwrap_or_default(),
        Shown::Network { id } => board
            .network
            .iter()
            .find(|request| request.id == *id)
            .map(network_lines)
            .unwrap_or_default(),
    }
}

/// An agent's own question or a provider dialog, for a session no request card covers.
fn question_card(board: &Board, session: &str) -> Option<String> {
    let report = board.report(session)?;
    let question = report.needs_input.as_deref()?;
    // A request has its own card; a question a coordinator holds is not the owner's (#710).
    if board.has_open_request(session) || report.held_by.is_some() {
        return None;
    }
    let what = match report.question_source {
        Some(cockpit_protocol::QuestionSource::Dialog) => "has a dialog open",
        _ => "asks",
    };
    Some(format!(
        "? {session} {what}: {question} (Enter on its line opens it)"
    ))
}

/// `>! project  state  doing-now`: the mark is `!` when it waits on the owner, `?` when its
/// agent asked and still works. Cut at the width: the cards carry what must be read whole.
fn session_line(
    board: &Board,
    session: &cockpit_protocol::SessionView,
    project: &str,
    is_selected: bool,
) -> String {
    let marker = if is_selected { ">" } else { " " };
    let mark = if !board.needs_you(&session.id) {
        " "
    } else if board.asked_while_working(&session.id) {
        "?"
    } else {
        "!"
    };
    let doing = board
        .report(&session.id)
        .and_then(|report| report.doing.as_deref().or(report.label.as_deref()))
        .unwrap_or("");
    format!("{marker}{mark} {project}  {}  {doing}", session.state)
        .trim_end()
        .to_owned()
}

/// The requests waiting on the owner, as the pane would show them in turn.
fn waiting_count(board: &Board) -> usize {
    crate::pane::waiting(board).count()
}

/// Each line of `card` wrapped at `width` (a command's own line breaks kept), never cut.
fn push_card(lines: &mut Vec<Line<'static>>, card: &[String], width: usize, style: Style) {
    for line in card {
        for part in line.split('\n') {
            lines.extend(
                wrapped(part, width, WRAP_INDENT)
                    .into_iter()
                    .map(|row| Line::from(row).style(style)),
            );
        }
    }
}

/// A blank row between two blocks: never above the first.
fn push_blank(lines: &mut Vec<Line<'static>>) {
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
}

/// The first of `variants` that fits `width`, or the last cut to it.
fn fitting(variants: &[String], width: usize) -> String {
    variants
        .iter()
        .find(|variant| columns(variant) <= width)
        .cloned()
        .unwrap_or_else(|| truncated(variants.last().map_or("", String::as_str), width))
}
