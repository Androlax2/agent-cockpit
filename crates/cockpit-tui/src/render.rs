//! Draws the app: the session list grouped by project, the approval pane, the form, the
//! help and the log, and a status line. Plain on purpose (decision 17): the designed UI is
//! M6's. Everything shown comes from the board and the wire fields, never from a screen.

use std::time::Instant;

use cockpit_protocol::{GuideView, HiddenView, RunRequestView, SessionView, ShownText};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{App, Confirm, Focus, Overlay};
use crate::board::Board;
use crate::form::{Field, Form, Place};
use crate::pane::{Pane, Shown, Typing};

/// The help overlay's text: every key the basic TUI has.
pub const HELP: &str = "\
Sessions
  j / k, Down / Up   select a session
  Enter              open it in the viewer (wakes it when asleep)
  n                  open the next session that needs you: marked ! when it is
                     blocked on you, ? when it asked you and still works
  N                  new session (project, agent, first message, in place or new clone)
  r                  restart the selected session (confirm with y)
  x                  exit the selected session (confirm with y)
  X                  exit every finished session (confirm with y)
Workflows
  F                  the running workflows; j / k select, Enter opens one
  a                  complete its step, waiving the step's requirements (type the handoff)
  p                  release the session doing the step (confirm with y)
  b                  unblock it
  x                  cancel it (confirm with y);  Esc goes back
Approval pane (Tab moves between the list and the pane)
  y                  approve the command shown / done with the guide step
  x                  decline the command / close the guide
  m                  show the whole command, or fold it again
  i                  type a line for a command waiting for input; Enter sends it
  s                  stuck on the guide step: type why, Enter sends it
  Keys are ignored for a moment after a request appears or the pane takes focus.
Elsewhere
  L                  log of what the TUI did
  ?                  this help;  Esc closes it
  Detach from the manager with tmux's prefix, then d.";

pub fn draw(app: &App, frame: &mut Frame<'_>, now: Instant) {
    let area = frame.area();
    if let Some(overlay) = app.overlay() {
        let (title, text) = match overlay {
            Overlay::Help => ("Help", HELP.to_owned()),
            Overlay::Log => ("Log", app.log().collect::<Vec<_>>().join("\n")),
        };
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(title)),
            area,
        );
        return;
    }
    let pane_height = if app.pane().shown().is_some() { 14 } else { 0 };
    let [main, pane, status] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(pane_height),
        Constraint::Length(1),
    ])
    .areas(area);
    match (app.form(), app.workflows()) {
        (Some(form), _) => draw_form(form, frame, main),
        (None, Some(workflows)) => workflows.draw(frame, main),
        (None, None) => draw_list(app, frame, main),
    }
    if pane_height > 0 {
        draw_pane(app, frame, pane, now);
    }
    frame.render_widget(Paragraph::new(status_line(app)), status);
}

fn status_line(app: &App) -> String {
    match (&app.confirm, &app.failure, &app.feed_problem) {
        (Some(Confirm::Restart(session)), ..) => format!("restart {session}? y to confirm"),
        (Some(Confirm::Exit(session)), ..) => format!("exit {session}? y to confirm"),
        (Some(Confirm::ExitFinished(sessions)), ..) => {
            format!("exit {} finished sessions? y to confirm", sessions.len())
        }
        (None, Some(failure), Some(problem)) => format!(
            "{}: {} · change feed: {problem}",
            failure.kind, failure.problem
        ),
        (None, Some(failure), None) => format!("{}: {}", failure.kind, failure.problem),
        (None, None, Some(problem)) => format!("change feed: {problem}"),
        (None, None, None) => "? help".to_owned(),
    }
}

/// The indent of a card's details line, and of every row a card's line wraps onto.
const CARD_INDENT: &str = "   ";

