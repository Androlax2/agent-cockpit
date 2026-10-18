//! What ask-to-run's approval terminal and a guide's run: steps share: the request files and the
//! daemon's FIFO, showing a command with its hidden characters escaped, the arrow-key menu, and
//! running a command in a real terminal through `script` with a progress line at the bottom.

use crate::paths;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use regex::Regex;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const BLUE: &str = "\x1b[34m";
pub const YELLOW: &str = "\x1b[33m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const CYAN: &str = "\x1b[36m";
pub const RESET: &str = "\x1b[0m";
const TYPE_AHEAD: Duration = Duration::from_millis(500);
/// Output stopped mid-line this long: the command waits for input (sudo, [y/N]).
const PROMPT_IDLE: Duration = Duration::from_millis(1500);

pub fn runs_dir() -> PathBuf {
    std::env::var("CLAUDE_RUNS_DIR").map(PathBuf::from).unwrap_or_else(|_| paths::home().join(".cache/claude/runs"))
}

pub fn guides_dir() -> PathBuf {
    std::env::var("CLAUDE_GUIDES_DIR").map(PathBuf::from).unwrap_or_else(|_| paths::home().join(".cache/claude/guides"))
}

/// The run log, outside the sandbox's reach (the runtime directory).
pub fn log_dir() -> PathBuf {
    paths::runtime_dir().join("claude-runs")
}

/// A request or guide id: 12 hex characters.
pub fn valid_id(id: &str) -> bool {
    id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// One line to the OSD daemon; false when no daemon reads the FIFO.
pub fn send(line: &str) -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(mut fifo) = OpenOptions::new().write(true).custom_flags(0o4000 /* O_NONBLOCK */).open(paths::osd_fifo()) else {
        return false;
    };
    fifo.write_all(format!("{line}\n").as_bytes()).is_ok()
}

/// The daemon learns whether this needs Theo (asking: the floating window comes up wherever he
/// is), runs (running: an open window stays, no new one opens), or is done (pane and window close).
pub fn phase(id: &str, name: &str) {
    send(&format!("run-phase {id} {name}"));
}

/// A line in the shared log, through the manager.
pub fn log(level: &str, message: &str) {
    let _ = Command::new("claude-sessions").args(["log", "run", level, message]).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

/// The repository a directory belongs to (its git checkout's name), else the directory's name.
pub fn project_of(directory: &str) -> String {
    let mut path = PathBuf::from(directory);
    loop {
        if path.as_os_str().is_empty() || path == Path::new("/") {
            break;
        }
        if path.join(".git").exists() {
            return path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        }
        if !path.pop() {
            break;
        }
    }
    let trimmed = directory.trim_end_matches('/');
    Path::new(trimmed).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| directory.to_string())
}

/// Written beside, then renamed over.
pub fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    fs::write(&temporary, value.to_string())?;
    fs::rename(&temporary, path)
}

/// CSI and OSC sequences, keypad-mode switches (ESC = / ESC >), carriage returns.
pub fn strip_ansi(text: &str) -> String {
    Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[=>]|\r").unwrap().replace_all(text, "").into_owned()
}

/// Characters that can hide what a command really does on screen: terminal control codes (ESC
/// sequences clear or rewrite lines, carriage returns overwrite them) and invisible or direction-
/// changing Unicode (bidi overrides, zero-width characters). Shown escaped, never interpreted.
pub fn is_hiding(character: char) -> bool {
    matches!(character as u32, 0x00..=0x08 | 0x0b..=0x1f | 0x7f..=0x9f | 0x200b..=0x200f | 0x2028..=0x202e | 0x2060..=0x2069 | 0xfeff)
}

/// Every hiding character as \uXXXX, each wrapped in `before` and `after` (colours, or nothing).
pub fn escape_hiding(text: &str, before: &str, after: &str) -> String {
    text.chars().map(|c| if is_hiding(c) { format!("{before}\\u{:04x}{after}", c as u32) } else { c.to_string() }).collect()
}

/// The command as it will run, hiding characters shown in red, and whether there was any.
pub fn visible_command(command: &str) -> (String, bool) {
    let shown = escape_hiding(command, RED, &format!("{RESET}{BOLD}"));
    let hiding = shown != command;
    (shown, hiding)
}

/// Like Python's shlex.quote: as is when it holds only safe characters, else single-quoted.
pub fn shell_quote(word: &str) -> String {
    if !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c)) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\"'\"'"))
    }
}

pub fn terminal_size() -> (usize, usize) {
    crossterm::terminal::size().map(|(columns, rows)| (columns as usize, rows as usize)).unwrap_or((100, 24))
}

/// Text wrapped at word boundaries to `width` characters.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Claude's own account of the command, escaped like the command: it says what Claude means the
/// command to do, the command above is what runs.
pub fn explanation(why: &str) -> String {
    if why.trim().is_empty() {
        return format!("{YELLOW}No explanation given: ask Claude what this does before running it.{RESET}");
    }
    let text: String = escape_hiding(why, "", "").chars().take(1000).collect();
    let columns = if io::stdout().is_terminal() { terminal_size().0 } else { 100 };
    let width = columns.saturating_sub(6).clamp(40, 100);
    let lines: Vec<String> = wrap(&text, width).into_iter().map(|line| format!("  {CYAN}{line}{RESET}")).collect();
    format!("{DIM}Why, in Claude's words (what runs is the command above):{RESET}\n{}", lines.join("\n"))
}

