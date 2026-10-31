//! Draws the app: the session list grouped by project, the approval pane, the form, the
//! help and the log, and a status line. Plain on purpose (decision 17): the designed UI is
//! M6's. Everything shown comes from the board and the wire fields, never from a screen.

use std::time::Instant;

use cockpit_protocol::{
    GuideView, HiddenView, RunRequestView, RunStateView, SessionStateView, SessionView, ShownText,
};
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
  n                  open the next session that needs you
  N                  new session (project, agent, first message, in place or new clone)
  r                  restart the selected session (confirm with y)
  x                  exit the selected session (confirm with y)
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
    match app.form() {
        Some(form) => draw_form(form, frame, main),
        None => draw_list(app, frame, main),
    }
    if pane_height > 0 {
        draw_pane(app, frame, pane, now);
    }
    frame.render_widget(Paragraph::new(status_line(app)), status);
}

fn status_line(app: &App) -> String {
    match (&app.confirm, &app.status) {
        (Some(Confirm::Restart(session)), _) => format!("restart {session}? y to confirm"),
        (Some(Confirm::Exit(session)), _) => format!("exit {session}? y to confirm"),
        (None, Some(status)) => status.clone(),
        (None, None) => "? help".to_owned(),
    }
}

fn draw_list(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let board = app.board();
    let mut lines = Vec::new();
    for group in board.groups() {
        lines.push(
            Line::from(group.project.unwrap_or("(no known project)").to_owned())
                .style(Style::default().add_modifier(Modifier::BOLD)),
        );
        if group.sessions.is_empty() {
            lines.push(Line::from("    no sessions"));
        }
        for session in group.sessions {
            let is_selected = app.selected() == Some(session.id.as_str());
            let line = Line::from(card(board, session, is_selected));
            lines.push(if is_selected {
                line.style(Style::default().add_modifier(Modifier::REVERSED))
            } else {
                line
            });
        }
    }
    if lines.is_empty() {
        lines.push(Line::from("No project yet: cockpit project add DIR"));
    }
    let title = if app.focus() == Focus::List {
        "Sessions *"
    } else {
        "Sessions"
    };
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

/// One session's line: id, agent, state, what it reports, and whether it waits on you.
fn card(board: &Board, session: &SessionView, is_selected: bool) -> String {
    let marker = if is_selected { ">" } else { " " };
    let mut card = format!(
        "{marker} {}  {}  {}",
        session.id,
        session.agent,
        state_word(&session.state)
    );
    if let Some(label) = board
        .report(&session.id)
        .and_then(|report| report.label.as_deref().or(report.doing.as_deref()))
    {
        card.push_str("  ");
        card.push_str(label);
    }
    if board.needs_you(&session.id) {
        card.push_str("  waiting on you");
    }
    card
}

fn state_word(state: &SessionStateView) -> String {
    match state {
        SessionStateView::Launching => "launching".to_owned(),
        SessionStateView::Running => "running".to_owned(),
        SessionStateView::Stopping { to } => format!("stopping:{}", to.as_str()),
        SessionStateView::Asleep => "asleep".to_owned(),
        SessionStateView::Ended => "ended".to_owned(),
        SessionStateView::Failed => "failed".to_owned(),
        SessionStateView::Unknown => "unknown".to_owned(),
    }
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
    let waiting = run.state == RunStateView::Running && run.waiting_for_input;
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
