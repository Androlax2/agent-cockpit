//! claude-run-approve in a real (pseudo-)terminal: the arrow-key menu, the progress line, a
//! question that waits for an answer, and a resize.

mod common;

use common::{Terminal, contains, last_index, set_size};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// A scratch folder with a request for `command`, and the environment pointing at it.
fn request(command: &str) -> (PathBuf, String) {
    let root = std::env::temp_dir().join(format!("approval-test-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(root.join("runs")).unwrap();
    fs::create_dir_all(root.join("run")).unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let id = "0123456789ab".to_string();
    // As the daemon holds it (approval::held_request), with the folder the session started in.
    let request = serde_json::json!({"id": id, "command": command, "why": "A test.", "cwd": root, "started_in": root, "project": "proj-a",
                                      "answers": "proj-a", "requested_at": now, "session": "aaaaaaaa-0000-4000-8000-000000000000",
                                      "verified_session": "aaaaaaaa-0000-4000-8000-000000000000"});
    fs::create_dir_all(root.join("run/claude-requests")).unwrap();
    fs::write(root.join(format!("run/claude-requests/{id}.request")), request.to_string()).unwrap();
    (root, id)
}

/// Runs the request: waits out the menu's half second, moves to "Run it", confirms. The wait
/// starts once the last option is drawn (the menu ignores keys from then on, not from the
/// question line), and Enter waits for "Run it" to show selected: on a busy machine a Down
/// typed too early is dropped, and Enter would decline.
fn approve_in_terminal(command: &str) -> (Terminal, PathBuf, String) {
    let (root, id) = request(command);
    let mut terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_claude-run-approve")), &[&id],
                                       &[("CLAUDE_RUNS_DIR", &root.join("runs")), ("CLAUDE_ANSWERS_DIR", &root.join("answers")), ("XDG_RUNTIME_DIR", &root.join("run")),
                                         ("XDG_STATE_HOME", &root.join("state")), ("COCKPIT_STATE_HOME", &root.join("state")), ("CLAUDE_AUDIT_JOURNAL", Path::new("off"))]);
    terminal.pump(10.0, Some(b"Edit it first"));
    assert!(contains(&terminal.screen, b"Edit it first"), "the menu never showed");
    terminal.pump(1.0, None);
    terminal.type_keys(b"\x1b[B");
    let run_it_selected = "❯ \x1b[1mRun it".as_bytes();
    terminal.pump(10.0, Some(run_it_selected));
    assert!(contains(&terminal.screen, run_it_selected), "Down did not select Run it");
    terminal.type_keys(b"\r");
    (terminal, root, id)
}

#[test]
fn a_question_without_newline_stays_on_screen_until_answered() {
    let question = b"Install x? [y/N] ";
    let (mut terminal, root, id) = approve_in_terminal("sh -c 'printf \"Install x? [y/N] \"; read answer; echo \"got $answer\"'");
    let waiting = b"waiting for your answer above";
    terminal.pump(10.0, Some(question));
    assert!(contains(&terminal.screen, question));
    terminal.pump(10.0, Some(waiting));
    let after_question = terminal.screen[last_index(&terminal.screen, question)..].to_vec();
    assert!(contains(&after_question, waiting));
    terminal.pump(0.3, None);
    let settled = terminal.screen.len();
    terminal.pump(1.0, None);
    assert_eq!(terminal.screen.len(), settled, "nothing is drawn while the question waits");
    terminal.type_keys(b"y\n");
    terminal.wait_for_exit(20.0);
    assert!(contains(&terminal.screen, b"got y"));
    let result: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("answers/proj-a/runs/{id}.result"))).unwrap()).unwrap();
    assert_eq!((result["status"].as_str(), result["exit"].as_i64()), (Some("ran"), Some(0)));
    assert!(result["output"].as_str().unwrap().contains("got y"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_resize_moves_the_reserved_row_to_the_new_bottom() {
    // The command runs until the test has seen the resize: a fixed sleep could end first on a busy machine.
    let (mut terminal, root, _) = approve_in_terminal("sh -c 'until [ -e resized ]; do sleep 0.1; done'");
    terminal.pump(10.0, Some(b"running \xc2\xb7"));
    assert!(contains(&terminal.screen, b"\x1b[1;23r"));
    use std::os::fd::AsRawFd;
    set_size(terminal.master.as_raw_fd(), 40, 80);
    terminal.pump(10.0, Some(b"\x1b[40;1H"));
    let resized = terminal.screen[last_index(&terminal.screen, b"\x1b[1;23r")..].to_vec();
    assert!(contains(&resized, b"\x1b[1;39r"));
    assert!(contains(&resized, b"\x1b[40;1H"));
    fs::write(root.join("resized"), "").unwrap();
    terminal.wait_for_exit(20.0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_command_gets_cat_as_its_pager() {
    let (mut terminal, root, _) = approve_in_terminal("sh -c 'echo \"pager=$PAGER\"'");
    terminal.wait_for_exit(20.0);
    assert!(contains(&terminal.screen, b"pager=cat"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_command_name_in_the_request_cannot_escape_its_folder() {
    let output = Command::new(env!("CARGO_BIN_EXE_claude-run-approve")).arg("../../etc").env("TMUX", "1").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("not a request id"));
}

#[test]
fn the_phone_page_answers_the_menu_and_the_command_runs() {
    // claude-remote writes <id>.remote while <id>.asking says the menu waits: as if Run were chosen here.
    let (root, id) = request("sh -c 'echo ran-from-the-phone'");
    let mut terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_claude-run-approve")), &[&id],
                                       &[("CLAUDE_ANSWERS_DIR", &root.join("answers")), ("XDG_RUNTIME_DIR", &root.join("run")),
                                         ("XDG_STATE_HOME", &root.join("state")), ("COCKPIT_STATE_HOME", &root.join("state")), ("CLAUDE_AUDIT_JOURNAL", Path::new("off"))]);
    terminal.pump(10.0, Some(b"Run it?"));
    let asking: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("run/claude-runs/{id}.asking"))).unwrap()).unwrap();
    assert_eq!(asking["command"].as_str(), Some("sh -c 'echo ran-from-the-phone'"));
    assert_eq!(asking["session"], "aaaaaaaa-0000-4000-8000-000000000000", "the phone must associate the command with its conversation");
    fs::write(root.join(format!("run/claude-runs/{id}.remote")), "y").unwrap();
    terminal.wait_for_exit(20.0);
    let result: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("answers/proj-a/runs/{id}.result"))).unwrap()).unwrap();
    assert!(result["output"].as_str().unwrap().contains("ran-from-the-phone"), "{result}");
    assert!(!root.join(format!("run/claude-runs/{id}.asking")).exists(), "still asking after the answer");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unverified_session_claims_do_not_associate_commands_with_a_mobile_chat() {
    let (root, id) = request("true");
    let path = root.join(format!("run/claude-requests/{id}.request"));
    let mut held: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    held.as_object_mut().unwrap().remove("verified_session");
    fs::write(path, held.to_string()).unwrap();
    let mut terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_claude-run-approve")), &[&id],
        &[("CLAUDE_ANSWERS_DIR", &root.join("answers")), ("XDG_RUNTIME_DIR", &root.join("run")),
            ("XDG_STATE_HOME", &root.join("state")), ("COCKPIT_STATE_HOME", &root.join("state")), ("CLAUDE_AUDIT_JOURNAL", Path::new("off"))]);
    terminal.pump(10.0, Some(b"Run it?"));
    let asking: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("run/claude-runs/{id}.asking"))).unwrap()).unwrap();
    assert_eq!(asking["session"], "");
    fs::write(root.join(format!("run/claude-runs/{id}.remote")), "n").unwrap();
    terminal.pump(5.0, Some(b"Declined"));
    let _ = terminal.child.wait();
    let _ = fs::remove_dir_all(root);
}