/// An arrow-key menu (↑↓ or j/k, Enter), starting on `default`; the chosen option's key. Without
/// a terminal (tests), a line of input: an option key, or empty for the default.
pub fn choose(question: &str, options: &[(&str, &str)], default: &str) -> String {
    let keys: Vec<&str> = options.iter().map(|(key, _)| *key).collect();
    if !io::stdin().is_terminal() {
        print!("{question} [{}] ", keys.join("/"));
        let _ = io::stdout().flush();
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
            return default.to_string();
        }
        let answer: String = line.trim().to_lowercase().chars().take(1).collect();
        return if keys.contains(&answer.as_str()) { answer } else { default.to_string() };
    }
    let mut selected = keys.iter().position(|key| *key == default).unwrap_or(0);
    println!("{BOLD}{question}{RESET}  {DIM}↑↓ choose · Enter confirms{RESET}");
    let draw = |selected: usize, first: bool| {
        let mut out = io::stdout();
        if !first {
            let _ = write!(out, "\x1b[{}A", options.len());
        }
        for (index, (_, label)) in options.iter().enumerate() {
            let marker = if index == selected { format!("{BLUE}❯ {BOLD}{label}{RESET}") } else { format!("  {DIM}{label}{RESET}") };
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
        let Ok(Event::Key(key)) = event::read() else { continue };
        if key.kind == KeyEventKind::Release || shown_at.elapsed() < TYPE_AHEAD {
            continue;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => selected = (selected + options.len() - 1) % options.len(),
            KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1) % options.len(),
            KeyCode::Enter => break keys[selected].to_string(),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break default.to_string(),
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
    let edited = fs::read_to_string(&path).ok().map(|text| text.trim().to_string());
    let _ = fs::remove_file(&path);
    edited.filter(|text| !text.is_empty())
}

/// The recorded output ends without a newline and has not grown for PROMPT_IDLE: a prompt.
pub fn waits_for_input(log_path: &Path, idle: Duration) -> bool {
    let Ok(bytes) = fs::read(log_path) else { return false };
    idle >= PROMPT_IDLE && bytes.last().is_some_and(|last| *last != b'\n')
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
pub fn run(command: &str, cwd: &str, log_path: &Path) -> i32 {
    let _ = fs::create_dir_all(log_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(log_dir(), fs::Permissions::from_mode(0o700));
    }
    let inner = ["fish", "-c", command].iter().map(|word| shell_quote(word)).collect::<Vec<_>>().join(" ");
    let mut child = match Command::new("script").args(["-qfec", &inner]).arg(log_path).current_dir(cwd).spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("{RED}cannot run script: {error}{RESET}");
            return 127;
        }
    };
    if !io::stdout().is_terminal() {
        return child.wait().ok().and_then(|status| status.code()).unwrap_or(1);
    }
    let mut rows = terminal_size().1;
    let started = Instant::now();
    let spinner: Vec<char> = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".chars().collect();
    print!("{}", reserve_last_row(rows));
    let (mut tick, mut size, mut changed_at, mut waiting) = (0usize, u64::MAX, Instant::now(), false);
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
                print!("{}", reserve_last_row(rows));
                draw_status(rows, &format!("{YELLOW}waiting for your answer above{RESET}"));
            }
        } else {
            waiting = false;
            let elapsed = started.elapsed().as_secs();
            draw_status(rows, &format!("{BLUE}{}{RESET} {DIM}running · {}m {:02}s{RESET}", spinner[tick % spinner.len()], elapsed / 60, elapsed % 60));
            tick += 1;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    print!("\x1b7\x1b[{rows};1H\x1b[2K\x1b8\x1b[r\x1b[{};1H\n", rows - 1);
    let _ = io::stdout().flush();
    code
}

/// What `script` recorded, without its "Script started/done" lines.
pub fn recorded_output(log_path: &Path) -> String {
    let Ok(bytes) = fs::read(log_path) else { return String::new() };
    let text = String::from_utf8_lossy(&bytes);
    let banner = Regex::new(r"(?m)^Script (started|done) on .*$").unwrap();
    banner.replace_all(&text, "").trim_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_matches_shlex() {
        assert_eq!(shell_quote("fish"), "fish");
        assert_eq!(shell_quote("echo hi"), "'echo hi'");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
    }

    #[test]
    fn output_stopped_mid_line_reads_as_a_prompt() {
        let log = std::env::temp_dir().join(format!("prompt-{}.log", crate::util::token_hex(4)));
        fs::write(&log, "[sudo] password for theo: ").unwrap();
        assert!(waits_for_input(&log, Duration::from_secs(2)));
        assert!(!waits_for_input(&log, Duration::from_millis(500)));
        fs::write(&log, "built 12 targets\n").unwrap();
        assert!(!waits_for_input(&log, Duration::from_secs(5)));
        let _ = fs::remove_file(log);
    }

    #[test]
    fn hidden_characters_are_escaped() {
        let (shown, hiding) = visible_command("ls\x1b[2K\r; rm \u{202e}");
        assert!(hiding);
        assert!(shown.contains("\\u001b") && shown.contains("\\u202e") && !shown.contains('\u{202e}'));
    }
}
