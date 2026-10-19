//! What ask-to-run's approval terminal and a guide's run: steps share: the request files and the
//! daemon's FIFO, showing a command with its hidden characters escaped, the arrow-key menu, and
//! running a command in a real terminal through `script` with a progress line at the bottom.

use crate::paths;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use regex::Regex;
use std::sync::LazyLock;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, IsTerminal, Read, Write};
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
const REQUEST_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// A session started with "this terminal" (the claude function's t sets CLAUDE_SESSIONS_TERMINAL):
/// Theo wanted it apart from the cockpit, so ask-to-run and ask-to-guide stay out of it and Claude
/// falls back to giving him the command or the steps.
pub fn in_terminal_session() -> bool {
    std::env::var("CLAUDE_SESSIONS_TERMINAL").is_ok_and(|value| value == "1")
}

pub fn runs_dir() -> PathBuf {
    std::env::var("CLAUDE_RUNS_DIR").map(PathBuf::from).unwrap_or_else(|_| paths::home().join(".cache/claude/runs"))
}

/// Where Theo's answers go: the results of runs and guides, and a guide's progress. Every sandbox
/// writes the request folders above, so an answer there could be forged for another session's
/// request; these folders only the programs outside the sandbox write. One per project (`key`, the
/// launcher's CLAUDE_ANSWERS_KEY), in the runtime directory no sandbox sees: the launcher mounts a
/// project's own folder, read-only, into its sandboxes and no other, so no session reads another
/// project's command output (security audit 2026-10-19, M3). `kind` is "runs" or "guides".
pub fn answers_dir(key: &str, kind: &str) -> PathBuf {
    std::env::var("CLAUDE_ANSWERS_DIR").map(PathBuf::from).unwrap_or_else(|_| paths::runtime_dir().join("claude-answers")).join(key).join(kind)
}

/// A project's answers folder name, as the launcher makes it: letters, digits and "-" only, so it
/// never leaves the answers folder.
pub fn valid_answers_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 255 && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// This sandbox's answers folder name, from the launcher; None in a session started before answers
/// went per project.
pub fn own_answers_key() -> Option<String> {
    std::env::var("CLAUDE_ANSWERS_KEY").ok().filter(|key| valid_answers_key(key))
}

/// The answers folder a run or guide request names (its "answers" field). The sandbox chose it:
/// naming another project's folder only sends its own answer where it cannot read it.
pub fn request_answers_dir(request: &Value, kind: &str) -> Option<PathBuf> {
    request.get("answers").and_then(Value::as_str).filter(|key| valid_answers_key(key)).map(|key| answers_dir(key, kind))
}

/// What ask-to-run and ask-to-guide say in a session the launcher gave no answers folder.
pub const NO_ANSWERS_KEY: &str = "This session started before Theo's answers went per project (no CLAUDE_ANSWERS_KEY): \
                                  ask Theo to restart it (r in the column).";

/// The run log, outside the sandbox's reach (the runtime directory).
pub fn log_dir() -> PathBuf {
    paths::runtime_dir().join("claude-runs")
}

/// A request or guide id: 12 hex characters.
pub fn valid_id(id: &str) -> bool {
    id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn send_to(path: &Path, line: &str) -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(mut fifo) = OpenOptions::new().write(true).custom_flags(0o4000 /* O_NONBLOCK */).open(path) else {
        return false;
    };
    fifo.write_all(format!("{line}\n").as_bytes()).is_ok()
}

/// Why the daemon did not take a request.
pub enum Refusal {
    /// Nothing answers on the report socket (osd.service down?).
    NoDaemon,
    /// The daemon read it and said why not.
    Refused(String),
}

