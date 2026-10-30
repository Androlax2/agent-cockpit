//! Terminal-only approval presentation and PTY execution adapters.

use super::*;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const TYPE_AHEAD: Duration = Duration::from_millis(500);
const REMOTE_POLL: Duration = Duration::from_millis(250);

pub fn terminal_size() -> (usize, usize) {
    crossterm::terminal::size()
        .map(|(columns, rows)| (columns as usize, rows as usize))
        .unwrap_or((100, 24))
}

/// Claude's own account of the command, escaped like the command: it says what Claude means the
/// command to do, the command above is what runs.
pub fn explanation(why: &str) -> String {
    if why.trim().is_empty() {
        return format!(
            "{YELLOW}No explanation given: ask Claude what this does before running it.{RESET}"
        );
    }
    let text: String = escape_hiding(why, "", "").chars().take(1000).collect();
    let columns = if io::stdout().is_terminal() {
        terminal_size().0
    } else {
        100
    };
    let width = columns.saturating_sub(6).clamp(40, 100);
    let lines: Vec<String> = wrap(&text, width)
        .into_iter()
        .map(|line| format!("  {CYAN}{line}{RESET}"))
        .collect();
    format!(
        "{DIM}Why, in Claude's words (what runs is the command above):{RESET}\n{}",
        lines.join("\n")
    )
}

/// An arrow-key menu (↑↓ or j/k, Enter), starting on `default`; the chosen option's key. Without
/// a terminal (tests), a line of input: an option key, or empty for the default.
pub fn choose(question: &str, options: &[(&str, &str)], default: &str) -> String {
    choose_or_remote(question, options, default, None)
}

/// choose, also answered by the phone: while the menu waits, an answer in `remote_answer` picks the
/// option of that key (only one of `options`), as if typed here.
pub fn choose_or_remote(
    question: &str,
    options: &[(&str, &str)],
    default: &str,
    remote_answer: Option<&Path>,
) -> String {
    let keys: Vec<&str> = options.iter().map(|(key, _)| *key).collect();
    let remote_pick = || -> Option<String> {
        let path = remote_answer?;
        let answer = fs::read_to_string(path).ok()?.trim().to_string();
        let _ = fs::remove_file(path);
        keys.contains(&answer.as_str()).then_some(answer)
    };
    if !io::stdin().is_terminal() {
        print!("{question} [{}] ", keys.join("/"));
        let _ = io::stdout().flush();
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
            return default.to_string();
        }
        let answer: String = line.trim().to_lowercase().chars().take(1).collect();
        return if keys.contains(&answer.as_str()) {
            answer
        } else {
            default.to_string()
        };
    }
    let mut selected = keys.iter().position(|key| *key == default).unwrap_or(0);
    println!("{BOLD}{question}{RESET}  {DIM}↑↓ choose · Enter confirms{RESET}");
    let draw = |selected: usize, first: bool| {
        let mut out = io::stdout();
        if !first {
            let _ = write!(out, "\x1b[{}A", options.len());
        }
        for (index, (_, label)) in options.iter().enumerate() {
            let marker = if index == selected {
                format!("{BLUE}❯ {BOLD}{label}{RESET}")
            } else {
                format!("  {DIM}{label}{RESET}")
            };
            let _ = write!(out, "\r\x1b[2K{marker}\r\n");
        }
        let _ = out.flush();
    };
    let _ = crossterm::terminal::enable_raw_mode();
    draw(selected, true);
    // A new window takes the keyboard: keys typed for something else in the moment it appears must
    // not answer. Drop what is pending, and anything typed in the first half second.
    while event::poll(Duration::ZERO).unwrap_or(false) {
        let _ = event::read();
    }
    let shown_at = Instant::now();
    let answer = loop {
        if let Some(answer) = remote_pick() {
            break answer;
        }
        if !event::poll(REMOTE_POLL).unwrap_or(false) {
            continue;
        }
        let Ok(Event::Key(key)) = event::read() else {
            continue;
        };
        if key.kind == KeyEventKind::Release || shown_at.elapsed() < TYPE_AHEAD {
            continue;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                selected = (selected + options.len() - 1) % options.len()
            }
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1) % options.len(),
            KeyCode::Enter => break keys[selected].to_string(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                break default.to_string();
            }
            KeyCode::Esc | KeyCode::Char('q') => break default.to_string(),
            _ => {}
        }
        draw(selected, false);
    };
    let _ = crossterm::terminal::disable_raw_mode();
    answer
}

/// The command opened in $EDITOR (nano without one); the edited text, or None when emptied.
pub fn edit(command: &str) -> Option<String> {
    let path = std::env::temp_dir().join(format!("claude-run-{}.fish", crate::util::token_hex(6)));
    fs::write(&path, format!("{command}\n")).ok()?;
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".into());
    let _ = Command::new(editor).arg(&path).status();
    let edited = fs::read_to_string(&path)
        .ok()
        .map(|text| text.trim().to_string());
    let _ = fs::remove_file(&path);
    edited.filter(|text| !text.is_empty())
}

/// Keep the last row out of the scroll region, so command output never lands under the status
/// line. IND (ESC D) first scrolls when the cursor sits on the last row, and keeps the column: a
/// half-printed prompt moves up whole instead of being cut or overwritten.
pub fn reserve_last_row(rows: usize) -> String {
    format!("\x1bD\x1b7\x1b[1;{}r\x1b8\x1b[A", rows - 1)
}

