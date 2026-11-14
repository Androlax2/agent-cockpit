//! Draws the app: the session list grouped by project, the approval pane, the form, the
//! help and the log, a row per provider account's plan windows, and a status line. Plain on purpose (decision 17): the designed UI is
//! M6's. Everything shown comes from the board and the wire fields, never from a screen.

use std::time::Instant;

use cockpit_protocol::{
    CompactionStatus, FindingView, GuideView, HiddenView, NO_KNOWN_RISKY_PATTERN,
    NetworkRequestView, NetworkSubjectView, NetworkTargetView, RootFileView, RootRunsView,
    RunRequestView, SessionReport, SessionStateView, SessionView, ShownText,
    changed_since_asked_warning, finding_line, sudo_ran_unasked_warning,
    typed_secret_redacted_notice,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{App, Confirm, Focus, Overlay};
use crate::board::{Board, Idle};
use crate::bug_form::{BugField, BugForm};
use crate::form::{Field, Form, Place};
use crate::pane::{Pane, Shown, Typing};

/// The help overlay's text: every key the basic TUI has.
pub const HELP: &str = "\
Documentation
  https://androlax2.github.io/agent-cockpit/latest/

Sessions
  j / k, Down / Up   select a session
  Enter              open it in the viewer (wakes it when asleep)
  n                  open the next session that needs you: marked ! when it is
                     blocked on you, ? when it asked you and still works
  N                  new session (project, agent, first message, in place or new clone)
  r                  restart the selected session (confirm with y)
  x                  exit the selected session (confirm with y)
  X                  exit every finished session (confirm with y)
Projects
  P                  the registered projects; d removes the selected one (confirm
                     with y): nothing on disk is deleted, its history stays
Tasks
  T                  tasks; a add, e edit, h hand to session, Q queue, ? explanation
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
  x                  on a command waiting for input: stop it and every process it
                     started (confirm with y)
  Tab                while typing that line: show it, or mask it again (masked first)
  s                  stuck on the guide step: type why, Enter sends it
  Keys are ignored for a moment after a request appears or the pane takes focus.
Phone layout (narrower than tui.phone_below_columns, 60 columns by default)
  Requests and questions come first, each whole; sessions one line each below
  j / k              scroll a long request; y and x wait until its end shows
  Tab                between the request and the sessions
  Enter              open the selected session full screen, read-only
  i                  type into it: Enter sends the line; Esc, leaving it or
                     a minute without a key makes it read-only again
  Esc                back to the list
Bug reports
  B                  the bug report drafts waiting for you; review and send them
                     with torlek bug-report show ID and send ID
  j k, Enter         in B: select a draft, then record the issue you submitted for
                     it in your browser (its URL or its number)
  b                  report a Torlek bug (description, optional title and steps)
Elsewhere
  L                  log of what the TUI did
  ?                  this help;  Esc closes it
  Copy from the session in the viewer with its copy mode: the prefix twice,
  then [. The copied text goes to the desktop clipboard when the terminal
  accepts OSC 52, as kitty does, and tmux has get-clipboard (3.8 has).
  Paste an image into the session in the viewer with CMD+V: Torlek saves the
  PNG on the desktop clipboard (Wayland) and pastes its path; without one,
  the key reaches the session. torlek doctor says whether both work.
  Detach from the manager with tmux's prefix, then d.";

pub fn draw(app: &App, frame: &mut Frame<'_>, now: Instant) {
    let area = frame.area();
    if app.overlay().is_none() && app.is_phone_view() {
        crate::phone::draw(app, frame, now);
        return;
    }
    if let Some(overlay) = app.overlay() {
        let (title, text) = match overlay {
            Overlay::Help => ("Help", HELP.to_owned()),
            Overlay::Log => ("Log", app.log().collect::<Vec<_>>().join("\n")),
        };
        draw_text_view(frame, area, title, text);
        return;
    }
    if let Some(view) = &app.bug_reports {
        draw_text_view(frame, area, "Bug report drafts", view.text(app.board()));
        return;
    }
    let pane_height = if app.pane().shown().is_some() { 14 } else { 0 };
    // The accounts get only the rows the list, the pane and the status line leave.
    let quota_height = u16::try_from(app.quota_rows.len())
        .unwrap_or(u16::MAX)
        .min(area.height.saturating_sub(3 + pane_height + 1));
    let [main, pane, quota, status] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(pane_height),
        Constraint::Length(quota_height),
        Constraint::Length(1),
    ])
    .areas(area);
    match (app.form(), app.bug_form(), app.workflows()) {
        (Some(form), ..) => draw_form(form, frame, main),
        (None, Some(form), _) => draw_bug_form(form, frame, main),
        (None, None, Some(workflows)) => workflows.draw(frame, main),
        (None, None, None) => match (&app.projects, &app.tasks) {
            (Some(projects), _) => projects.draw(app.board(), frame, main),
            (None, Some(tasks)) => tasks.draw(frame, main),
            (None, None) => draw_list(app, frame, main),
        },
    }
    if pane_height > 0 {
        draw_pane(app, frame, pane, now);
    }
    let width = usize::from(quota.width);
    let quota_lines: Vec<Line<'_>> = app
        .quota_rows
        .iter()
        .map(|row| Line::from(truncated(row, width)))
        .collect();
    frame.render_widget(Paragraph::new(quota_lines), quota);
    frame.render_widget(Paragraph::new(status_line(app)), status);
}