/// A run or guide request handed to the OSD daemon from inside a sandbox, whole, on the report
/// socket (`kind` is "run-request" or "guide-request"). There the kernel names the sender, and the
/// daemon keeps its own copy at once, where no sandbox reaches: no other session can rewrite it
/// before Theo reads it (security audit 2026-10-19, H3).
pub fn file_request(kind: &str, request: &Value) -> Result<(), Refusal> {
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(paths::osd_report_socket()).map_err(|_| Refusal::NoDaemon)?;
    let _ = stream.set_read_timeout(Some(REQUEST_REPLY_TIMEOUT));
    stream.write_all(format!("{kind} {request}\n").as_bytes()).map_err(|_| Refusal::NoDaemon)?;
    let _ = stream.shutdown(Shutdown::Write);
    let mut reply = String::new();
    let _ = (&mut stream).take(4096).read_to_string(&mut reply);
    match reply.trim() {
        "ok" => Ok(()),
        "" => Err(Refusal::NoDaemon),
        other => Err(Refusal::Refused(other.strip_prefix("refused: ").unwrap_or(other).to_string())),
    }
}

/// The daemon's copy of a run request (`extension` "request") or a guide ("guide"), in the
/// runtime directory no sandbox sees: what the approval and the guide window read.
pub fn held_request(id: &str, extension: &str) -> PathBuf {
    paths::runtime_dir().join("claude-requests").join(format!("{id}.{extension}"))
}

/// A command only programs outside the sandbox may give (phases, banners, notice-clear).
pub fn send_host(line: &str) -> bool {
    send_to(&paths::osd_host_fifo(), line)
}

/// The daemon learns whether this needs Theo (asking: the floating window comes up wherever he
/// is), runs (running: an open window stays, no new one opens), or is done (pane and window close).
pub fn phase(id: &str, name: &str) {
    send_host(&format!("run-phase {id} {name}"));
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

/// Written beside, then renamed over, without following a link the sandbox planted (util::write_json).
pub fn write_json(path: &Path, value: &Value) -> io::Result<()> {
    crate::util::write_json(path, value)
}

/// CSI and OSC sequences, keypad-mode switches (ESC = / ESC >), carriage returns.
pub fn strip_ansi(text: &str) -> String {
    Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[=>]|\r").unwrap().replace_all(text, "").into_owned()
}

/// Characters that can hide what a command really does on screen: terminal control codes (ESC
/// sequences clear or rewrite lines, carriage returns overwrite them) and invisible or direction-
/// changing Unicode (bidi overrides, zero-width characters). Shown escaped, never interpreted.
pub fn is_hiding(character: char) -> bool {
    matches!(character as u32, 0x00..=0x09 | 0x0b..=0x1f | 0x7f..=0x9f | 0xad | 0x34f | 0x115f | 0x1160 | 0x200b..=0x200f | 0x2028..=0x202e
                               | 0x2060..=0x2069 | 0x2800 | 0x3164 | 0xfe00..=0xfe0f | 0xfeff | 0xffa0 | 0xe0000..=0xe007f)
}

/// Blank lines in a row are where a payload hides: enough of them scroll the start of a command
/// off the window. Each run of two or more shows as one marker line instead.
fn mark_blank_runs(text: &str, before: &str, after: &str) -> String {
    let mut shown = Vec::new();
    let mut blanks = 0;
    for line in text.split('\n').chain(std::iter::once("\u{0}end")) {
        if line.trim().is_empty() {
            blanks += 1;
            continue;
        }
        match blanks {
            0 => {}
            1 => shown.push(String::new()),
            many => shown.push(format!("{before}[{many} blank lines]{after}")),
        }
        blanks = 0;
        if line != "\u{0}end" {
            shown.push(line.to_string());
        }
    }
    shown.join("\n")
}

/// Every hiding character as \uXXXX, each wrapped in `before` and `after` (colours, or nothing).
pub fn escape_hiding(text: &str, before: &str, after: &str) -> String {
    text.chars().map(|c| if is_hiding(c) { format!("{before}\\u{:04x}{after}", c as u32) } else { c.to_string() }).collect()
}

/// The command as it will run, hiding characters shown in red, and whether there was any.
pub fn visible_command(command: &str) -> (String, bool) {
    let escaped = escape_hiding(command, RED, &format!("{RESET}{BOLD}"));
    let shown = mark_blank_runs(&escaped, RED, &format!("{RESET}{BOLD}"));
    let hiding = shown != command;
    (shown, hiding)
}

/// Characters outside ASCII in a command, as \uXXXX: lookalike letters (a Cyrillic а for an a) make
/// a command read as something else. Named, not hidden: French text in a command is legitimate.
pub fn non_ascii(command: &str) -> Vec<String> {
    let mut found: Vec<String> = command.chars().filter(|c| !c.is_ascii() && !is_hiding(*c)).map(|c| format!("{c} \\u{:04x}", c as u32)).collect();
    found.sort();
    found.dedup();
    found
}

/// The directory a command will run in, resolved once: symlinks followed now, so what Theo reads is
/// where it runs (the process changes into it before he answers, see enter_directory).
pub fn resolved_directory(cwd: &str) -> Option<String> {
    fs::canonicalize(cwd).ok().filter(|path| path.is_dir()).map(|path| path.to_string_lossy().into_owned())
}

/// Moves this process into the directory before the command is shown: a symlink swapped in after
/// Theo read the path no longer changes where the command runs.
pub fn enter_directory(directory: &str) -> io::Result<()> {
    std::env::set_current_dir(directory)
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

/// What a question looks like at the end of a line: a password or passphrase, a yes/no choice,
/// or a line ending in ":", "?" or ">". Progress output (test dots, a half-drawn bar) stops
/// mid-line too, and is none of these.
static PROMPT_SHAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
    r"(?i)(password|passphrase|\[y/n\]|\(y/n\)|yes/no|[:?>]\s*$)").unwrap());

