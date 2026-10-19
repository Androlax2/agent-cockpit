//! claude-run-approve <id>: the approval terminal for a command Claude asked to run (ask-to-run).
//!
//! Started by the OSD daemon outside the sandbox, in a tmux session of its own (claude-run-<id>),
//! shown in the cockpit and, while it needs Theo, in a floating window; it reports asking /
//! running / done. Runs with Theo's rights, so nothing happens without his "y". The request is read
//! once, here, from the daemon's copy (approval::held_request), which no sandbox reaches. "e" edits
//! the command first; the edited command is what runs and what Claude is told. The command runs in
//! a real terminal through `script`, so sudo passwords and prompts work, and its output is recorded
//! in the runtime directory, which the sandbox cannot read; it reaches Claude only after "send the
//! output to Claude?". XDG_RUNTIME_DIR moves the daemon's copies and the log (tests).

use cockpit::approval::{self, BOLD, BLUE, DIM, GREEN, RED, RESET, YELLOW};
use cockpit::audit;
use cockpit::util::now_secs;
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};


const MAX_AGE_S: i64 = 15 * 60;
const LONG_COMMAND: usize = 600;

/// The request, checked, with the answers folder it names, or why it cannot be answered.
fn read_request(id: &str) -> Result<(Value, PathBuf), String> {
    if !approval::valid_id(id) {
        return Err("not a request id".into());
    }
    let text = std::fs::read_to_string(approval::held_request(id, "request")).map_err(|error| error.to_string())?;
    let request: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let answers = approval::request_answers_dir(&request, "runs")
        .ok_or("it names no answers folder (its session started before answers went per project: restart it)")?;
    if answers.join(format!("{id}.result")).exists() {
        return Err("already answered".into());
    }
    if !request.get("command").and_then(Value::as_str).is_some_and(|command| !command.trim().is_empty()) {
        return Err("no command in the request".into());
    }
    if now_secs() - request.get("requested_at").and_then(Value::as_i64).unwrap_or(0) > MAX_AGE_S {
        return Err("older than 15 minutes".into());
    }
    Ok((request, answers))
}

fn write_result(answers: &Path, id: &str, result: Value) {
    let _ = approval::write_json(&answers.join(format!("{id}.result")), &result);
}

