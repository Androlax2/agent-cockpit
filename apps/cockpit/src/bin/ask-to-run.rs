//! Ask Theo to run a command outside the sandbox, and get its output back.
//!
//!     ask-to-run [--cwd DIR] --why '<what it does and why>' '<command>'
//!     ask-to-run --wait <id>          keep waiting for an earlier request
//!
//! Run by Claude inside greywall. The request goes to ~/.cache/claude/runs/<id>.request and the OSD
//! daemon opens an approval terminal outside the sandbox (claude-run-approve). Nothing runs until
//! Theo reads the command there and answers y. --why is required: a few plain sentences shown under
//! the command, saying what it does, why it is needed and what to expect. Prints the command's
//! output and exits with its status; 125 when Theo declines, 124 when the wait runs out (9 minutes,
//! under the Bash tool's 10), 126 when no daemon reads the requests (fall back to a fish block).
//! While it waits it touches <id>.waiting every few seconds: the daemon drops an approval nobody
//! waits for any more, and --wait on it then says so.
//! CLAUDE_RUNS_DIR and ASK_TO_RUN_WAIT_S override the folder and the wait (tests).

use cockpit::approval;
use cockpit::util::{now_secs, token_hex};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const OUTPUT_LINES: usize = 400;
const WHY_MAX: usize = 1000;
const USAGE: &str = "usage: ask-to-run [--cwd DIR] --why '<what it does and why>' '<command>' | ask-to-run --wait <id>";
const DECLINED: i32 = 125;
const TIMED_OUT: i32 = 124;
const NO_DAEMON: i32 = 126;
const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);

fn wait_seconds() -> u64 {
    std::env::var("ASK_TO_RUN_WAIT_S").ok().and_then(|s| s.parse().ok()).unwrap_or(540)
}

fn touch(path: &std::path::Path) {
    if let Err(error) = std::fs::write(path, now_secs().to_string()) {
        eprintln!("ask-to-run: cannot write {}: {error}", path.display());
    }
}

fn wait_for_result(id: &str) -> i32 {
    let path = approval::runs_dir().join(format!("{id}.result"));
    let heartbeat = approval::runs_dir().join(format!("{id}.waiting"));
    let deadline = Instant::now() + Duration::from_secs(wait_seconds());
    let mut last_beat: Option<Instant> = None;
    while !path.exists() {
        if Instant::now() >= deadline {
            println!("No answer after {} min. Keep waiting with: ask-to-run --wait {id} (within 5 minutes, or the request is dropped)", wait_seconds() / 60);
            return TIMED_OUT;
        }
        if last_beat.is_none_or(|beat| beat.elapsed() >= HEARTBEAT_EVERY) {
            touch(&heartbeat);
            last_beat = Some(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let _ = std::fs::remove_file(&heartbeat);
    let result: Value = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(json!({}));
    if result.get("status").and_then(Value::as_str) == Some("abandoned") {
        println!("The request was dropped: nobody waited for its answer for a while, so Theo's approval window closed. Ask again if it is still needed.");
        return TIMED_OUT;
    }
    if result.get("status").and_then(Value::as_str) == Some("declined") {
        println!("Theo declined to run it.");
        return DECLINED;
    }
    if let Some(edited) = result.get("edited_command").and_then(Value::as_str).filter(|e| !e.is_empty()) {
        println!("Theo edited the command before running it:\n  {edited}\n");
    }
    let output = approval::strip_ansi(result.get("output").and_then(Value::as_str).unwrap_or(""));
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() > OUTPUT_LINES {
        println!("[first {} lines left out]", lines.len() - OUTPUT_LINES);
    }
    println!("{}", lines[lines.len().saturating_sub(OUTPUT_LINES)..].join("\n"));
    if let Some(redacted) = result.get("redacted").and_then(Value::as_u64).filter(|count| *count > 0) {
        println!("[{redacted} secret-looking value(s) shown as [redacted]]");
    }
    let exit = result.get("exit").and_then(Value::as_i64).unwrap_or(1) as i32;
    println!("[exit {exit}]");
    exit
}

fn main() {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() == 2 && arguments[0] == "--wait" && approval::valid_id(&arguments[1]) {
        std::process::exit(wait_for_result(&arguments[1]));
    }
    let mut cwd = std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    let mut why = String::new();
    while arguments.len() >= 3 && (arguments[0] == "--cwd" || arguments[0] == "--why") {
        if arguments[0] == "--cwd" {
            let path = std::path::Path::new(&arguments[1]);
            cwd = if path.is_absolute() { arguments[1].clone() } else { std::env::current_dir().unwrap_or_default().join(path).to_string_lossy().into_owned() };
        } else {
            why = cockpit::util::collapse(&arguments[1]);
        }
        arguments.drain(..2);
    }
    if arguments.len() != 1 || arguments[0].trim().is_empty() {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    if why.is_empty() || why.chars().count() > WHY_MAX {
        eprintln!("--why is required (at most {WHY_MAX} characters): what the command does, why it is needed and what \
                   Theo should expect. It is shown under the command in his approval window.\n{USAGE}");
        std::process::exit(2);
    }
    let id = token_hex(6);
    let request = json!({"id": id, "command": arguments[0], "why": why, "cwd": cwd, "project": approval::project_of(&cwd),
                         "session": std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default(), "requested_at": now_secs()});
    let path = approval::runs_dir().join(format!("{id}.request"));
    if let Err(error) = approval::write_json(&path, &request) {
        eprintln!("ask-to-run: cannot file the request: {error}");
        std::process::exit(1);
    }
    touch(&approval::runs_dir().join(format!("{id}.waiting")));
    if !approval::send(&format!("run-request {id}")) {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(approval::runs_dir().join(format!("{id}.waiting")));
        println!("No OSD daemon reads the requests (osd.service down?). Give Theo the command as a fish block instead.");
        std::process::exit(NO_DAEMON);
    }
    eprintln!("Asked Theo to run it (request {id}); waiting for his answer.");
    std::process::exit(wait_for_result(&id));
}
