//! Run the hook as Codex does, against a channel double enforcing conversation handoff.
use std::io::Write;
use std::process::{Command, Output, Stdio};

struct Session {
    dir: tempfile::TempDir,
}

impl Session {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("reports-state"), "").unwrap();
        let cockpit = dir.path().join("cockpit");
        std::fs::write(
            &cockpit,
            r#"#!/usr/bin/env python3
import json, os, pathlib, sys
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
if args == ['brief']:
    print('launch brief')
    sys.exit(0)
with (root / 'calls').open('a') as log:
    log.write(json.dumps(args) + '\n')
state_path = root / 'channel'
state = json.loads(state_path.read_text()) if state_path.exists() else {}
conversation = args[args.index('--conversation') + 1]
ending = '--conversation-ends' in args
if (root / 'refuse').exists() and (root / 'refuse').read_text() in ('', conversation):
    sys.exit(3)
if state.get('current') not in (None, conversation):
    if ending or not state.get('ending'):
        sys.exit(4)
state = {'current': conversation, 'ending': ending}
state_path.write_text(json.dumps(state))
"#,
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(cockpit, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn hook(&self, name: &str, id: &str, source: &str) -> Output {
        let payload = serde_json::json!({
            "hook_event_name": name, "session_id": id, "source": source,
        });
        let mut child = Command::new(env!("CARGO_BIN_EXE_cockpit-codex-hook"))
            .arg(self.dir.path().join("cockpit"))
            .env(
                "COCKPIT_CODEX_REPORT_STATE",
                self.dir.path().join("reports-state"),
            )
            .env("COCKPIT_LAUNCH_TOKEN", "isolated-test-token")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn start(&self, id: &str, source: &str) {
        let output = self.hook("SessionStart", id, source);
        assert!(output.status.success(), "{output:?}");
    }

    fn current(&self) -> String {
        let state: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(self.dir.path().join("channel")).unwrap(),
        )
        .unwrap();
        state["current"].as_str().unwrap().to_owned()
    }

    fn calls(&self) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.dir.path().join("calls"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[test]
fn clear_announces_the_old_conversation_before_following_the_new_one() {
    let session = Session::new();
    session.start("thread-1", "startup");
    session.start("thread-2", "clear");
    assert_eq!(session.current(), "thread-2");
    assert_eq!(
        session.calls()[1],
        [
            "agent",
            "report",
            "--conversation",
            "thread-1",
            "--conversation-ends"
        ]
    );
    assert!(
        session
            .hook("UserPromptSubmit", "thread-2", "")
            .status
            .success()
    );
    session.start("thread-3", "clear");
    assert_eq!(session.current(), "thread-3");
}

#[test]
fn duplicate_and_late_clear_hooks_never_move_back_to_a_retired_conversation() {
    let session = Session::new();
    session.start("thread-1", "startup");
    session.start("thread-2", "clear");
    session.start("thread-2", "clear");
    session.start("thread-3", "clear");
    let calls = session.calls().len();
    session.start("thread-2", "clear");
    assert!(session.hook("Stop", "thread-1", "").status.success());
    assert_eq!(session.calls().len(), calls);
    assert_eq!(session.current(), "thread-3");
}

#[test]
fn a_refused_handoff_is_reported_and_can_be_retried() {
    let session = Session::new();
    session.start("thread-1", "startup");
    std::fs::write(session.dir.path().join("refuse"), "").unwrap();
    assert_eq!(
        session
            .hook("SessionStart", "thread-2", "clear")
            .status
            .code(),
        Some(1)
    );
    assert_eq!(session.current(), "thread-1");
    std::fs::remove_file(session.dir.path().join("refuse")).unwrap();
    session.start("thread-2", "clear");
    assert_eq!(session.current(), "thread-2");
}

#[test]
fn a_prompt_can_finish_a_handoff_whose_successor_report_failed() {
    let session = Session::new();
    session.start("thread-1", "startup");
    std::fs::write(session.dir.path().join("refuse"), "thread-2").unwrap();
    assert_eq!(
        session
            .hook("SessionStart", "thread-2", "clear")
            .status
            .code(),
        Some(1)
    );
    std::fs::remove_file(session.dir.path().join("refuse")).unwrap();
    let calls = session.calls().len();
    assert!(session.hook("Stop", "thread-1", "").status.success());
    assert_eq!(
        session.calls().len(),
        calls,
        "the retired thread must not consume the end announcement"
    );
    assert!(
        session
            .hook("UserPromptSubmit", "thread-2", "")
            .status
            .success()
    );
    session.start("thread-3", "clear");
    assert_eq!(session.current(), "thread-3");
}

#[test]
fn a_clear_can_skip_a_successor_whose_start_report_failed() {
    let session = Session::new();
    session.start("thread-1", "startup");
    std::fs::write(session.dir.path().join("refuse"), "thread-2").unwrap();
    assert_eq!(
        session
            .hook("SessionStart", "thread-2", "clear")
            .status
            .code(),
        Some(1)
    );
    std::fs::remove_file(session.dir.path().join("refuse")).unwrap();
    session.start("thread-3", "clear");
    assert_eq!(session.current(), "thread-3");
}

#[test]
fn an_unannounced_thread_is_still_refused_and_launches_do_not_share_state() {
    let first = Session::new();
    first.start("thread-1", "startup");
    assert_eq!(
        first.hook("UserPromptSubmit", "thread-2", "").status.code(),
        Some(1)
    );
    assert_eq!(first.current(), "thread-1");
    let second = Session::new();
    second.start("thread-2", "clear");
    assert_eq!(second.current(), "thread-2");
    assert!(
        !second.calls()[0]
            .iter()
            .any(|word| word == "--conversation-ends")
    );
}

#[test]
fn damaged_or_missing_launch_state_fails_before_sending_a_report() {
    let session = Session::new();
    let path = session.dir.path().join("reports-state");
    std::fs::write(&path, "torn-write").unwrap();
    assert_eq!(
        session
            .hook("SessionStart", "thread-1", "startup")
            .status
            .code(),
        Some(1)
    );
    assert!(!session.dir.path().join("calls").exists());
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        session
            .hook("SessionStart", "thread-1", "startup")
            .status
            .code(),
        Some(1)
    );
    assert!(!session.dir.path().join("calls").exists());
}