/// The list, wrapped here rather than by the paragraph so the rows the selected card takes
/// are known: the list scrolls to keep that card whole on screen (#291).
fn draw_list(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let board = app.board();
    let width = usize::from(area.width.saturating_sub(2));
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line<'_>> = Vec::new();
    let mut selected_rows = None;
    for group in board.groups() {
        let project = group.project.unwrap_or("(no known project)");
        push_wrapped(&mut lines, project, width, "", bold);
        if group.sessions.is_empty() {
            push_wrapped(&mut lines, "    no sessions", width, "", Style::default());
        }
        for session in group.sessions {
            let is_selected = app.selected() == Some(session.id.as_str());
            let style = if is_selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let first_row = lines.len();
            for text in card(board, session, is_selected) {
                push_wrapped(&mut lines, &text, width, CARD_INDENT, style);
            }
            if is_selected {
                selected_rows = Some(first_row..lines.len());
            }
        }
    }
    let finished = board.finished_sessions().len();
    if finished > 0 {
        let text = format!("Finished ({finished}), folded: X exits them");
        push_wrapped(&mut lines, &text, width, "", bold);
    }
    if lines.is_empty() {
        let text = "No project yet: cockpit project add DIR";
        push_wrapped(&mut lines, text, width, "", Style::default());
    }
    let title = if app.focus() == Focus::List {
        "Sessions *"
    } else {
        "Sessions"
    };
    let height = usize::from(area.height.saturating_sub(2));
    let offset = selected_rows.map_or(0, |rows| scroll_to_show(&rows, height));
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0))
            .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

/// The fewest rows to scroll so `rows` end on screen; a card taller than the list shows its
/// first rows.
fn scroll_to_show(rows: &std::ops::Range<usize>, height: usize) -> usize {
    rows.end.saturating_sub(height).min(rows.start)
}

fn push_wrapped(lines: &mut Vec<Line<'_>>, text: &str, width: usize, indent: &str, style: Style) {
    lines.extend(
        wrapped(text, width, indent)
            .into_iter()
            .map(|row| Line::from(row).style(style)),
    );
}

/// `text` in rows of at most `width` columns, broken at spaces (inside a word only when the
/// word alone is wider). Rows after the first start with `indent`, so a wrapped line stays
/// under its card's indent instead of returning to column 0 (#291). Runs of spaces inside a
/// row are kept; those a break falls on are dropped.
fn wrapped(text: &str, width: usize, indent: &str) -> Vec<String> {
    let width = width.max(columns(indent) + 1);
    let body = text.trim_start_matches(' ');
    let mut rows = Vec::new();
    let mut row = text[..text.len() - body.len()].to_owned();
    let mut row_has_word = false;
    let mut gap = String::new();
    for (at, word) in body.split(' ').enumerate() {
        if at > 0 {
            gap.push(' ');
        }
        if word.is_empty() {
            continue;
        }
        if row_has_word {
            if columns(&row) + columns(&gap) + columns(word) <= width {
                row.push_str(&gap);
                row.push_str(word);
                gap.clear();
                continue;
            }
            rows.push(std::mem::replace(&mut row, indent.to_owned()));
        }
        gap.clear();
        // The word starts this row; one wider than a row is cut where the row ends.
        let mut row_has_char = false;
        for character in word.chars() {
            let character = character.to_string();
            if row_has_char && columns(&row) + columns(&character) > width {
                rows.push(std::mem::replace(&mut row, indent.to_owned()));
            }
            row.push_str(&character);
            row_has_char = true;
        }
        row_has_word = true;
    }
    rows.push(row);
    rows
}

fn columns(text: &str) -> usize {
    ratatui::text::Span::raw(text).width()
}