fn draw_status(rows: usize, text: &str) {
    let mut out = io::stdout();
    let _ = write!(out, "\x1b7\x1b[{rows};1H\x1b[2K{text}\x1b8");
    let _ = out.flush();
}

/// The command in a real terminal through `script` (sudo and prompts work), recorded, with a
/// "running · 12s" line kept at the bottom: the lines above it scroll, so output still shows, and
/// a command that prints nothing no longer looks stuck. While the command waits for an answer,
/// the line stops redrawing and says so once, below the question.
/// `request`: the approval's id, to tell the daemon when the command starts and stops waiting
/// for an answer (phases "input" and "running"), so the window can come back for it.
pub fn run(command: &str, cwd: &str, log_path: &Path, request: Option<&str>) -> i32 {
    let _ = fs::create_dir_all(log_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(log_dir(), fs::Permissions::from_mode(0o700));
    }
    // fish traces each command it runs to a file of its own (not the output): the last top-level one
    // names the step a chain stopped at (last_step). -C turns the trace on after config.fish and
    // before the command, as a global left unexported, so the programs it starts are not traced.
    let trace = trace_path(log_path);
    let _ = fs::remove_file(&trace);
    let debug_output = format!("--debug-output={}", trace.display());
    let inner = [
        "fish",
        &debug_output,
        "-C",
        "set -g fish_trace 1",
        "-c",
        command,
    ]
    .iter()
    .map(|word| shell_quote(word))
    .collect::<Vec<_>>()
    .join(" ");
    // script hands `inner` to $SHELL, and the quoting above is sh's: under fish (Theo's login
    // shell) a backslash inside single quotes is an escape, so the command shown and the command
    // run could differ.
    let mut command_line = Command::new("script");
    command_line.args(["-qfec", &inner]).arg(log_path).env("SHELL", "/bin/sh")
        .env("COCKPIT_APPROVAL_ID", request.unwrap_or("guide"))
        // A progress bar redraws one line without ending it, which reads as a command waiting for
        // an answer (waits_for_input): builds print plain lines here.
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        // A pager's full screen ends on its ":" prompt, which reads as a question, and the status
        // line drawn under it left the window blank (apply-dotfiles showed its patches in less).
        .env("PAGER", "cat");
    // The approval runs in a tmux session the daemon starts, whose server may predate Hyprland's
    // last start or lack its signature: hyprctl in an approved command would find no Hyprland.
    if let Some(instance) = crate::util::running_hyprland_instance() {
        command_line.env("HYPRLAND_INSTANCE_SIGNATURE", instance);
    }
    if !cwd.is_empty() {
        command_line.current_dir(cwd);
    }
    let mut child = match command_line.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("{RED}cannot run script: {error}{RESET}");
            return 127;
        }
    };
    if !io::stdout().is_terminal() {
        return child
            .wait()
            .ok()
            .and_then(|status| status.code())
            .unwrap_or(1);
    }
    let mut rows = terminal_size().1;
    let started = Instant::now();
    let spinner: Vec<char> = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".chars().collect();
    print!("{}", reserve_last_row(rows));
    let (mut tick, mut size, mut changed_at, mut waiting) =
        (0usize, u64::MAX, Instant::now(), false);
    let code = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break status.code().unwrap_or(1);
        }
        // A resize (the floating window attaching to the tmux session) resets the scroll region.
        let now_rows = terminal_size().1;
        if now_rows != rows {
            rows = now_rows;
            waiting = false;
            print!("{}", reserve_last_row(rows));
        }
        let now_size = fs::metadata(log_path).map(|m| m.len()).unwrap_or(0);
        if now_size != size {
            size = now_size;
            changed_at = Instant::now();
        }
        if waits_for_input(log_path, changed_at.elapsed()) {
            if !waiting {
                waiting = true;
                if let Some(id) = request {
                    phase(id, "input");
                    if let Some(prompt) = prompt_line(log_path) {
                        let _ = write_json(
                            &remote_input(id),
                            &serde_json::json!({"id": id, "command": command, "prompt": prompt.trim()}),
                        );
                    }
                }
                print!("{}", reserve_last_row(rows));
                draw_status(
                    rows,
                    &format!("{YELLOW}waiting for your answer above{RESET}"),
                );
            }
        } else {
            if waiting && let Some(id) = request {
                let _ = fs::remove_file(remote_input(id));
                phase(id, "running");
            }
            waiting = false;
            let elapsed = started.elapsed().as_secs();
            draw_status(
                rows,
                &format!(
                    "{BLUE}{}{RESET} {DIM}running · {}m {:02}s{RESET}",
                    spinner[tick % spinner.len()],
                    elapsed / 60,
                    elapsed % 60
                ),
            );
            tick += 1;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if let Some(id) = request {
        let _ = fs::remove_file(remote_input(id));
    }
    println!("\x1b7\x1b[{rows};1H\x1b[2K\x1b8\x1b[r\x1b[{};1H", rows - 1);
    // Whatever modes the output left on (no line wrap, a line-drawing charset, hidden text) must
    // not change how the next command or question reads.
    print!("{}", TERMINAL_RESET);
    let _ = io::stdout().flush();
    code
}
