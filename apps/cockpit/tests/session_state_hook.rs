//! The Claude Code hook (config/claude/.claude/hooks/session-state.sh): one report per event, on the
//! daemon's report socket.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct HookTest {
    sandbox: Sandbox,
    socket: PathBuf,
    lines: Arc<Mutex<Vec<String>>>,
}

impl HookTest {
    /// A stand-in for the daemon's report socket: every line any connection sends is kept.
    fn new() -> HookTest {
        let sandbox = Sandbox::new();
        let socket = sandbox.runtime.join("osd-report.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let lines = Arc::new(Mutex::new(Vec::new()));
        let collected = lines.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut text = String::new();
                let _ = stream.read_to_string(&mut text);
                collected.lock().unwrap().extend(text.lines().map(str::to_string));
            }
        });
        sandbox.shim("notify-send", "echo 42");
        sandbox.shim("gdbus", "");
        HookTest { sandbox, socket, lines }
    }

    fn run_hook(&self, payload: Value, extra: &[(&str, &str)]) -> Output {
        let mut environment = vec![("KITTY_PID", "4242"), ("TMUX_PANE", "%3")];
        environment.extend_from_slice(extra);
        self.sandbox.run_with_input("bash", &[hook().to_str().unwrap()], &environment, &payload.to_string())
    }

    fn received(&mut self) -> Vec<Value> {
        std::thread::sleep(Duration::from_millis(300));
        self.lines.lock().unwrap().iter()
            .filter_map(|line| line.strip_prefix("session "))
            .map(|payload| serde_json::from_str(payload).unwrap())
            .collect()
    }
}

fn with_session(payload: Value, cwd: &str) -> Value {
    let mut payload = payload;
    payload["session_id"] = json!(SESSION_A);
    payload["cwd"] = json!(cwd);
    payload
}

/// A stand-in process named "claude": a copy of bash under that name.
fn fake_claude(root: &Path) -> PathBuf {
    let bash = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|directory| directory.join("bash"))
        .find(|candidate| candidate.is_file())
        .expect("bash on PATH");
    let fake = root.join("claude");
    copy_executable(&bash, &fake);
    fake
}

