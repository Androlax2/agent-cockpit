//! claude-guide <id>: steps Claude handed Theo to do by hand (ask-to-guide), one at a time.
//!
//! Started by the OSD daemon outside the sandbox, in a floating kitty window of its own (class
//! claude-guide) that Theo moves wherever he needs it. The guide comes from the sandbox: it is read
//! once, from the daemon's copy (approval::held_request), which no sandbox reaches, and checked; hidden characters are shown escaped; links open only on Ctrl+O, values
//! are copied only on Ctrl+Y; a run: step goes through the same approval as ask-to-run: nothing runs
//! without Theo's "Run it". The text box under the step is always open: the answer when the step
//! asks something, else a note to Claude. When every step is done, or Theo is stuck or closes the
//! guide, the result goes back to Claude; <id>.progress tells a waiting Claude where Theo is.
//! XDG_RUNTIME_DIR moves the daemon's copies (tests).

use cockpit::app::{put, style, width_of};
use cockpit::approval::{self, BOLD, DIM, RED, RESET, YELLOW};
use cockpit::util::now_secs;
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use crossterm::{cursor, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Modifier, Style};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

const MAX_AGE_S: i64 = 2 * 3600; // a guide can wait in the queue behind another one
const MAX_STEPS: usize = 60;
const MAX_TEXT: usize = 2000;
const STEP_FIELDS: [&str; 6] = ["text", "section", "open", "copy", "ask", "run"];
const HINTS: [&str; 2] = ["⏎ next   ↑ back   ⌘O open   ⌘Y copy   ⌘R run", "⌘S stuck: send my note now   Esc close"];
const CYAN: u8 = 6;
const BLUE: u8 = 4;
const GREEN: u8 = 2;
const RED_INDEX: u8 = 1;

/// The guide, checked, with the answers folder it names, or why it cannot be shown.
fn read_guide(id: &str) -> Result<(Value, PathBuf), String> {
    if !approval::valid_id(id) {
        return Err("not a guide id".into());
    }
    let text = std::fs::read_to_string(approval::held_request(id, "guide")).map_err(|error| error.to_string())?;
    let guide: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let answers_folder = approval::request_answers_dir(&guide, "guides")
        .ok_or("it names no answers folder (its session started before answers went per project: restart it)")?;
    if answers_folder.join(format!("{id}.result")).exists() {
        return Err("already answered".into());
    }
    if !guide.is_object() || now_secs() - guide.get("requested_at").and_then(Value::as_i64).unwrap_or(0) > MAX_AGE_S {
        return Err("older than two hours".into());
    }
    let steps = guide.get("steps").and_then(Value::as_array).filter(|steps| !steps.is_empty() && steps.len() <= MAX_STEPS)
        .ok_or("no steps, or too many")?;
    for step in steps {
        if !step.get("text").and_then(Value::as_str).is_some_and(|text| !text.trim().is_empty()) {
            return Err("a step without text".into());
        }
        let fields_ok = STEP_FIELDS.iter().all(|field| match step.get(*field) {
            None => true,
            Some(Value::String(text)) => text.chars().count() <= MAX_TEXT,
            Some(_) => false,
        });
        if !fields_ok {
            return Err("a step field that is not text, or too long".into());
        }
    }
    if guide.get("title").is_some_and(|title| !title.is_string()) {
        return Err("a title that is not text".into());
    }
    Ok((guide, answers_folder))
}

/// Text as displayed: characters that could hide what it says are escaped.
fn shown(text: &str) -> String {
    approval::escape_hiding(text, "", "")
}

