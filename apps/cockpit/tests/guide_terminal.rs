//! claude-guide in a real (pseudo-)terminal: typing, pasting and Enter as Theo would, a run: step
//! declined by default, and guides it refuses to show.

mod common;

use common::Terminal;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// A scratch guides folder holding one guide with these steps.
fn guide(steps: Value, fields: Value) -> (PathBuf, &'static str) {
    let root = std::env::temp_dir().join(format!("guide-test-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(root.join("guides")).unwrap();
    fs::create_dir_all(root.join("run")).unwrap();
    let id = "aaaaaaaaaaaa";
    let mut guide = json!({"id": id, "title": "Key", "cwd": root, "answers": "proj-a", "requested_at": now(), "steps": steps});
    for (key, value) in fields.as_object().unwrap() {
        guide[key] = value.clone();
    }
    fs::create_dir_all(root.join("run/claude-requests")).unwrap();
    fs::write(root.join(format!("run/claude-requests/{id}.guide")), guide.to_string()).unwrap();
    (root, id)
}

fn start(root: &Path, id: &str) -> Terminal {
    Terminal::start(Path::new(env!("CARGO_BIN_EXE_claude-guide")), &[id],
                    &[("CLAUDE_GUIDES_DIR", &root.join("guides")), ("CLAUDE_ANSWERS_DIR", &root.join("answers")), ("XDG_RUNTIME_DIR", &root.join("run")), ("PATH", Path::new("/usr/bin:/bin")),
                      ("XDG_STATE_HOME", &root.join("state")), ("COCKPIT_STATE_HOME", &root.join("state")), ("CLAUDE_AUDIT_JOURNAL", Path::new("off"))])
}

fn result(root: &Path, id: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(root.join(format!("answers/proj-a/guides/{id}.result"))).unwrap()).unwrap()
}

#[test]
fn typing_pasting_and_enter_go_through_the_real_screen() {
    let (root, id) = guide(json!([{"text": "Log in"}, {"text": "Answer", "ask": "Which email?"}]), json!({}));
    let mut terminal = start(&root, id);
    for keys in [&b"logged in"[..], b"\r", b"\x1b[200~a@b\nc\x1b[201~", b"\r"] {
        terminal.pump(0.6, None);
        terminal.type_keys(keys);
    }
    terminal.pump(3.0, Some(b"Every step done"));
    let _ = terminal.child.wait();
    let result = result(&root, id);
    assert_eq!(result["status"], "done");
    assert_eq!(result["notes"], json!([{"step": 1, "note": "logged in"}]));
    assert_eq!(result["answers"], json!([{"step": 2, "question": "Which email?", "answer": "a@b c"}]));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_run_step_is_declined_by_default() {
    let (root, id) = guide(json!([{"text": "Check", "run": "touch ran-it"}]), json!({}));
    let mut terminal = start(&root, id);
    terminal.pump(1.0, None);
    terminal.type_keys(b"\r");
    terminal.pump(5.0, Some(b"Run it?"));
    terminal.pump(0.7, None);
    terminal.type_keys(b"\r");
    terminal.pump(3.0, Some(b"Every step done"));
    let _ = terminal.child.wait();
    assert_eq!(result(&root, id)["runs"][0]["status"], "declined");
    assert!(!root.join("ran-it").exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unusable_guides_are_refused() {
    for (steps, fields, reason) in [
        (json!([{"text": "x"}]), json!({"requested_at": 1}), "older than two hours"),
        (json!([{"text": " "}]), json!({}), "a step without text"),
        (json!([{"text": "x", "run": ["rm", "-rf"]}]), json!({}), "a step field that is not text"),
        (json!([{"text": "x"}]), json!({"answers": "../../elsewhere"}), "names no answers folder"),
    ] {
        let (root, id) = guide(steps, fields);
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_claude-guide")).arg(id).env("XDG_RUNTIME_DIR", root.join("run")).env("CLAUDE_ANSWERS_DIR", root.join("answers")).env("XDG_STATE_HOME", root.join("state")).env("COCKPIT_STATE_HOME", root.join("state")).env("CLAUDE_AUDIT_JOURNAL", "off").output().unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains(reason), "{reason}");
        let _ = fs::remove_dir_all(root);
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_claude-guide")).arg("../etc").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("not a guide id"));
}