/// One session's card: the id line, then whether it waits on you and what the agent reports
/// on an indented line of their own. The mark after the selection marker lets the eye find the
/// sessions that need you down the list's left edge: `!` blocked on you, `?` asked you and
/// still works (#215).
fn card(board: &Board, session: &SessionView, is_selected: bool) -> Vec<String> {
    let marker = if is_selected { ">" } else { " " };
    let (mark, waiting) = if !board.needs_you(&session.id) {
        (" ", None)
    } else if board.asked_while_working(&session.id) {
        ("?", Some("asked you, still working"))
    } else {
        ("!", Some("waiting on you"))
    };
    let mut card = vec![format!(
        "{marker}{mark} {}  {}  {}",
        session.id, session.agent, session.state,
    )];
    let details: Vec<&str> = waiting
        .into_iter()
        .chain(
            board
                .report(&session.id)
                .into_iter()
                .flat_map(|report| [report.label.as_deref(), report.doing.as_deref()])
                .flatten(),
        )
        .collect();
    if !details.is_empty() {
        card.push(format!("{CARD_INDENT}{}", details.join(" · ")));
    }
    card
}

fn draw_form(form: &Form, frame: &mut Frame<'_>, area: Rect) {
    let field = |name: Field, label: &str, value: String| {
        let marker = if form.field() == name { ">" } else { " " };
        Line::from(format!("{marker} {label}: {value}"))
    };
    let agent = match &form.agents {
        None => "loading…".to_owned(),
        Some(agents) if agents.is_empty() => "none available".to_owned(),
        Some(agents) => agents[form.agent].name.clone(),
    };
    let place = match form.place {
        Place::InPlace => "in place".to_owned(),
        Place::NewClone => "new clone".to_owned(),
    };
    let mut lines = vec![
        field(Field::Project, "Project", form.project.clone()),
        field(Field::Agent, "Agent", agent),
        field(Field::Message, "First message", form.message.clone()),
        field(Field::Place, "Where", place),
    ];
    if form.place == Place::NewClone {
        lines.push(field(Field::Branch, "Branch", form.branch.clone()));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Tab next field  Left/Right choose  Enter start  Esc cancel",
    ));
    if let Some(problem) = &form.problem {
        lines.push(Line::from(format!("Cannot start: {problem}")));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("New session")),
        area,
    );
}

