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
    let request = serde_json::json!({"id": id, "command": command, "why": "A test.", "cwd": root, "answers": "proj-a", "requested_at": now});
    fs::write(root.join(format!("runs/{id}.request")), request.to_string()).unwrap();
    (root, id)
}

/// Runs the request: waits out the menu's half second, moves to "Run it", confirms.
fn approve_in_terminal(command: &str) -> (Terminal, PathBuf, String) {
    let (root, id) = request(command);
    let mut terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_claude-run-approve")), &[&id],
                                       &[("CLAUDE_RUNS_DIR", &root.join("runs")), ("CLAUDE_ANSWERS_DIR", &root.join("answers")), ("XDG_RUNTIME_DIR", &root.join("run"))]);
    terminal.pump(10.0, Some(b"Run it?"));
    terminal.pump(0.7, None);
    terminal.type_keys(b"\x1b[B");
    terminal.pump(0.3, None);
    terminal.type_keys(b"\r");
    (terminal, root, id)
}

#[test]
fn a_question_without_newline_stays_on_screen_until_answered() {
    let question = b"Install x? [y/N] ";
    let (mut terminal, root, id) = approve_in_terminal("sh -c 'printf \"Install x? [y/N] \"; read answer; echo \"got $answer\"'");
    terminal.pump(10.0, Some(question));
    assert!(contains(&terminal.screen, question));
    terminal.pump(2.0, None);
    let settled = terminal.screen.len();
    terminal.pump(1.0, None);
    let after_question = terminal.screen[last_index(&terminal.screen, question)..].to_vec();
    assert!(contains(&after_question, b"waiting for your answer above"));
    assert_eq!(terminal.screen.len(), settled, "nothing is drawn while the question waits");
    terminal.type_keys(b"y\n");
    terminal.pump(5.0, Some(b"got y"));
    terminal.pump(10.0, Some(b"Send the output"));
    terminal.pump(0.7, None);
    terminal.type_keys(b"\r");
    let _ = terminal.child.wait();
    assert!(contains(&terminal.screen, b"got y"));
    let result: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("answers/proj-a/runs/{id}.result"))).unwrap()).unwrap();
    assert_eq!((result["status"].as_str(), result["exit"].as_i64()), (Some("ran"), Some(0)));
    assert!(result["output"].as_str().unwrap().contains("got y"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_resize_moves_the_reserved_row_to_the_new_bottom() {
    let (mut terminal, root, _) = approve_in_terminal("sleep 1.5");
    terminal.pump(10.0, Some(b"running \xc2\xb7"));
    assert!(contains(&terminal.screen, b"\x1b[1;23r"));
    use std::os::fd::AsRawFd;
    set_size(terminal.master.as_raw_fd(), 40, 80);
    terminal.pump(0.6, None);
    let resized = terminal.screen[last_index(&terminal.screen, b"\x1b[1;23r")..].to_vec();
    assert!(contains(&resized, b"\x1b[1;39r"));
    assert!(contains(&resized, b"\x1b[40;1H"));
    terminal.pump(10.0, Some(b"Send the output"));
    terminal.pump(0.7, None);
    terminal.type_keys(b"\r");
    let _ = terminal.child.wait();
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_command_name_in_the_request_cannot_escape_its_folder() {
    let output = Command::new(env!("CARGO_BIN_EXE_claude-run-approve")).arg("../../etc").env("TMUX", "1").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("not a request id"));
}
