//! The Claude Code hook (config/claude/.claude/hooks/session-state.sh): one FIFO line per event.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

struct HookTest {
    sandbox: Sandbox,
    fifo: PathBuf,
    reader: Option<Child>,
    holder: Child,
}

impl HookTest {
    fn new() -> HookTest {
        let sandbox = Sandbox::new();
        let fifo = sandbox.runtime.join("osd.fifo");
        let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0, "mkfifo {}", fifo.display());
        // Collect what arrives, and hold the FIFO open for writing like the daemon does so the
        // reader sees no EOF between hook runs. The reader must open first: opening a FIFO for
        // writing blocks until someone reads.
        let reader = Command::new("cat").arg(&fifo).stdout(Stdio::piped()).spawn().unwrap();
        let writer = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
        // Only the holder keeps the write end (the temporary Command drops ours), so killing it gives the reader its EOF.
        let holder = Command::new("sleep").arg("30").stdout(writer).spawn().unwrap();
        sandbox.shim("notify-send", "echo 42");
        sandbox.shim("gdbus", "");
        HookTest { sandbox, fifo, reader: Some(reader), holder }
    }

    fn run_hook(&self, payload: Value, extra: &[(&str, &str)]) -> Output {
        let mut environment = vec![("KITTY_PID", "4242"), ("TMUX_PANE", "%3")];
        environment.extend_from_slice(extra);
        self.sandbox.run_with_input("bash", &[hook().to_str().unwrap()], &environment, &payload.to_string())
    }

    fn received(&mut self) -> Vec<Value> {
        std::thread::sleep(Duration::from_millis(300));
        let _ = self.holder.kill();
        let _ = self.holder.wait();
        let output = self.reader.take().unwrap().wait_with_output().unwrap();
        stdout(&output)
            .lines()
            .filter_map(|line| line.strip_prefix("session "))
            .map(|payload| serde_json::from_str(payload).unwrap())
            .collect()
    }
}

impl Drop for HookTest {
    fn drop(&mut self) {
        let _ = self.holder.kill();
        let _ = self.holder.wait();
        if let Some(mut reader) = self.reader.take() {
            let _ = reader.kill();
            let _ = reader.wait();
        }
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
    fs::copy(bash, &fake).unwrap();
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
fn fallback_toast_when_the_fifo_is_unreachable() {
    let test = HookTest::new();
    fs::remove_file(&test.fifo).unwrap();
    test.run_hook(with_session(json!({"hook_event_name": "Stop"}), "/x/proj"), &[]);
    assert_eq!(test.sandbox.calls("notify-send").len(), 1);
    let toast_id = test.sandbox.home.join(format!(".cache/claude/toast-{SESSION_A}.id"));
    assert_eq!(fs::read_to_string(&toast_id).unwrap().trim(), "42");
    test.run_hook(with_session(json!({"hook_event_name": "UserPromptSubmit", "prompt": "x"}), "/x/proj"), &[]);
    assert_eq!(test.sandbox.calls("gdbus").len(), 1);
    assert!(!toast_id.exists());
}