fn approve(id: &str) -> i32 {
    let (request, answers) = match read_request(id) {
        Ok(read) => read,
        Err(reason) => {
            println!("{RED}Cannot answer request {id}: {reason}.{RESET}");
            return 1;
        }
    };
    let original = request["command"].as_str().unwrap_or("").to_string();
    let mut command = original.clone();
    let why = request.get("why").and_then(Value::as_str).unwrap_or("").to_string();
    let cwd = request.get("cwd").and_then(Value::as_str).and_then(approval::resolved_directory)
        .unwrap_or_else(|| cockpit::paths::home().to_string_lossy().into_owned());
    if let Err(error) = approval::enter_directory(&cwd) {
        eprintln!("{RED}cannot enter {cwd}: {error}{RESET}");
        write_result(&answers, id, json!({"status": "declined"}));
        return 1;
    }
    // Both come from the daemon's copy, out of the sandbox's reach: the folder the asking session's
    // sandbox started in, and its project.
    let started_in = request.get("started_in").and_then(Value::as_str).unwrap_or("").to_string();
    let project = request.get("project").and_then(Value::as_str).unwrap_or("?").to_string();
    let is_outside_its_folder = !std::path::Path::new(&cwd).starts_with(&started_in) || started_in.is_empty();
    let session = request.get("session").and_then(Value::as_str).unwrap_or("").to_string();
    audit::record("run", "run.shown", "asking", json!({"id": id, "project": project, "started_in": started_in, "cwd": cwd,
                                                      "claimed_session": session, "command": audit::fingerprint(&original),
                                                      "is_outside_its_folder": is_outside_its_folder}));
    approval::phase(id, "asking");
    let answer = loop {
        let (shown, hiding) = approval::visible_command(&command);
        // The folder and project come from the sandbox too: a directory name can carry escape codes.
        let shown_project = approval::escape_hiding(&project, RED, &format!("{RESET}{BOLD}{BLUE}"));
        let shown_cwd = approval::escape_hiding(&cwd, RED, &format!("{RESET}{DIM}"));
        println!("\n{BOLD}{BLUE}Claude · {shown_project}{RESET} asks you to run, in {DIM}{shown_cwd}{RESET}:\n");
        println!("{}\n", shown.split('\n').map(|line| format!("  {BOLD}{line}{RESET}")).collect::<Vec<_>>().join("\n"));
        println!("{}\n", approval::explanation(&why));
        if is_outside_its_folder {
            let shown_started_in = approval::escape_hiding(&started_in, RED, &format!("{RESET}{RED}{BOLD}"));
            println!("{RED}{BOLD}This runs outside the folder the session works in ({shown_started_in}). \
                      Decline unless you expected that folder.{RESET}\n");
        }
        if hiding {
            println!("{RED}{BOLD}This command contains hidden characters or blank-line padding (shown in red). \
                      They can make a command look different from what runs. Decline unless you know why.{RESET}\n");
        }
        let foreign = approval::non_ascii(&command);
        if !foreign.is_empty() {
            println!("{YELLOW}Characters outside ASCII (a lookalike letter can change a command): {}{RESET}\n", foreign.join(", "));
        }
        if command.chars().count() > LONG_COMMAND {
            println!("{YELLOW}Long command ({} characters): read all of it.{RESET}\n", command.chars().count());
        }
        // The phone page (claude-remote) can answer too: it shows what this shows, from this file.
        let _ = std::fs::create_dir_all(approval::log_dir());
        let _ = std::fs::remove_file(approval::remote_answer(id));
        let _ = approval::write_json(&approval::remote_asking(id), &json!({"id": id, "project": project, "cwd": cwd, "started_in": started_in,
                                                                         "command": command, "why": why, "is_outside_its_folder": is_outside_its_folder}));
        let answer = approval::choose_or_remote("Run it?", &[("n", "Decline"), ("y", "Run it"), ("e", "Edit it first")], "n",
                                                Some(&approval::remote_answer(id)));
        let _ = std::fs::remove_file(approval::remote_asking(id));
        if answer == "e" {
            if let Some(edited) = approval::edit(&command) {
                command = edited;
            }
            continue;
        }
        break answer;
    };
    approval::send_host(&format!("notice-clear run-{id}"));
    if answer != "y" {
        approval::phase(id, "done");
        write_result(&answers, id, json!({"status": "declined"}));
        approval::log("INFO", &format!("declined {id} for {project}"));
        audit::record("run", "run.declined", "declined", json!({"id": id, "project": project, "command": audit::fingerprint(&command)}));
        println!("Declined: Claude is told.");
        return 0;
    }
    audit::keep_command(id, &original, &command);
    audit::record("run", "run.approved", "approved", json!({"id": id, "project": project, "cwd": cwd, "edited": command != original,
                                                          "command": audit::fingerprint(&command)}));
    let log_path = approval::log_dir().join(format!("{id}.log"));
    println!("{DIM}─── running ───{RESET}");
    approval::phase(id, "running");
    let exit_code = approval::run(&command, "", &log_path, Some(id));
    let trace_path = approval::trace_path(&log_path);
    let failed_step = if exit_code == 0 { None } else { approval::last_step(&std::fs::read_to_string(&trace_path).unwrap_or_default()) };
    let _ = std::fs::remove_file(&trace_path);
    let stopped_at = failed_step.as_ref().map(|step| format!(" · stopped at: {step}")).unwrap_or_default();
    println!("{DIM}─── {RESET}{}exit {exit_code}{stopped_at}{RESET}", if exit_code == 0 { GREEN } else { RED });
    // The output always goes back, secrets taken out first: approving the command is approving
    // that Claude reads what it printed.
    let (output, redacted) = approval::redact_secrets(&approval::recorded_output(&log_path));
    if redacted > 0 {
        println!("{YELLOW}{redacted} secret-looking value{} redacted before the output went to Claude.{RESET}", if redacted == 1 { "" } else { "s" });
    }
    let edited = if command != original { json!(command) } else { Value::Null };
    // The whole output stays readable by Claude in a file; the result carries it too, and ask-to-run
    // prints only its end.
    let output_path = answers.join(format!("{id}.output"));
    let _ = std::fs::write(&output_path, approval::strip_ansi(&output));
    let step = failed_step.map(Value::from).unwrap_or(Value::Null);
    write_result(&answers, id, json!({"status": "ran", "exit": exit_code, "output": output, "redacted": redacted, "edited_command": edited,
                            "failed_step": step, "output_file": output_path.display().to_string()}));
    let _ = std::fs::remove_file(&log_path);
    approval::phase(id, "done");
    audit::record("run", "run.ran", if exit_code == 0 { "ok" } else { "failed" },
                  json!({"id": id, "project": project, "exit": exit_code, "redacted": redacted, "output": audit::fingerprint(&output)}));
    approval::log("INFO", &format!("ran {id} for {project}: exit {exit_code}{}{}", if command != original { ", edited" } else { "" },
                                   if redacted > 0 { format!(", {redacted} secret(s) redacted") } else { String::new() }));
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