#[test]
fn events_map_to_states() {
    let mut test = HookTest::new();
    let cases = [
        (json!({"hook_event_name": "SessionStart", "source": "startup"}), "waiting", None),
        (json!({"hook_event_name": "UserPromptSubmit", "prompt": "add the chip\nmore"}), "working", None),
        (json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"}), "working", None),
        (json!({"hook_event_name": "Notification", "message": "perm", "notification_type": "permission_prompt"}), "needs-you", Some(true)),
        (json!({"hook_event_name": "Notification", "message": "idle", "notification_type": "idle_prompt"}), "waiting", Some(true)),
        (json!({"hook_event_name": "Notification", "message": "done", "notification_type": "push_notification"}), "waiting", None),
        (json!({"hook_event_name": "Stop"}), "waiting", None),
        (json!({"hook_event_name": "SessionEnd", "reason": "exit"}), "gone", None),
    ];
    for (payload, _, _) in &cases {
        test.run_hook(with_session(payload.clone(), "/x/proj"), &[]);
    }
    let lines = test.received();
    let states: Vec<&str> = lines.iter().map(|line| line["state"].as_str().unwrap()).collect();
    assert_eq!(states, cases.iter().map(|(_, state, _)| *state).collect::<Vec<_>>());
    let stickies: Vec<Option<bool>> = lines.iter().map(|line| line.get("sticky").map(|sticky| sticky.as_bool().unwrap())).collect();
    assert_eq!(stickies, cases.iter().map(|(_, _, sticky)| *sticky).collect::<Vec<_>>());
    assert_eq!(lines[1]["task"], "add the chip");
    assert_eq!(lines[0]["tmux_pane"], "%3");
}

#[test]
fn informational_notification_carries_no_state() {
    let mut test = HookTest::new();
    test.run_hook(
        with_session(json!({"hook_event_name": "Notification", "message": "ok", "notification_type": "auth_success"}), "/x/proj"),
        &[],
    );
    let line = test.received().remove(0);
    assert!(line.get("state").is_none(), "{line}");
    assert_eq!(line["message"], "ok");
}

#[test]
fn unknown_event_fails_loudly() {
    let test = HookTest::new();
    let result = test.run_hook(with_session(json!({"hook_event_name": "PreCompact"}), "/x"), &[]);
    assert_eq!(result.status.code(), Some(1));
    assert!(stderr(&result).contains("unexpected hook event"), "{}", stderr(&result));
}

#[test]
fn nested_claude_reports_nothing() {
    let mut test = HookTest::new();
    // A stand-in process named "claude" that a tool started (CLAUDECODE=1 in its environment),
    // between the hook and its real ancestors.
    let fake = fake_claude(&test.sandbox.root);
    let script = format!("bash {}; :", hook().display());
    let payload = with_session(json!({"hook_event_name": "Stop"}), "/x/.issues-tmp");
    test.sandbox.run_with_input(&fake, &["-c", &script], &[("CLAUDECODE", "1")], &payload.to_string());
    assert_eq!(test.received(), Vec::<Value>::new());
}

#[test]
fn a_session_started_from_a_shell_reports_even_under_the_daemon() {
    use std::io::Write;
    let mut test = HookTest::new();
    // Since Claude Code 2.1.300 a session sits under processes named claude (launcher, daemon)
    // that carry no CLAUDECODE mark: two of them must not look like nesting.
    let fake = fake_claude(&test.sandbox.root);
    // The daemon scrubs the session's environment: the pane and the kitty window come from
    // the launcher above (the outer fake claude here).
    let script = format!("env -u TMUX_PANE -u KITTY_PID {} -c 'bash {}'; :", fake.display(), hook().display());
    let mut child = test
        .sandbox
        .command(&fake, &[("TMUX_PANE", "%9"), ("KITTY_PID", "4242")])
        .env_remove("CLAUDECODE")
        .args(["-c", &script])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = with_session(json!({"hook_event_name": "Stop"}), "/x/proj");
    child.stdin.take().unwrap().write_all(payload.to_string().as_bytes()).unwrap();
    child.wait().unwrap();
    let lines = test.received();
    let states: Vec<&str> = lines.iter().map(|line| line["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["waiting"]);
    assert_eq!((&lines[0]["tmux_pane"], &lines[0]["kitty_pid"]), (&json!("%9"), &json!("4242")));
}

#[test]
fn each_event_leaves_a_line_in_the_hook_log() {
    let mut test = HookTest::new();
    test.run_hook(
        with_session(json!({"hook_event_name": "UserPromptSubmit", "prompt": "the secret plan", "source": "user"}), "/x/proj"),
        &[],
    );
    test.received();
    let log = fs::read_to_string(test.sandbox.home.join(".cache/claude/hook.log")).unwrap();
    let line = log.lines().last().unwrap();
    assert!(regex::Regex::new(r" UserPromptSubmit aaaaaaaa working: sent to the osd daemon$").unwrap().is_match(line), "{line}");
    assert!(!line.contains("secret"), "{line}");
}

#[test]
fn fallback_toast_when_the_daemon_is_unreachable() {
    let test = HookTest::new();
    fs::remove_file(&test.socket).unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "Stop"}), "/x/proj"), &[]);
    assert_eq!(test.sandbox.calls("notify-send").len(), 1);
    let toast_id = test.sandbox.home.join(format!(".cache/claude/toast-{SESSION_A}.id"));
    assert_eq!(fs::read_to_string(&toast_id).unwrap().trim(), "42");
    test.run_hook(with_session(json!({"hook_event_name": "UserPromptSubmit", "prompt": "x"}), "/x/proj"), &[]);
    assert_eq!(test.sandbox.calls("gdbus").len(), 1);
    assert!(!toast_id.exists());
}

fn git(repository: &Path, arguments: &[&str]) {
    let status = Command::new("git").arg("-C").arg(repository).args(["-c", "user.email=t@t", "-c", "user.name=t"]).args(arguments)
        .output().unwrap().status;
    assert!(status.success(), "git {arguments:?}");
}