pub(crate) fn status_line(app: &App) -> String {
    match (&app.confirm, &app.failure, &app.feed_problem) {
        (Some(Confirm::Restart(session)), ..) => format!("restart {session}? y to confirm"),
        (Some(Confirm::Exit(session)), ..) => format!("exit {session}? y to confirm"),
        (Some(Confirm::ExitFinished(sessions)), ..) => {
            format!("exit {} finished sessions? y to confirm", sessions.len())
        }
        (Some(Confirm::RemoveProject(project)), ..) => {
            format!("remove project {project}? y to confirm")
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
/// A view that is one wrapped text under its title: the help, the log, the bug report drafts.
fn draw_text_view(frame: &mut Frame<'_>, area: Rect, title: &str, text: String) {
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn draw_list(app: &App, frame: &mut Frame<'_>, area: Rect) {
    let board = app.board();
    let width = usize::from(area.width.saturating_sub(2));
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line<'_>> = Vec::new();
    let mut selected_rows = None;
    if let Some(text) = bug_reports_card(board) {
        push_wrapped(&mut lines, &text, width, CARD_INDENT, bold);
    }
    let drafts_rows = lines.len();
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
            for text in card(board, session, is_selected, width) {
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
    if lines.len() == drafts_rows {
        let text = "No project yet: torlek project add DIR";
        push_wrapped(&mut lines, text, width, "", Style::default());
    }
    let mut title = match &app.profile {
        Some(profile) => format!("Sessions · profile {profile}"),
        None => "Sessions".to_owned(),
    };
    if app.focus() == Focus::List {
        title.push_str(" *");
    }
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
pub(crate) fn scroll_to_show(rows: &std::ops::Range<usize>, height: usize) -> usize {
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
pub(crate) fn wrapped(text: &str, width: usize, indent: &str) -> Vec<String> {
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
        let mut row_columns = columns(&row);
        let mut row_has_char = false;
        for character in word.chars() {
            let character = character.to_string();
            let character_columns = columns(&character);
            if row_has_char && row_columns + character_columns > width {
                rows.push(std::mem::replace(&mut row, indent.to_owned()));
                row_columns = columns(indent);
            }
            row.push_str(&character);
            row_columns += character_columns;
            row_has_char = true;
        }
        row_has_word = true;
    }
    rows.push(row);
    rows
}

pub(crate) fn columns(text: &str) -> usize {
    ratatui::text::Span::raw(text).width()
}

/// One session's card: the id line, then whether it waits on you and what the agent reports
/// on an indented line of their own. The mark after the selection marker lets the eye find the
/// sessions that need you down the list's left edge: `!` blocked on you, `?` asked you and
/// still works (#215). A question a coordinator holds is named with it instead (#710). A running
/// session whose agent waits with nothing in progress says `waiting`, the lifecycle kept in
/// brackets (#864).
fn card(board: &Board, session: &SessionView, is_selected: bool, width: usize) -> Vec<String> {
    let marker = if is_selected { ">" } else { " " };
    let (mark, waiting) = if !board.needs_you(&session.id) {
        (" ", None)
    } else if board.asked_while_working(&session.id) {
        ("?", Some("asked you, still working"))
    } else {
        ("!", Some("waiting on you"))
    };
    let idle = board.idle(session);
    let state = match idle {
        Some(_) => format!("waiting ({})", session.state),
        None => session.state.to_string(),
    };
    let mut card = vec![format!(
        "{marker}{mark} {}  {}  {state}",
        session.id, session.agent,
    )];
    let held = board
        .question_holder(&session.id)
        .map(|coordinator| format!("question with coordinator {coordinator}"));
    // A resumed session's report predates the resume: it says nothing of the agent that runs now.
    let is_resumed = idle == Some(Idle::Resumed);
    let resumed_note = is_resumed.then_some("resumed, idle until you send it something");
    let reported = board
        .report(&session.id)
        .filter(|_| !is_resumed)
        .into_iter()
        .flat_map(|report| [report.label.as_deref(), report.doing.as_deref()])
        .flatten();
    let details: Vec<&str> = waiting
        .into_iter()
        .chain(held.as_deref())
        .chain(resumed_note)
        .chain(reported)
        .collect();
    if !details.is_empty() {
        card.push(format!("{CARD_INDENT}{}", details.join(" · ")));
    }
    if let Some(exit) = exit_line(session) {
        card.push(format!("{CARD_INDENT}{exit}"));
    }
    if let Some(compaction) = board.report(&session.id).and_then(compaction_line) {
        card.push(format!("{CARD_INDENT}{compaction}"));
    }
    if let Some(threshold) = &session.compaction_threshold {
        card.push(format!("{CARD_INDENT}compaction threshold {threshold}"));
    }
    if let Some(coordination) = coordination_line(session, is_selected) {
        let line = format!("{CARD_INDENT}{coordination}");
        card.push(if is_selected {
            line
        } else {
            truncated(&line, width)
        });
    }
    card
}

/// The session's latest compaction in its current launch (#782): `compacting (auto) from 268364
/// tokens`, `compacted (auto): 268364 -> 30470 tokens in 41s`, `compaction failed (auto)`. A
/// plain line until the card has a design for it; a size or duration the harness did not say
/// is left out.
fn compaction_line(report: &SessionReport) -> Option<String> {
    let compaction = report.compaction.as_ref()?;
    let trigger = compaction
        .trigger
        .map(|trigger| format!(" ({})", trigger.as_str()))
        .unwrap_or_default();
    let sizes = match (compaction.tokens_before, compaction.tokens_after) {
        (Some(before), Some(after)) => format!("{before} -> {after} tokens"),
        (Some(before), None) => format!("from {before} tokens"),
        (None, Some(after)) => format!("to {after} tokens"),
        (None, None) => String::new(),
    };
    let seconds = compaction
        .seconds
        .map(|seconds| format!("in {seconds}s"))
        .unwrap_or_default();
    let measured = [sizes, seconds]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    Some(match compaction.status {
        CompactionStatus::Failed => format!("compaction failed{trigger}"),
        CompactionStatus::Running if measured.is_empty() => format!("compacting{trigger}"),
        CompactionStatus::Running => format!("compacting{trigger} {measured}"),
        CompactionStatus::Completed if measured.is_empty() => format!("compacted{trigger}"),
        CompactionStatus::Completed => format!("compacted{trigger}: {measured}"),
    })
}

/// How a failed session's agent exited, and the last line it printed (#632):
/// `exited with status 3 · Error: …`. A plain line until the card has a design for it.
fn exit_line(session: &SessionView) -> Option<String> {
    if session.state != SessionStateView::Failed {
        return None;
    }
    let exit = session.exit.as_ref()?;
    let status = format!("exited with {}", exit.status);
    let line = match exit.output.last() {
        Some(printed) => format!("{status} · {printed}"),
        None => status,
    };
    Some(match line.char_indices().nth(EXIT_LINE_CHARACTERS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    })
}

/// Characters of [`exit_line`] shown: a few rows at most; `session show` prints it all.
const EXIT_LINE_CHARACTERS: usize = 120;

/// The coordinator grants a session holds and the latest message a coordinator queued to it,
/// with its sender (#410): `grants observe,message · ✉ s1a2 (queued): "text"`. A plain line
/// until the card has a design for them. A message the agent received needs nothing from the
/// owner, so only the selected card, which shows it whole, keeps it (#728).
fn coordination_line(session: &SessionView, is_selected: bool) -> Option<String> {
    let grants = (!session.coordinator_grants.is_empty()).then(|| {
        let names: Vec<&str> = session
            .coordinator_grants
            .iter()
            .map(|grant| grant.strip_suffix("-sessions").unwrap_or(grant))
            .collect();
        format!("grants {}", names.join(","))
    });
    let inbound = session
        .inbound_message
        .as_ref()
        .filter(|message| is_selected || message.delivery != "received")
        .map(|message| {
            let delivery = match &message.note {
                Some(note) => format!("{}: {note}", message.delivery),
                None => message.delivery.clone(),
            };
            format!("✉ {} ({delivery}): {:?}", message.from, message.text)
        });
    let parts: Vec<String> = grants.into_iter().chain(inbound).collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// `text` cut to one row of `width` columns, ending in `…` where it was cut.
pub(crate) fn truncated(text: &str, width: usize) -> String {
    if columns(text) <= width {
        return text.to_owned();
    }
    let mut row = String::new();
    let mut row_columns = columns("…");
    for character in text.chars() {
        let character_columns = columns(&character.to_string());
        if row_columns + character_columns > width {
            break;
        }
        row.push(character);
        row_columns += character_columns;
    }
    format!("{}…", row.trim_end())
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

/// The needs-you card of the bug report drafts: the ones waiting for a send, and apart from
/// them the ones a send opened, whose issue the owner records in `B` (#847).
pub(crate) fn bug_reports_card(board: &Board) -> Option<String> {
    let drafts = board.bug_report_drafts();
    let opened = drafts.iter().filter(|draft| draft.is_opened()).count();
    match (drafts.len() - opened, opened) {
        (0, 0) => None,
        (waiting, 0) => Some(format!("! Bug report drafts ({waiting}): B lists them")),
        (0, opened) => Some(format!(
            "! Bug reports opened to send ({opened}): B records their issue"
        )),
        (waiting, opened) => Some(format!(
            "! Bug report drafts ({waiting}), {opened} more opened to send: B lists them"
        )),
    }
}

fn draw_bug_form(form: &BugForm, frame: &mut Frame<'_>, area: Rect) {
    let field = |name: BugField, label: &str, value: &str| {
        let marker = if form.field == name { ">" } else { " " };
        Line::from(format!("{marker} {label}: {value}"))
    };
    let mut lines = vec![
        field(BugField::Description, "What went wrong", &form.description),
        field(BugField::Title, "Title (optional)", &form.title),
        field(BugField::Steps, "Steps (optional)", &form.steps),
        Line::from(""),
        Line::from("Tab next field  Enter draft it  Esc cancel"),
        Line::from("It is a draft: nothing is sent until you run torlek bug-report send."),
    ];
    if form.submitted.is_some() {
        lines.push(Line::from("Drafting…"));
    }
    if let Some(problem) = &form.problem {
        lines.push(Line::from(format!("Cannot draft: {problem}")));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Report a Torlek bug"),
        ),
        area,
    );
}

fn draw_pane(app: &App, frame: &mut Frame<'_>, area: Rect, now: Instant) {
    let pane = app.pane();
    let board = app.board();
    let lines = match pane.shown() {
        Some(Shown::Run { id, .. }) => board
            .runs
            .iter()
            .find(|run| run.id == *id)
            .map(|run| run_lines(run, pane.is_expanded()))
            .unwrap_or_default(),
        Some(Shown::Guide { id, .. }) => board
            .guides
            .iter()
            .find(|guide| guide.id == *id)
            .map(guide_lines)
            .unwrap_or_default(),
        Some(Shown::Network { id, .. }) => board
            .network
            .iter()
            .find(|request| request.id == *id)
            .map(network_lines)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let input = match pane.typing() {
        Some(_) => typed_input_lines(pane),
        None if pane.in_grace(now) => {
            vec!["Read it first: keys work again in a moment.".to_owned()]
        }
        None => vec![pane_keys(pane).to_owned()],
    };
    let title = if app.focus() == Focus::Pane {
        "Waiting on you *"
    } else {
        "Waiting on you (Tab)"
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // The input sits at the bottom, whole: a refusal under it is never cut off by a long
    // request above (#1372).
    let input_rows: Vec<Line<'_>> = input
        .iter()
        .flat_map(|line| wrapped(line, usize::from(inner.width), ""))
        .map(Line::from)
        .collect();
    let input_height = u16::try_from(input_rows.len()).unwrap_or(u16::MAX);
    let [details, input_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(input_height)]).areas(inner);
    frame.render_widget(
        Paragraph::new(lines.join("\n")).wrap(Wrap { trim: false }),
        details,
    );
    frame.render_widget(Paragraph::new(input_rows), input_area);
}

/// The line the owner types into the pane, with its prompt, and under it why the daemon did
/// not type the last one into the command, if it refused it (#1372).
pub(crate) fn typed_input_lines(pane: &Pane) -> Vec<String> {
    let Some((typing, typed)) = pane.typing() else {
        return Vec::new();
    };
    let mut lines = vec![typing_line(typing, typed, pane.is_input_revealed())];
    if let Some(problem) = pane.input_refusal() {
        lines.push(format!("Not sent: {problem}"));
        lines.push(INPUT_REFUSED_NEXT.to_owned());
    }
    lines
}

/// What the owner can do instead of a refused line, safest first. A visible answer is only
/// for a prompt that masks each key: one that echoes copies the line into the agent's output.
const INPUT_REFUSED_NEXT: &str = "Your line is kept; Esc drops it. Safest: Esc, then x stops \
    the command so its agent gets an answer, and you run it in your own terminal. For sudo, \
    `Defaults !pwfeedback` in sudoers gives a prompt the pane can answer. Or, only for a prompt \
    that shows a * per key, Tab then Enter sends it anyway: your line then shows here in clear.";

/// What the owner types into the pane, with its prompt. A line for a command shows one `*`
/// per character unless `is_input_revealed`: it may be a password (#1010).
pub(crate) fn typing_line(typing: Typing, typed: &str, is_input_revealed: bool) -> String {
    let (prompt, reveal) = match typing {
        Typing::Input => (
            "Line for the command",
            format!(", {}", reveal_hint(is_input_revealed)),
        ),
        Typing::SudoPassword => ("sudo's password", ", to sudo alone".to_owned()),
        Typing::Answer => ("Answer", String::new()),
        Typing::StuckNote => ("Stuck because", String::new()),
    };
    let is_masked = match typing {
        Typing::Input => !is_input_revealed,
        Typing::SudoPassword => true,
        Typing::Answer | Typing::StuckNote => false,
    };
    let shown = if is_masked {
        "*".repeat(typed.chars().count())
    } else {
        typed.to_owned()
    };
    format!("{prompt}: {shown}_   (Enter sends{reveal}, Esc cancels)")
}

/// What Tab does to the line typed for a command. A revealed line is sent as a visible answer,
/// which the command's terminal may echo into the output (#1160).
pub(crate) fn reveal_hint(is_input_revealed: bool) -> &'static str {
    if is_input_revealed {
        "sent shown, echo and all; Tab hides it"
    } else {
        "Tab shows it, for a visible answer"
    }
}

pub(crate) fn pane_keys(pane: &Pane) -> &'static str {
    if pane.is_confirming_stop() {
        return "stop it and every process it started? y to confirm, any other key keeps it";
    }
    match pane.shown() {
        Some(Shown::Run {
            sudo_ask: Some(_), ..
        }) => "i type sudo's password   x stop it   m more",
        Some(Shown::Run {
            waiting_for_input: true,
            ..
        }) => "i type a line   x stop it   m more",
        Some(Shown::Run { .. }) => "y approve   x decline   m more",
        Some(Shown::Guide { asks: true, .. }) => "y answer   s stuck   x close",
        Some(Shown::Guide { .. }) => "y done   s stuck   x close",
        Some(Shown::Network {
            can_allow_once: true,
            ..
        }) => "y allow once   a allow always   x deny",
        Some(Shown::Network { .. }) => "a allow always   x deny",
        None => "",
    }
}

/// A network request's lines (#674): "allow once" lasts the rest of the asking session, a
/// check's request has none (#352), "allow always" is a grant to the project the owner can
/// revoke. A service's request names
/// its whole record (#880): what the grant is bound to.
pub(crate) fn network_lines(request: &NetworkRequestView) -> Vec<String> {
    let asked = match &request.asks {
        NetworkTargetView::Host { .. } => {
            format!("Host: {}  [refused: no entry allows it]", request.target)
        }
        NetworkTargetView::Service { .. } => format!(
            "Service: {}  [closed: no grant allows it; the service sees a local client]",
            request.target
        ),
    };
    let mut lines = vec![
        format!(
            "Network request {} from {} (project {})",
            request.id,
            crate::board::network_requester(request),
            request.project
        ),
        asked,
    ];
    if let Some(reason) = &request.shown_reason {
        lines.push(format!("Reason: {}", reason.text));
        let warnings = warnings(&[reason]);
        if !warnings.is_empty() {
            lines.push(format!("Warning: {warnings}"));
        }
    }
    lines.push(match request.subject {
        NetworkSubjectView::Session { .. } => "Allow once: for the rest of this session. \
            Allow always: every session of the project, until `torlek network revoke`."
            .to_owned(),
        NetworkSubjectView::Check { .. } => "Allow always: every check and session of the \
            project, until `torlek network revoke`; run the check again. Deny: its next runs \
            are refused it too."
            .to_owned(),
        NetworkSubjectView::Containers { .. } => "Allow always: the project's containers and \
            sessions, until `torlek network revoke`. Deny: its containers stay refused it."
            .to_owned(),
    });
    lines
}

/// A run request's lines; `is_expanded` shows the whole command instead of its folded start.
pub(crate) fn run_lines(run: &RunRequestView, is_expanded: bool) -> Vec<String> {
    let waiting = crate::pane::waits_for_input(run);
    let sudo_ask = run.sudo_ask.as_ref().filter(|_| waiting);
    let project = run
        .project
        .as_ref()
        .map(|project| format!(" (project {project})"))
        .unwrap_or_default();
    let mut lines = vec![match (sudo_ask, waiting) {
        (Some(_), _) => format!(
            "sudo asks for your password for request {} from {}{project}",
            run.id, run.session
        ),
        (None, true) => format!(
            "Command waiting for input: {} from {}{project}",
            run.id, run.session
        ),
        (None, false) => format!("Run request {} from {}{project}", run.id, run.session),
    }];
    let prompt = match sudo_ask {
        Some(ask) => Some(&ask.prompt),
        None => run.waiting_prompt.as_ref().filter(|_| waiting),
    };
    if let Some(prompt) = prompt {
        lines.push(format!("Prompt: {}", prompt.text));
    }
    if sudo_ask.is_some() {
        lines.push(
            "Your password goes to sudo through Torlek's askpass helper: never the command's \
             terminal or its output."
                .to_owned(),
        );
    }
    let ran_unasked = run.sudo_ran_unasked_so_far();
    if ran_unasked > 0 {
        lines.push(format!(
            "Warning: {}",
            sudo_ran_unasked_warning(ran_unasked)
        ));
    }
    let typed_secret_redacted = run.typed_secret_redacted_so_far();
    if typed_secret_redacted > 0 {
        lines.push(format!(
            "Warning: {}",
            typed_secret_redacted_notice(typed_secret_redacted)
        ));
    }
    let outside = if run.outside_project {
        "  [outside the project]"
    } else {
        ""
    };
    lines.push(format!("Directory: {}{outside}", run.shown_cwd.text));
    lines.push(format!("Reason: {}", run.shown_reason.text));
    match (&run.folded_command, is_expanded) {
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
    lines.extend(
        run.limits
            .iter()
            .map(|limit| format!("Limit: {}", limit.text)),
    );
    let mut shown_texts = vec![&run.shown_command, &run.shown_cwd, &run.shown_reason];
    shown_texts.extend(prompt);
    shown_texts.extend(&run.limits);
    let warnings = warnings(&shown_texts);
    if !warnings.is_empty() {
        lines.push(format!("Warning: {warnings}"));
    }
    if run.timeout_without_foreground {
        lines.push(
            "Warning: timeout without --foreground: a prompt in the command would stop it"
                .to_owned(),
        );
    }
    if let Some(shell) = &run.unchecked_shell {
        lines.push(format!(
            "Warning: not syntax-checked: {shell} has no known parse-only mode"
        ));
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
    if let Some(findings) = &run.findings {
        lines.extend(findings_lines(findings, is_expanded));
    }
    if let Some(root_runs) = &run.root_runs {
        lines.extend(root_runs_lines(root_runs, is_expanded));
    }
    lines.extend(pinned_lines(run, is_expanded));
    lines
}

/// The other files a request runs that the asking session can change, pinned (#1387), shown
/// as the files sudo runs are.
fn pinned_lines(run: &RunRequestView, is_expanded: bool) -> Vec<String> {
    let files: Vec<&RootFileView> = run.pinned_beyond_root().collect();
    file_lines(&files, "Pinned", "the pinned files", is_expanded)
}
/// The width the card's findings and warnings about what runs as root keep to, a long text
/// shortened: the pane's inside on an 80-column terminal, its borders taking two.
const ROOT_LINE_COLUMNS: usize = 78;
const WARNING: &str = "Warning: ";

/// The findings the card shows before `m` shows them all.
const FINDINGS_FOLDED: usize = 3;

/// What the rule scan found (#1388), high first, one line each within 78 columns with the
/// matching text marked; expanded, every finding with its whole text.
fn findings_lines(findings: &[FindingView], is_expanded: bool) -> Vec<String> {
    if findings.is_empty() {
        return vec![NO_KNOWN_RISKY_PATTERN.to_owned()];
    }
    let mut lines: Vec<String> = if is_expanded {
        findings
            .iter()
            .map(|finding| finding_line(finding, &finding.excerpt.text))
            .collect()
    } else {
        findings
            .iter()
            .take(FINDINGS_FOLDED)
            .map(folded_finding_line)
            .collect()
    };
    let more = findings.len().saturating_sub(FINDINGS_FOLDED);
    if !is_expanded && more > 0 {
        lines.push(format!("  … {more} more findings (m shows them)"));
    }
    let shown: Vec<&ShownText> = findings
        .iter()
        .flat_map(|finding| std::iter::once(&finding.excerpt).chain(&finding.file))
        .collect();
    let hidden = warnings(&shown);
    if !hidden.is_empty() {
        lines.push(format!("{WARNING}in the findings: {hidden}"));
    }
    lines
}

/// One finding within [`ROOT_LINE_COLUMNS`]: its file shortened at its start when it would
/// leave the matching text less than [`EXCERPT_ROOM_MIN`], then the matching text cut at its
/// end.
fn folded_finding_line(finding: &FindingView) -> String {
    let whole = finding_line(finding, &finding.excerpt.text);
    if columns(&whole) <= ROOT_LINE_COLUMNS {
        return whole;
    }
    let mut folded = finding.clone();
    let over = (columns(&finding_line(&folded, "")) + EXCERPT_FRAME + EXCERPT_ROOM_MIN)
        .saturating_sub(ROOT_LINE_COLUMNS);
    if let Some(file) = folded.file.as_mut().filter(|_| over > 0) {
        file.text = shortened(&file.text, columns(&file.text).saturating_sub(over));
    }
    let room =
        ROOT_LINE_COLUMNS.saturating_sub(columns(&finding_line(&folded, "")) + EXCERPT_FRAME);
    let excerpt = cut_end(&folded.excerpt.text, room);
    finding_line(&folded, &excerpt)
}

/// What [`finding_line`] puts around the matching text: `: ` and two backticks.
const EXCERPT_FRAME: usize = 4;

/// The least of a finding's matching text a long file name leaves.
const EXCERPT_ROOM_MIN: usize = 20;

/// `text` within `room` columns: its start kept, an ellipsis for what was cut.
fn cut_end(text: &str, room: usize) -> String {
    if columns(text) <= room {
        return text.to_owned();
    }
    let mut kept = String::new();
    for character in text.chars() {
        if columns(&kept) + columns(&character.to_string()) + 1 > room {
            break;
        }
        kept.push(character);
    }
    format!("{kept}…")
}

/// What a request's sudo runs that the asking session can change (#1383), last before the
/// keys. That it runs them is among the findings.
fn root_runs_lines(root_runs: &RootRunsView, is_expanded: bool) -> Vec<String> {
    let files: Vec<&RootFileView> = root_runs.editable.iter().collect();
    file_lines(&files, "Runs as root", "what runs as root", is_expanded)
}

/// Files a request runs that the asking session can change: a file changed since the ask,
/// and hidden characters; expanded, each file's hash and first lines under `label`. `place`
/// names them in the hidden-character warning.
fn file_lines(files: &[&RootFileView], label: &str, place: &str, is_expanded: bool) -> Vec<String> {
    let mut lines = Vec::new();
    for file in files.iter().filter(|file| file.changed_since_asked()) {
        let room = ROOT_LINE_COLUMNS
            .saturating_sub(WARNING.len() + changed_since_asked_warning("").chars().count());
        lines.push(format!(
            "{WARNING}{}",
            changed_since_asked_warning(&shortened(&file.shown_path.text, room))
        ));
    }
    let shown: Vec<&ShownText> = files
        .iter()
        .flat_map(|file| {
            let head = file.now.as_ref().and_then(|now| now.head.as_ref());
            std::iter::once(&file.shown_path).chain(head)
        })
        .collect();
    let hidden = warnings(&shown);
    if !hidden.is_empty() {
        lines.push(format!("{WARNING}in {place}: {hidden}"));
    }
    if !is_expanded {
        return lines;
    }
    for file in files {
        lines.push(format!("{label}: {}", file.shown_path.text));
        let Some(now) = &file.now else {
            lines.push(format!("  sha256 {}", hash_text(file.sha256.as_deref())));
            continue;
        };
        if now.unreadable {
            lines.push("  sha256 unknown: it cannot be read now".to_owned());
            continue;
        }
        lines.push(format!("  sha256 {}", hash_text(now.sha256.as_deref())));
        match (&now.head, &now.sha256) {
            (Some(head), _) => {
                lines.extend(head.text.lines().map(|line| format!("  | {line}")));
                if now.more_lines > 0 {
                    lines.push(format!("  … {} more lines", now.more_lines));
                }
            }
            (None, Some(_)) => lines.push("  (not text)".to_owned()),
            (None, None) => {}
        }
    }
    lines
}

fn hash_text(sha256: Option<&str>) -> &str {
    sha256.unwrap_or("none: the file is not there")
}

/// `path` within `room` characters: its end kept, an ellipsis for what was cut.
fn shortened(path: &str, room: usize) -> String {
    let count = path.chars().count();
    if count <= room {
        return path.to_owned();
    }
    let kept: String = path.chars().skip(count - room.saturating_sub(1)).collect();
    format!("…{kept}")
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

pub(crate) fn guide_lines(guide: &GuideView) -> Vec<String> {
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