/// The recorded output ends mid-line on something shaped like a question, and has not grown for
/// PROMPT_IDLE: a prompt.
pub fn waits_for_input(log_path: &Path, idle: Duration) -> bool {
    if idle < PROMPT_IDLE {
        return false;
    }
    let Ok(bytes) = fs::read(log_path) else { return false };
    if bytes.last().is_none_or(|last| *last == b'\n') {
        return false;
    }
    let tail = &bytes[bytes.len().saturating_sub(400)..];
    let text = strip_ansi(&String::from_utf8_lossy(tail));
    let last_line = text.rsplit(['\n', '\r']).next().unwrap_or("");
    PROMPT_SHAPE.is_match(last_line)
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
    let inner = ["fish", &debug_output, "-C", "set -g fish_trace 1", "-c", command].iter()
        .map(|word| shell_quote(word)).collect::<Vec<_>>().join(" ");
    // script hands `inner` to $SHELL, and the quoting above is sh's: under fish (Theo's login
    // shell) a backslash inside single quotes is an escape, so the command shown and the command
    // run could differ.
    let mut command_line = Command::new("script");
    command_line.args(["-qfec", &inner]).arg(log_path).env("SHELL", "/bin/sh")
        // A progress bar redraws one line without ending it, which reads as a command waiting for
        // an answer (waits_for_input): builds print plain lines here.
        .env("CARGO_TERM_PROGRESS_WHEN", "never")
        // A pager's full screen ends on its ":" prompt, which reads as a question, and the status
        // line drawn under it left the window blank (apply-dotfiles showed its patches in less).
        .env("PAGER", "cat");
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
                if let Some(id) = request {
                    phase(id, "input");
                }
                print!("{}", reserve_last_row(rows));
                draw_status(rows, &format!("{YELLOW}waiting for your answer above{RESET}"));
            }
        } else {
            if waiting {
                if let Some(id) = request {
                    phase(id, "running");
                }
            }
            waiting = false;
            let elapsed = started.elapsed().as_secs();
            draw_status(rows, &format!("{BLUE}{}{RESET} {DIM}running · {}m {:02}s{RESET}", spinner[tick % spinner.len()], elapsed / 60, elapsed % 60));
            tick += 1;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    print!("\x1b7\x1b[{rows};1H\x1b[2K\x1b8\x1b[r\x1b[{};1H\n", rows - 1);
    // Whatever modes the output left on (no line wrap, a line-drawing charset, hidden text) must
    // not change how the next command or question reads.
    print!("{}", TERMINAL_RESET);
    let _ = io::stdout().flush();
    code
}