#[test]
fn a_finished_turn_reports_its_commits_and_uncommitted_changes() {
    // Worked out by the hook, inside the session's sandbox: git reads configuration the session
    // can write, so the daemon outside never runs it.
    let mut test = HookTest::new();
    let repository = test.sandbox.root.join("repo");
    fs::create_dir(&repository).unwrap();
    git(&repository, &["init", "-q"]);
    fs::write(repository.join("a.txt"), "one\n").unwrap();
    git(&repository, &["add", "a.txt"]);
    git(&repository, &["commit", "-qm", "start"]);
    let cwd = repository.to_string_lossy().into_owned();
    test.run_hook(with_session(json!({"hook_event_name": "UserPromptSubmit", "prompt": "work"}), &cwd), &[]);
    fs::write(repository.join("b.txt"), "two\n").unwrap();
    git(&repository, &["add", "b.txt"]);
    git(&repository, &["commit", "-qm", "work"]);
    fs::write(repository.join("a.txt"), "one\nmore\nlines\n").unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "Stop"}), &cwd), &[]);
    let reports = test.received();
    assert_eq!(reports.last().unwrap()["outcome"], "1 commit · 1 file changed, +2 −0 uncommitted");
}

#[test]
fn a_turn_outside_a_repository_reports_no_outcome() {
    let mut test = HookTest::new();
    let folder = test.sandbox.root.join("notes");
    fs::create_dir(&folder).unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "Stop"}), &folder.to_string_lossy()), &[]);
    assert!(test.received().last().unwrap().get("outcome").is_none());
}

/// A transcript: a prompt, then each (command, failed) as a Bash call and its result.
fn transcript(folder: &Path, turns: &[(&str, &[(&str, bool)])]) -> String {
    let mut lines = Vec::new();
    for (turn, (prompt, calls)) in turns.iter().enumerate() {
        lines.push(json!({"type": "user", "message": {"role": "user", "content": prompt}}));
        for (index, (command, failed)) in calls.iter().enumerate() {
            let id = format!("toolu_{turn}_{index}");
            lines.push(json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}]}}));
            lines.push(json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id, "is_error": failed, "content": "…"}]}}));
        }
    }
    let path = folder.join("transcript.jsonl");
    fs::write(&path, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    path.to_string_lossy().into_owned()
}

fn stop_outcome(turns: &[(&str, &[(&str, bool)])]) -> Option<String> {
    let mut test = HookTest::new();
    let folder = test.sandbox.root.join("notes");
    fs::create_dir(&folder).unwrap();
    let mut payload = with_session(json!({"hook_event_name": "Stop"}), &folder.to_string_lossy());
    payload["transcript_path"] = json!(transcript(&folder, turns));
    test.run_hook(payload, &[]);
    test.received().last().unwrap().get("outcome").and_then(Value::as_str).map(str::to_string)
}

#[test]
fn a_turn_whose_last_test_run_failed_says_so() {
    let outcome = stop_outcome(&[("fix it", &[("make test", false), ("cargo test -p vitals", true)])]);
    assert_eq!(outcome.as_deref(), Some("tests failed"));
}

#[test]
fn a_turn_that_passed_its_tests_and_closed_a_todo_says_both() {
    let outcome = stop_outcome(&[("fix it", &[("ask-to-run --why 'x' 'make cockpit; and make test'", false), ("claude-todo done 8e50f7a0", false)])]);
    assert_eq!(outcome.as_deref(), Some("tests passed · todo done"));
}

#[test]
fn tests_of_an_earlier_turn_do_not_count() {
    let outcome = stop_outcome(&[("first", &[("make test", true)]), ("second", &[("ls", false)])]);
    assert_eq!(outcome, None);
}

#[test]
fn a_test_command_only_mentioned_in_text_does_not_count() {
    let outcome = stop_outcome(&[("write the test", &[("cat >> t.rs <<'X'\nstop_outcome(&[(\"make test\", true), (\"claude-todo done 1\", false)]);\nX", false)])]);
    assert_eq!(outcome, None);
}

#[test]
fn a_session_started_in_its_own_terminal_says_so() {
    let mut test = HookTest::new();
    let folder = test.sandbox.root.join("notes");
    fs::create_dir(&folder).unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "SessionStart"}), &folder.to_string_lossy()), &[("CLAUDE_SESSIONS_TERMINAL", "1")]);
    test.run_hook(with_session(json!({"hook_event_name": "SessionStart"}), &folder.to_string_lossy()), &[]);
    let reports = test.received();
    assert_eq!(reports[0]["terminal"], true);
    assert!(reports[1].get("terminal").is_none(), "a cockpit session carries no flag");
}