fn draw_pane(app: &App, frame: &mut Frame<'_>, area: Rect, now: Instant) {
    let pane = app.pane();
    let board = app.board();
    let mut lines = match pane.shown() {
        Some(Shown::Run { id, .. }) => board
            .runs
            .iter()
            .find(|run| run.id == *id)
            .map(|run| run_lines(run, pane))
            .unwrap_or_default(),
        Some(Shown::Guide { id, .. }) => board
            .guides
            .iter()
            .find(|guide| guide.id == *id)
            .map(guide_lines)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    if let Some((typing, typed)) = pane.typing() {
        let prompt = match typing {
            Typing::Input => "Line for the command",
            Typing::Answer => "Answer",
            Typing::StuckNote => "Stuck because",
        };
        lines.push(format!("{prompt}: {typed}_   (Enter sends, Esc cancels)"));
    } else if pane.in_grace(now) {
        lines.push("Read it first: keys work again in a moment.".to_owned());
    } else {
        lines.push(pane_keys(pane.shown()).to_owned());
    }
    let title = if app.focus() == Focus::Pane {
        "Waiting on you *"
    } else {
        "Waiting on you (Tab)"
    };
    frame.render_widget(
        Paragraph::new(lines.join("\n"))
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn pane_keys(shown: Option<&Shown>) -> &'static str {
    match shown {
        Some(Shown::Run {
            waiting_for_input: true,
            ..
        }) => "i type a line   m more",
        Some(Shown::Run { .. }) => "y approve   x decline   m more",
        Some(Shown::Guide { asks: true, .. }) => "y answer   s stuck   x close",
        Some(Shown::Guide { .. }) => "y done   s stuck   x close",
        None => "",
    }
}

fn run_lines(run: &RunRequestView, pane: &Pane) -> Vec<String> {
    let waiting = crate::pane::waits_for_input(run);
    let mut lines = vec![format!(
        "{} {} from {}{}",
        if waiting {
            "Command waiting for input:"
        } else {
            "Run request"
        },
        run.id,
        run.session,
        run.project
            .as_ref()
            .map(|project| format!(" (project {project})"))
            .unwrap_or_default(),
    )];
    let outside = if run.outside_project {
        "  [outside the project]"
    } else {
        ""
    };
    lines.push(format!("Directory: {}{outside}", run.shown_cwd.text));
    lines.push(format!("Reason: {}", run.shown_reason.text));
    match (&run.folded_command, pane.is_expanded()) {
        (Some(folded), false) => {
            lines.push(format!("Command: {}", folded.preview));
            lines.push(format!(
                "  … more: {} lines, {} characters (m shows them)",
                folded.more_lines, folded.more_chars
            ));
        }
        _ => lines.push(format!("Command: {}", run.shown_command.text)),
    }
    if let Some(asked) = &run.asked_command {
        lines.push(format!("Edited from what the agent asked: {asked}"));
    }
    let warnings = warnings(&[&run.shown_command, &run.shown_cwd, &run.shown_reason]);
    if !warnings.is_empty() {
        lines.push(format!("Warning: {warnings}"));
    }
    if run.timeout_without_foreground {
        lines.push(
            "Warning: timeout without --foreground: a prompt in the command would stop it"
                .to_owned(),
        );
    }
    if let Some(files) = run
        .changed_build_files
        .as_ref()
        .filter(|files| !files.is_empty())
    {
        lines.push(format!(
            "Warning: it runs build files changed outside the default branch: {}",
            files.join(", ")
        ));
    }
    lines
}

/// Every warning the display rules raised, each kind once, and the non-ASCII characters.
fn warnings(texts: &[&ShownText]) -> String {
    let mut kinds: Vec<&str> = Vec::new();
    let mut non_ascii: Vec<&str> = Vec::new();
    for text in texts {
        for kind in &text.warnings {
            let name = hidden_name(*kind);
            if !kinds.contains(&name) {
                kinds.push(name);
            }
        }
        if text.has_hidden_characters
            && text.warnings.is_empty()
            && !kinds.contains(&"hidden characters")
        {
            kinds.push("hidden characters");
        }
        non_ascii.extend(text.non_ascii.iter().map(String::as_str));
    }
    let mut parts: Vec<String> = kinds.into_iter().map(str::to_owned).collect();
    if !non_ascii.is_empty() {
        parts.push(format!("non-ASCII {}", non_ascii.join(", ")));
    }
    parts.join("; ")
}

fn hidden_name(kind: HiddenView) -> &'static str {
    match kind {
        HiddenView::BidiControl => "direction controls escaped",
        HiddenView::Invisible => "invisible characters escaped",
        HiddenView::Control => "control codes escaped",
        HiddenView::BlankLines => "blank lines collapsed",
        HiddenView::Unknown => "characters escaped",
    }
}

fn guide_lines(guide: &GuideView) -> Vec<String> {
    let mut lines = vec![format!(
        "Guide {} from {}: {}",
        guide.id, guide.session, guide.title.text
    )];
    if let Some(step) = guide.steps.get(guide.current_step) {
        lines.push(format!(
            "Step {} of {}: {}",
            guide.current_step + 1,
            guide.steps.len(),
            step.text.text
        ));
        lines.extend(step.open.iter().map(|url| format!("  open: {}", url.text)));
        lines.extend(
            step.copy
                .iter()
                .map(|value| format!("  copy: {}", value.text)),
        );
        if let Some(ask) = &step.ask {
            lines.push(format!("  ask: {}", ask.text));
        }
        if let Some(run) = &step.run {
            lines.push(format!("  run: {}", run.text));
        }
        let mut texts: Vec<&ShownText> = vec![&guide.title, &step.text];
        texts.extend(step.open.iter().chain(&step.copy));
        let warnings = warnings(&texts);
        if !warnings.is_empty() {
            lines.push(format!("Warning: {warnings}"));
        }
    }
    lines
}