/// Where run() leaves fish's trace of a command whose output goes to `log_path`.
pub fn trace_path(log_path: &Path) -> PathBuf {
    log_path.with_extension("trace")
}

/// The last top-level command fish ran, from its trace: with `a; and b; and c`, the step that
/// failed when the whole failed. Nested lines (functions, blocks) start with more dashes.
pub fn last_step(trace: &str) -> Option<String> {
    trace.lines().filter_map(|line| line.strip_prefix("> ")).filter(|step| !step.starts_with("end ")).last()
        .map(|step| step.chars().take(200).collect())
}

/// Soft reset, line wrap on, the ASCII charset, plain attributes.
pub const TERMINAL_RESET: &str = "\x1b[!p\x1b[?7h\x1b(B\x1b[0m";

/// What `script` recorded, without its "Script started/done" lines.
pub fn recorded_output(log_path: &Path) -> String {
    let Ok(bytes) = fs::read(log_path) else { return String::new() };
    let text = String::from_utf8_lossy(&bytes);
    let banner = Regex::new(r"(?m)^Script (started|done) on .*$").unwrap();
    banner.replace_all(&text, "").trim_matches('\n').to_string()
}

/// What a command's output must not carry back into a conversation: tokens and keys by their
/// shape, private key blocks, credentials in headers and URLs, and the value of anything named like
/// a password, secret, token or key. Matched values become [redacted]; names and the rest stay, so
/// Claude still sees what happened.
static SECRET_SHAPES: LazyLock<Vec<Regex>> = LazyLock::new(|| [
    r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----[\s\S]*?(?:-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)",
    r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}",
    r"\bwhsec_[A-Za-z0-9+/=]{16,}",
    r"\bya29\.[A-Za-z0-9_-]{20,}",
    r"https://hooks\.slack\.com/services/[A-Za-z0-9/]+",
    r"\bgh[pousr]_[A-Za-z0-9]{30,}\b",
    r"\bgithub_pat_[A-Za-z0-9_]{20,}\b",
    r"\bsk-ant-[A-Za-z0-9_-]{20,}",
    r"\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}",
    r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
    r"\bxox[abprs]-[A-Za-z0-9-]{10,}",
    r"\bAIza[0-9A-Za-z_-]{35}\b",
    r"\bglpat-[A-Za-z0-9_-]{20,}",
    r"\bnpm_[A-Za-z0-9]{36}\b",
    r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
].iter().map(|pattern| Regex::new(pattern).unwrap()).collect());
/// The value after a name that says it is secret, or after an Authorization scheme, or the
/// password in scheme://user:password@host.
static SECRET_VALUES: LazyLock<Vec<Regex>> = LazyLock::new(|| [
    r#"(?i)((?:authorization|proxy-authorization)\s*:\s*(?:bearer|basic|token)\s+)[^\s"']+"#,
    r#"(?i)((?:set-)?cookie\s*:\s*)[^\n]+"#,
    // JSON: a quoted secret value may hold spaces ("token": "…", Docker's "auth": "…").
    r#"(?i)("[a-z0-9_.-]*(?:password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?|auth)[a-z0-9_.-]*"\s*:\s*")[^"]+(")"#,
    r#"(?i)(\b[a-z0-9_.-]*(?:password|passwd|passphrase|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?)[a-z0-9_.-]*\b["']?\s*[:=]\s*["']?)[^\s"']+"#,
    // .netrc and .pgpass style: the keyword, a space, the value.
    r"(?im)(\b(?:password|passwd)\s+)[^\s=:]\S*",
    r"(?i)([a-z][a-z0-9+.-]*://[^\s:/@]+:)[^\s@/]+(@)",
].iter().map(|pattern| Regex::new(pattern).unwrap()).collect());

