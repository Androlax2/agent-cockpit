//! claude-run-approve <id>: the approval terminal for a command Claude asked to run (ask-to-run).
//!
//! Started by the OSD daemon outside the sandbox, in a tmux session of its own (claude-run-<id>),
//! shown in the cockpit and, while it needs Theo, in a floating window; it reports asking /
//! running / done. Runs with Theo's rights, so nothing happens without his "y". The request is read
//! once, here: the sandbox can rewrite the file, so what is shown is what runs. "e" edits the
//! command first; the edited command is what runs and what Claude is told. The command runs in a
//! real terminal through `script`, so sudo passwords and prompts work, and its output is recorded
//! in the runtime directory, which the sandbox cannot read; it reaches Claude only after "send the
//! output to Claude?". CLAUDE_RUNS_DIR overrides the requests folder (tests).

use cockpit::approval::{self, BOLD, BLUE, DIM, GREEN, RED, RESET, YELLOW};
use cockpit::util::now_secs;
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::Path;

const MAX_AGE_S: i64 = 15 * 60;
const LONG_COMMAND: usize = 600;

/// The request, checked, or why it cannot be answered.
fn read_request(id: &str) -> Result<Value, String> {
    if !approval::valid_id(id) {
        return Err("not a request id".into());
    }
    let directory = approval::runs_dir();
    if directory.join(format!("{id}.result")).exists() {
        return Err("already answered".into());
    }
    let text = std::fs::read_to_string(directory.join(format!("{id}.request"))).map_err(|error| error.to_string())?;
    let request: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    if !request.get("command").and_then(Value::as_str).is_some_and(|command| !command.trim().is_empty()) {
        return Err("no command in the request".into());
    }
    if now_secs() - request.get("requested_at").and_then(Value::as_i64).unwrap_or(0) > MAX_AGE_S {
        return Err("older than 15 minutes".into());
    }
    Ok(request)
}

fn write_result(id: &str, result: Value) {
    let _ = approval::write_json(&approval::runs_dir().join(format!("{id}.result")), &result);
}

fn approve(id: &str) -> i32 {
    let request = match read_request(id) {
        Ok(request) => request,
        Err(reason) => {
            println!("{RED}Cannot answer request {id}: {reason}.{RESET}");
            return 1;
        }
    };
    let original = request["command"].as_str().unwrap_or("").to_string();
    let mut command = original.clone();
    let why = request.get("why").and_then(Value::as_str).unwrap_or("").to_string();
    let cwd = request.get("cwd").and_then(Value::as_str).filter(|cwd| Path::new(cwd).is_dir()).map(str::to_string)
        .unwrap_or_else(|| cockpit::paths::home().to_string_lossy().into_owned());
    let project = approval::project_of(&cwd);
    approval::phase(id, "asking");
    let answer = loop {
        let (shown, hiding) = approval::visible_command(&command);
        // The folder and project come from the sandbox too: a directory name can carry escape codes.
        let shown_project = approval::escape_hiding(&project, RED, &format!("{RESET}{BOLD}{BLUE}"));
        let shown_cwd = approval::escape_hiding(&cwd, RED, &format!("{RESET}{DIM}"));
        println!("\n{BOLD}{BLUE}Claude · {shown_project}{RESET} asks you to run, in {DIM}{shown_cwd}{RESET}:\n");
        println!("{}\n", shown.split('\n').map(|line| format!("  {BOLD}{line}{RESET}")).collect::<Vec<_>>().join("\n"));
        println!("{}\n", approval::explanation(&why));
        if hiding {
            println!("{RED}{BOLD}This command contains hidden characters (shown in red as \\uXXXX). \
                      They can make a command look different from what runs. Decline unless you know why.{RESET}\n");
        }
        if command.chars().count() > LONG_COMMAND {
            println!("{YELLOW}Long command ({} characters): read all of it.{RESET}\n", command.chars().count());
        }
        let answer = approval::choose("Run it?", &[("n", "Decline"), ("y", "Run it"), ("e", "Edit it first")], "n");
        if answer == "e" {
            if let Some(edited) = approval::edit(&command) {
                command = edited;
            }
            continue;
        }
        break answer;
    };
    approval::send(&format!("notice-clear run-{id}"));
    if answer != "y" {
        approval::phase(id, "done");
        write_result(id, json!({"status": "declined"}));
        approval::log("INFO", &format!("declined {id} for {project}"));
        println!("Declined: Claude is told.");
        return 0;
    }
    let log_path = approval::log_dir().join(format!("{id}.log"));
    println!("{DIM}─── running ───{RESET}");
    approval::phase(id, "running");
    let exit_code = approval::run(&command, &cwd, &log_path);
    approval::phase(id, "asking");
    println!("{DIM}─── {RESET}{}exit {exit_code}{RESET}", if exit_code == 0 { GREEN } else { RED });
    let keep = approval::choose("Send the output to Claude?", &[("y", "Send the output"), ("n", "Keep it out (only the exit status)")], "y") == "y";
    let edited = if command != original { json!(command) } else { Value::Null };
    write_result(id, json!({"status": "ran", "exit": exit_code, "output": if keep { approval::recorded_output(&log_path) } else { String::new() },
                            "output_withheld": !keep, "edited_command": edited}));
    let _ = std::fs::remove_file(&log_path);
    approval::phase(id, "done");
    approval::log("INFO", &format!("ran {id} for {project}: exit {exit_code}{}{}",
                                   if command != original { ", edited" } else { "" }, if keep { "" } else { ", output withheld" }));
    0
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.len() != 1 {
        eprintln!("usage: claude-run-approve <id>");
        std::process::exit(1);
    }
    let status = approve(&arguments[0]);
    if std::env::var_os("TMUX").is_none() {
        // In its tmux session the pane and window close by themselves.
        print!("{DIM}Enter closes this window.{RESET}");
        let _ = io::stdout().flush();
        let _ = io::stdin().lock().read_line(&mut String::new());
    }
    std::process::exit(status);
}