/// The link Ctrl+O opens: the step's open: value, else the first link in its text; http(s) only.
fn link_of(step: &Map<String, Value>) -> String {
    let link = Regex::new(r#"https?://[^\s<>"')]+"#).unwrap();
    let candidate = match step.get("open").and_then(Value::as_str).filter(|open| !open.is_empty()) {
        Some(open) => open.to_string(),
        None => link.find(step.get("text").and_then(Value::as_str).unwrap_or("")).map(|found| found.as_str().to_string()).unwrap_or_default(),
    };
    let whole = Regex::new(r#"^https?://[^\s<>"')]+$"#).unwrap();
    if whole.is_match(&candidate) { candidate } else { String::new() }
}

/// Where Theo is: the step on screen, the steps done, his answers, notes and the commands run.
struct Guide {
    id: String,
    title: String,
    steps: Vec<Map<String, Value>>,
    cwd: String,
    project: String,
    /// Where Theo's answer and progress go: the folder the guide names (approval::answers_dir).
    answers_folder: PathBuf,
    position: usize,
    done: BTreeSet<usize>,
    answers: BTreeMap<usize, String>,
    notes: BTreeMap<usize, String>,
    runs: BTreeMap<usize, Map<String, Value>>,
}

impl Guide {
    fn new(id: &str, guide: &Value, answers_folder: PathBuf) -> Self {
        let steps = guide["steps"].as_array().unwrap().iter().filter_map(|step| step.as_object().cloned()).collect();
        let cwd = guide.get("cwd").and_then(Value::as_str).and_then(approval::resolved_directory)
            .unwrap_or_else(|| cockpit::paths::home().to_string_lossy().into_owned());
        let title = guide.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or("Steps from Claude").to_string();
        // The project comes from the daemon's copy: the one of the folder the session's sandbox started in.
        let project = guide.get("project").and_then(Value::as_str).unwrap_or("?").to_string();
        Guide { id: id.into(), title, steps, project, cwd, answers_folder, position: 0, done: BTreeSet::new(),
                answers: BTreeMap::new(), notes: BTreeMap::new(), runs: BTreeMap::new() }
    }

    fn step(&self) -> &Map<String, Value> {
        &self.steps[self.position]
    }

    fn field(&self, name: &str) -> String {
        self.step().get(name).and_then(Value::as_str).unwrap_or("").to_string()
    }

    fn move_by(&mut self, offset: i64) {
        self.position = (self.position as i64 + offset).clamp(0, self.steps.len() as i64 - 1) as usize;
    }

    /// Marks the step on screen done and moves to the next one not done; true when all are.
    fn mark_done(&mut self) -> bool {
        self.done.insert(self.position);
        let remaining: Vec<usize> = (0..self.steps.len()).filter(|index| !self.done.contains(index)).collect();
        let Some(first) = remaining.first().copied() else { return true };
        self.position = remaining.iter().copied().find(|index| *index > self.position).unwrap_or(first);
        false
    }

    /// What the box holds for the step on screen: its answer, or its note to Claude.
    fn typed(&self) -> String {
        let source = if self.field("ask").is_empty() { &self.notes } else { &self.answers };
        source.get(&self.position).cloned().unwrap_or_default()
    }

    /// The box's text: the answer when the step asks something, else a note (dropped when empty).
    fn submit(&mut self, text: &str) {
        let text = cockpit::util::collapse(text);
        if !self.field("ask").is_empty() {
            self.answers.insert(self.position, text);
        } else if !text.is_empty() {
            self.notes.insert(self.position, text);
        } else {
            self.notes.remove(&self.position);
        }
    }

    fn progress(&self) -> Value {
        json!({"step": self.position + 1, "total": self.steps.len(), "done": self.done.len()})
    }

    fn result(&self, status: &str, note: &str) -> Value {
        json!({
            "status": status, "step": self.position + 1, "total": self.steps.len(),
            "step_text": self.field("text").split('\n').next().unwrap_or(""), "note": note,
            "done": self.done.iter().map(|index| index + 1).collect::<Vec<_>>(),
            "answers": self.answers.iter().map(|(index, answer)| json!({"step": index + 1, "question": self.steps[*index].get("ask"), "answer": answer})).collect::<Vec<_>>(),
            "notes": self.notes.iter().map(|(index, note)| json!({"step": index + 1, "note": note})).collect::<Vec<_>>(),
            "runs": self.runs.iter().map(|(index, run)| { let mut run = run.clone(); run.insert("step".into(), json!(index + 1)); Value::Object(run) }).collect::<Vec<_>>(),
        })
    }

    fn file(&self, extension: &str) -> PathBuf {
        self.answers_folder.join(format!("{}.{extension}", self.id))
    }
}

fn finish(guide: &Guide, status: &str, note: &str) {
    let _ = approval::write_json(&guide.file("result"), &guide.result(status, note));
    let _ = std::fs::remove_file(guide.file("progress"));
    approval::send_host(&format!("notice-clear guide-{}", guide.id));
    let note_part = if note.is_empty() { String::new() } else { format!(" ({})", note.chars().take(80).collect::<String>()) };
    approval::log("INFO", &format!("guide {} for {}: {status} at step {}/{}{note_part}", guide.id, guide.project, guide.position + 1, guide.steps.len()));
}

/// The step on screen as (text, style) lines: its section, its text, its link, value and command
/// with their keys, and how its command went.
fn render_step(guide: &Guide, width: usize) -> Vec<(String, Style)> {
    let wrap = |text: &str, indent: usize| -> Vec<String> {
        let lines = approval::wrap(&shown(text), width.saturating_sub(indent).max(10));
        if lines.is_empty() { vec![String::new()] } else { lines.into_iter().map(|line| format!("{}{line}", " ".repeat(indent))).collect() }
    };
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    let section = guide.field("section");
    if !section.is_empty() {
        lines.push((shown(&section).chars().take(width).collect(), style(CYAN).add_modifier(Modifier::BOLD)));
        lines.push((String::new(), Style::default()));
    }
    for paragraph in guide.field("text").split('\n') {
        lines.extend(wrap(paragraph, 0).into_iter().map(|line| (line, bold)));
    }
    if STEP_FIELDS.iter().any(|field| { let value = guide.field(field); shown(&value) != value }) {
        lines.push((String::new(), Style::default()));
        lines.push(("⚠ hidden characters, shown as \\uXXXX".into(), style(RED_INDEX).add_modifier(Modifier::BOLD)));
    }
    let details: Vec<(&str, String, &str)> = [("link", link_of(guide.step()), "⌘O opens"), ("copy", guide.field("copy"), "⌘Y copies"),
                                               ("run", guide.field("run"), "⌘R runs (⏎ too)")]
        .into_iter().filter(|(_, value, _)| !value.is_empty()).collect();
    if !details.is_empty() {
        lines.push((String::new(), Style::default()));
    }
    for (label, value, key) in details {
        lines.push((format!("  {label} · {key}"), style(BLUE)));
        lines.extend(wrap(&value, 4).into_iter().map(|line| (line, Style::default())));
    }
    if let Some(run) = guide.runs.get(&guide.position) {
        let outcome = if run.get("status").and_then(Value::as_str) == Some("declined") { "declined".to_string() } else { format!("exit {}", run.get("exit").cloned().unwrap_or(json!(""))) };
        lines.push((format!("  ↳ {outcome}"), style(GREEN)));
    }
    lines
}

fn draw(buffer: &mut Buffer, guide: &Guide, draft: &str, message: &str) -> (u16, u16) {
    let (width, height) = (buffer.area.width as usize, buffer.area.height as usize);
    let counter = format!("step {} of {}", guide.position + 1, guide.steps.len());
    put(buffer, 0, 1, &shown(&guide.title).chars().take(width.saturating_sub(counter.len() + 4).max(1)).collect::<String>(), Style::default().add_modifier(Modifier::BOLD));
    put(buffer, 0, width.saturating_sub(counter.len() + 2) as u16, &counter, style(CYAN));
    let done = if guide.done.is_empty() { String::new() } else { format!(" · {} done", guide.done.len()) };
    put(buffer, 1, 1, &format!("Claude · {}{done}", guide.project), Style::default().add_modifier(Modifier::DIM));
    put(buffer, 2, 1, &"─".repeat(width.saturating_sub(3)), Style::default().add_modifier(Modifier::DIM));
    let box_width = width.saturating_sub(6).max(10);
    let mut box_lines = approval::wrap(draft, box_width);
    if draft.ends_with(' ') && !box_lines.is_empty() {
        let last = box_lines.len() - 1;
        box_lines[last].push(' ');
    }
    if box_lines.is_empty() {
        box_lines.push(String::new());
    }
    let box_lines: Vec<String> = box_lines[box_lines.len().saturating_sub(3)..].to_vec();
    let box_top = height.saturating_sub(4 + box_lines.len() + 3);
    for (row, (text, line_style)) in render_step(guide, width.saturating_sub(4)).into_iter().take(box_top.saturating_sub(5)).enumerate() {
        put(buffer, 4 + row as u16, 2, &text, line_style);
    }
    let ask = guide.field("ask");
    let (question, question_style) = if ask.is_empty() {
        ("A note for Claude about this step (optional)".to_string(), style(BLUE).add_modifier(Modifier::DIM))
    } else {
        (format!("Claude asks: {}", shown(&ask)), style(BLUE).add_modifier(Modifier::BOLD))
    };
    put(buffer, box_top.saturating_sub(1) as u16, 2, &question, question_style);
    let rule = "─".repeat(width.saturating_sub(4));
    put(buffer, box_top as u16, 1, &format!("┌{rule}┐"), Style::default().add_modifier(Modifier::DIM));
    for (row, line) in box_lines.iter().enumerate() {
        let padding = width.saturating_sub(6).saturating_sub(width_of(line));
        put(buffer, (box_top + 1 + row) as u16, 1, &format!("│ {line}{} │", " ".repeat(padding)), Style::default());
    }
    put(buffer, (box_top + 1 + box_lines.len()) as u16, 1, &format!("└{rule}┘"), Style::default().add_modifier(Modifier::DIM));
    put(buffer, height.saturating_sub(3) as u16, 2, message, style(GREEN));
    for (offset, hint) in HINTS.iter().enumerate() {
        put(buffer, (height.saturating_sub(2) + offset) as u16, 2, hint, Style::default().add_modifier(Modifier::DIM));
    }
    let last = box_lines.last().map(|line| width_of(line)).unwrap_or(0);
    ((3 + last).min(width.saturating_sub(3)) as u16, (box_top + box_lines.len()) as u16)
}

/// The step's command, through the ask-to-run approval: shown escaped, Decline by default.
fn run_step(guide: &mut Guide) {
    let original = guide.field("run");
    let mut command = original.clone();
    if let Err(error) = approval::enter_directory(&guide.cwd) {
        println!("{RED}cannot enter {}: {error}{RESET}", approval::escape_hiding(&guide.cwd, "", ""));
        return;
    }
    let shown_cwd = approval::escape_hiding(&guide.cwd, RED, &format!("{RESET}{DIM}"));
    println!("\n{BOLD}Step {} of the guide{RESET} asks you to run, in {DIM}{shown_cwd}{RESET}:\n", guide.position + 1);
    let answer = loop {
        let (text, hiding) = approval::visible_command(&command);
        println!("{}\n", text.split('\n').map(|line| format!("  {BOLD}{line}{RESET}")).collect::<Vec<_>>().join("\n"));
        if hiding {
            println!("{RED}{BOLD}This command contains hidden characters or blank-line padding (shown in red). Decline unless you know why.{RESET}\n");
        }
        let foreign = approval::non_ascii(&command);
        if !foreign.is_empty() {
            println!("{YELLOW}Characters outside ASCII (a lookalike letter can change a command): {}{RESET}\n", foreign.join(", "));
        }
        let answer = approval::choose("Run it?", &[("n", "Decline"), ("y", "Run it"), ("e", "Edit it first")], "n");
        if answer != "e" {
            break answer;
        }
        if let Some(edited) = approval::edit(&command) {
            command = edited;
        }
    };
    let edited = if command != original { json!(command) } else { Value::Null };
    let mut run = Map::new();
    run.insert("command".into(), json!(original));
    run.insert("edited_command".into(), edited);
    if answer != "y" {
        run.insert("status".into(), json!("declined"));
        guide.runs.insert(guide.position, run);
        return;
    }
    let log_path = approval::log_dir().join(format!("guide-{}-{}.log", guide.id, guide.position + 1));
    let exit_code = approval::run(&command, "", &log_path, None);
    println!("{DIM}─── {RESET}exit {exit_code}");
    // Like ask-to-run: the output always goes back, secrets taken out first.
    let (output, redacted) = approval::redact_secrets(&approval::recorded_output(&log_path));
    if redacted > 0 {
        println!("{redacted} secret-looking value{} redacted before the output went to Claude.", if redacted == 1 { "" } else { "s" });
    }
    run.insert("status".into(), json!("ran"));
    run.insert("exit".into(), json!(exit_code));
    run.insert("output".into(), json!(output));
    run.insert("redacted".into(), json!(redacted));
    guide.runs.insert(guide.position, run);
    let _ = std::fs::remove_file(&log_path);
}

fn copy(value: &str) -> String {
    let child = Command::new("wl-copy").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    match child {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(value.as_bytes());
            }
            match child.wait() {
                Ok(status) if status.success() => "copied".into(),
                Ok(status) => format!("cannot copy: wl-copy exited {}", status.code().unwrap_or(-1)),
                Err(error) => format!("cannot copy: {error}"),
            }
        }
        Err(error) => format!("cannot copy: {error}"),
    }
}