/// The output with every secret-looking value replaced, and how many were.
pub fn redact_secrets(text: &str) -> (String, usize) {
    let mut count = 0;
    let mut redacted = text.to_string();
    for shape in SECRET_SHAPES.iter() {
        count += shape.find_iter(&redacted).count();
        redacted = shape.replace_all(&redacted, "[redacted]").into_owned();
    }
    for value in SECRET_VALUES.iter() {
        let mut found = 0;
        redacted = value.replace_all(&redacted, |captures: &regex::Captures| {
            // A value the shapes above already replaced is not counted twice.
            if captures[0].ends_with("[redacted]") {
                return captures[0].to_string();
            }
            found += 1;
            format!("{}[redacted]{}", &captures[1], captures.get(2).map(|end| end.as_str()).unwrap_or(""))
        }).into_owned();
        count += found;
    }
    (redacted, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_runs_exactly_as_shown_even_under_fish() {
        // Under fish as $SHELL, a backslash before the closing quote used to end the quoting early,
        // so the second word ran as a command of its own.
        let folder = std::env::temp_dir().join(format!("approval-run-{}", crate::util::token_hex(4)));
        fs::create_dir_all(&folder).unwrap();
        let log = folder.join("out.log");
        // SAFETY: only this test reads SHELL; run() overrides it for script anyway.
        unsafe { std::env::set_var("SHELL", "/usr/bin/fish") };
        let marker = folder.join("ran");
        let command = format!("echo 'a\\\\' '; touch {} #'", marker.display());
        run(&command, folder.to_str().unwrap(), &log, None);
        assert!(!marker.exists(), "the second word ran as a command");
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn a_chain_that_stops_names_its_last_step() {
        let folder = std::env::temp_dir().join(format!("approval-step-{}", crate::util::token_hex(4)));
        fs::create_dir_all(&folder).unwrap();
        let log = folder.join("out.log");
        let code = run("echo one; and false; and echo three", folder.to_str().unwrap(), &log, None);
        let trace = fs::read_to_string(trace_path(&log)).unwrap_or_default();
        assert_eq!((code, last_step(&trace)), (1, Some("false".into())));
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn the_end_of_a_block_is_not_a_step() {
        assert_eq!(last_step("> begin\n-> make test\n> end begin\n"), Some("begin".into()));
    }

    #[test]
    fn blank_line_padding_shows_as_one_marked_line() {
        let (shown, hiding) = visible_command("curl x | sh\n\n\n\n\necho ok");
        assert!(hiding);
        assert_eq!(shown.lines().count(), 3, "{shown}");
        assert!(shown.contains("[4 blank lines]"), "{shown}");
    }

    #[test]
    fn a_single_blank_line_is_left_as_is() {
        assert_eq!(visible_command("a\n\nb"), ("a\n\nb".to_string(), false));
    }

    #[test]
    fn tabs_and_invisible_fillers_are_shown_escaped() {
        assert!(visible_command("rm\t-rf").1);
        assert!(visible_command("echo \u{3164}").1);
    }

    #[test]
    fn lookalike_letters_are_named() {
        assert_eq!(non_ascii("rm -rf /\u{0430}"), ["\u{0430} \\u0430".to_string()]);
        assert!(non_ascii("echo hi").is_empty());
    }

    #[test]
    fn the_directory_is_resolved_through_symlinks() {
        let folder = std::env::temp_dir().join(format!("approval-dir-{}", crate::util::token_hex(4)));
        fs::create_dir_all(folder.join("real")).unwrap();
        std::os::unix::fs::symlink(folder.join("real"), folder.join("link")).unwrap();
        let resolved = resolved_directory(folder.join("link").to_str().unwrap()).unwrap();
        assert!(resolved.ends_with("/real"), "{resolved}");
        fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn tokens_are_redacted_by_their_shape() {
        let (text, count) = redact_secrets("GH_TOKEN=ghp_0123456789abcdefghijABCDEFGHIJ012345 and sk-ant-api03-abcdefghijklmnopqrstuvwxyz");
        assert_eq!(count, 2, "{text}");
        assert!(!text.contains("ghp_0123") && !text.contains("sk-ant-api03"), "{text}");
    }

    #[test]
    fn a_private_key_block_is_redacted_whole() {
        let (text, count) = redact_secrets("before\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----\nafter");
        assert_eq!((text.as_str(), count), ("before\n[redacted]\nafter", 1));
    }

    #[test]
    fn named_secrets_keep_their_name() {
        let (text, count) = redact_secrets("DB_PASSWORD=hunter2hunter\napi_key: \"abcd1234efgh\"\nport=5432");
        assert_eq!(count, 2, "{text}");
        assert!(text.contains("DB_PASSWORD=[redacted]") && text.contains("port=5432"), "{text}");
    }

    #[test]
    fn credentials_in_headers_and_urls_are_redacted() {
        let (text, count) = redact_secrets("Authorization: Bearer abc.def.ghi\nhttps://theo:s3cret@example.com/repo.git");
        assert_eq!(count, 2, "{text}");
        assert!(text.contains("Authorization: Bearer [redacted]") && text.contains("https://theo:[redacted]@example.com"), "{text}");
    }

    #[test]
    fn secrets_in_other_common_formats_are_redacted() {
        for (text, leaked) in [
            ("machine github.com login theo password hunter2hunter", "hunter2"),
            (r#"{"auths": {"ghcr.io": {"auth": "dGhlbzpzZWNyZXQ="}}}"#, "dGhlbzpz"),
            (r#"{"token": "two words secret"}"#, "two words"),
            ("Cookie: session=abcdef123456; theme=dark", "abcdef123456"),
            ("STRIPE=sk_live_0123456789abcdefABCD", "sk_live_0123"),
            ("password=ab;cdef", "ab;cdef"),
            ("-----BEGIN PGP PRIVATE KEY BLOCK-----\nlQOYBF\n-----END PGP PRIVATE KEY BLOCK-----", "lQOYBF"),
        ] {
            let (redacted, count) = redact_secrets(text);
            assert!(count > 0 && !redacted.contains(leaked), "{text} -> {redacted}");
        }
    }

    #[test]
    fn ordinary_output_is_left_alone() {
        let output = "Compiling cockpit v0.1.0\ntest result: ok. 91 passed\nmonkey token count: 3 words";
        assert_eq!(redact_secrets(output), (output.to_string(), 0));
    }

    #[test]
    fn quoting_matches_shlex() {
        assert_eq!(shell_quote("fish"), "fish");
        assert_eq!(shell_quote("echo hi"), "'echo hi'");
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
    }

    #[test]
    fn test_progress_stopped_mid_line_is_no_prompt() {
        let log = std::env::temp_dir().join(format!("progress-{}.log", crate::util::token_hex(4)));
        fs::write(&log, "running 3 tests\n.").unwrap();
        assert!(!waits_for_input(&log, Duration::from_secs(10)));
        fs::write(&log, "   Compiling cockpit v0.1.0\r  Building [=====>    ] 120/240").unwrap();
        assert!(!waits_for_input(&log, Duration::from_secs(10)));
        let _ = fs::remove_file(log);
    }

    #[test]
    fn output_stopped_mid_line_reads_as_a_prompt() {
        let log = std::env::temp_dir().join(format!("prompt-{}.log", crate::util::token_hex(4)));
        fs::write(&log, "[sudo] password for theo: ").unwrap();
        assert!(waits_for_input(&log, Duration::from_secs(2)));
        assert!(!waits_for_input(&log, Duration::from_millis(500)));
        fs::write(&log, "built 12 targets\n").unwrap();
        assert!(!waits_for_input(&log, Duration::from_secs(5)));
        fs::write(&log, "Enter passphrase for key '/home/theo/.ssh/id_ed25519': ").unwrap();
        assert!(waits_for_input(&log, Duration::from_secs(2)));
        fs::write(&log, ":: Proceed with installation? [Y/n] ").unwrap();
        assert!(waits_for_input(&log, Duration::from_secs(2)));
        let _ = fs::remove_file(log);
    }

    #[test]
    fn hidden_characters_are_escaped() {
        let (shown, hiding) = visible_command("ls\x1b[2K\r; rm \u{202e}");
        assert!(hiding);
        assert!(shown.contains("\\u001b") && shown.contains("\\u202e") && !shown.contains('\u{202e}'));
    }
}