fn open_link(link: &str) -> String {
    match Command::new("xdg-open").arg(link).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        Ok(_) => "opened in the browser".into(),
        Err(error) => format!("cannot open: {error}"),
    }
}

fn enter() -> io::Result<()> {
    enable_raw_mode()?; // Ctrl+S, Ctrl+O, Ctrl+Y and Ctrl+C reach the guide instead of the terminal
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)
}

fn leave() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen, cursor::Show)
}

/// Leaves the guide's screen for a plain terminal (prompts, a command's output), then comes back.
fn outside<T>(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, action: impl FnOnce() -> T) -> T {
    let _ = leave();
    let result = action();
    let _ = enter();
    let _ = terminal.clear();
    result
}

fn run_loop(guide: &mut Guide) -> io::Result<String> {
    enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut draft = guide.typed();
    let mut message = String::new();
    loop {
        let _ = approval::write_json(&guide.file("progress"), &guide.progress());
        let mut cursor_at = (0, 0);
        terminal.draw(|frame| cursor_at = draw(frame.buffer_mut(), guide, &draft, &message))?;
        execute!(io::stdout(), cursor::MoveTo(cursor_at.0, cursor_at.1), cursor::Show)?;
        let event = event::read()?;
        message.clear();
        let key = match event {
            Event::Paste(pasted) => {
                // Line breaks become spaces: a paste never moves on.
                draft.push_str(&pasted.replace(['\r', '\n'], " "));
                continue;
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => key,
            _ => continue,
        };
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => {
                if !guide.field("run").is_empty() && !guide.runs.contains_key(&guide.position) {
                    outside(&mut terminal, || run_step(guide));
                }
                guide.submit(&draft);
                if guide.mark_done() {
                    finish(guide, "done", "");
                    leave()?;
                    return Ok("Every step done: Claude is told.".into());
                }
                draft = guide.typed();
            }
            KeyCode::Up => {
                guide.submit(&draft);
                guide.move_by(-1);
                draft = guide.typed();
            }
            KeyCode::Backspace => { draft.pop(); }
            KeyCode::Char('o') if control => {
                let link = link_of(guide.step());
                message = if link.is_empty() { "no link on this step".into() } else { open_link(&link) };
            }
            KeyCode::Char('y') if control => {
                let value = guide.field("copy");
                message = if value.is_empty() { "nothing to copy on this step".into() } else { copy(&value) };
            }
            KeyCode::Char('r') if control => {
                if !guide.field("run").is_empty() {
                    outside(&mut terminal, || run_step(guide));
                }
            }
            KeyCode::Char('s') if control => {
                guide.submit(&draft);
                let note = if draft.trim().is_empty() { "(no note)".to_string() } else { draft.trim().to_string() };
                finish(guide, "stuck", &note);
                leave()?;
                return Ok("Claude is told where you are stuck.".into());
            }
            KeyCode::Esc | KeyCode::Char('c') if key.code == KeyCode::Esc || control => {
                guide.submit(&draft);
                let closing = outside(&mut terminal, || approval::choose("Close the guide? Claude is told where you stopped.",
                                                                            &[("n", "Keep going"), ("y", "Close it")], "n"));
                if closing == "y" {
                    finish(guide, "closed", "");
                    leave()?;
                    return Ok("Closed: Claude is told.".into());
                }
            }
            KeyCode::Char(character) if !control => draft.push(character),
            _ => {}
        }
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() != 1 {
        eprintln!("usage: claude-guide <id>");
        std::process::exit(1);
    }
    let mut guide = match read_guide(&arguments[0]) {
        Ok((found, answers_folder)) => Guide::new(&arguments[0], &found, answers_folder),
        Err(reason) => {
            println!("Cannot show guide {}: {reason}.", arguments[0]);
            std::thread::sleep(Duration::from_secs(5));
            std::process::exit(1);
        }
    };
    match run_loop(&mut guide) {
        Ok(message) => println!("{message}"),
        Err(error) => {
            let _ = leave();
            eprintln!("claude-guide: {error}");
        }
    }
    std::thread::sleep(Duration::from_millis(1500));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guide(steps: Value) -> Guide {
        Guide::new("aaaaaaaaaaaa", &json!({"title": "Key", "cwd": "/", "steps": steps}), std::env::temp_dir())
    }

    #[test]
    fn done_moves_to_the_next_step_not_done_and_reports_the_end() {
        let mut guide = guide(json!([{"text": "a"}, {"text": "b"}, {"text": "c"}]));
        guide.move_by(1);
        assert!(!guide.mark_done());
        assert_eq!(guide.position, 2);
        assert!(!guide.mark_done());
        assert_eq!(guide.position, 0);
        assert!(guide.mark_done());
    }

    #[test]
    fn the_box_is_the_answer_on_a_question_and_a_note_elsewhere() {
        let mut guide = guide(json!([{"text": "Log in"}, {"text": "Answer", "ask": "Which email?"}]));
        guide.submit("  login   done  ");
        guide.move_by(1);
        guide.submit("a@b");
        assert_eq!((guide.notes.get(&0).cloned(), guide.answers.get(&1).cloned()), (Some("login done".into()), Some("a@b".into())));
        guide.move_by(-1);
        assert_eq!(guide.typed(), "login done");
        guide.submit("");
        assert!(guide.notes.is_empty());
    }

    #[test]
    fn hidden_characters_are_shown_escaped_and_flagged() {
        let guide = guide(json!([{"text": "Run\u{202e} this", "copy": "a\u{1b}[2Kb"}]));
        let text: String = render_step(&guide, 50).into_iter().map(|(line, _)| line + "\n").collect();
        assert!(!text.contains('\u{202e}') && !text.contains('\u{1b}'));
        assert!(text.contains("\\u202e") && text.contains("hidden characters"));
    }

    #[test]
    fn only_http_links_open() {
        let step = |value: Value| value.as_object().unwrap().clone();
        assert_eq!(link_of(&step(json!({"text": "Go to https://example.com/x."}))), "https://example.com/x.");
        assert_eq!(link_of(&step(json!({"text": "x", "open": "javascript:alert(1)"}))), "");
        assert_eq!(link_of(&step(json!({"text": "x", "open": "file:///etc/passwd"}))), "");
    }

    #[test]
    fn the_result_carries_answers_notes_runs_and_where_theo_stopped() {
        let mut guide = guide(json!([{"text": "a"}, {"text": "b", "ask": "Which email?"}, {"text": "c", "run": "make check"}]));
        guide.answers.insert(1, "a@b".into());
        guide.notes.insert(0, "slow page".into());
        let mut run = Map::new();
        run.insert("status".into(), json!("declined"));
        guide.runs.insert(2, run);
        guide.position = 1;
        let result = guide.result("stuck", "no email shown");
        assert_eq!((result["status"].as_str(), result["step"].as_i64(), result["note"].as_str()), (Some("stuck"), Some(2), Some("no email shown")));
        assert_eq!(result["answers"], json!([{"step": 2, "question": "Which email?", "answer": "a@b"}]));
        assert_eq!(result["notes"], json!([{"step": 1, "note": "slow page"}]));
        assert_eq!(result["runs"][0]["step"], 3);
    }
}
