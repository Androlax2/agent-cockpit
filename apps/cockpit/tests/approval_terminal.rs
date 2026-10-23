//! cockpit-run-approve in a real (pseudo-)terminal: the arrow-key menu, the progress line, a
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

#[test]
fn a_bound_run_shows_the_exact_binding_and_never_publishes_an_unverified_checkout() {
    let (root,id) = request("true");
    let path = root.join(format!("run/claude-requests/{id}.request"));
    let mut held: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    held["evidence"] = serde_json::json!({"assignment":"assignment-123","gate":"tests"});
    fs::write(path,held.to_string()).unwrap();
    let mut terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_cockpit-run-approve")), &[&id],
        &[("CLAUDE_ANSWERS_DIR",&root.join("answers")),("XDG_RUNTIME_DIR",&root.join("run")),
            ("XDG_STATE_HOME",&root.join("state")),("COCKPIT_STATE_HOME",&root.join("state")),("CLAUDE_AUDIT_JOURNAL",Path::new("off"))]);
    terminal.pump(10.0,Some(b"Run it?"));
    assert!(contains(&terminal.screen,b"assignment-123"));
    assert!(contains(&terminal.screen,b"Evidence unavailable"));
    let asking: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("run/cockpit-runs/{id}.asking"))).unwrap()).unwrap();
    assert!(asking["why"].as_str().unwrap().contains("assignment-123"),"phone approval must display the binding too");
    fs::write(root.join(format!("run/cockpit-runs/{id}.remote")),"n").unwrap();
    terminal.pump(5.0,Some(b"Declined"));
    let _ = terminal.child.wait();
    let _ = fs::remove_dir_all(root);
}

/// Runs the request: waits out the menu's half second, moves to "Run it", confirms. The wait
/// starts once the last option is drawn (the menu ignores keys from then on, not from the
/// question line), and Enter waits for "Run it" to show selected: on a busy machine a Down
/// typed too early is dropped, and Enter would decline.
fn approve_in_terminal(command: &str) -> (Terminal, PathBuf, String) {
    let (root, id) = request(command);
    let mut terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_cockpit-run-approve")), &[&id],
                                       &[("CLAUDE_RUNS_DIR", &root.join("runs")), ("CLAUDE_ANSWERS_DIR", &root.join("answers")), ("XDG_RUNTIME_DIR", &root.join("run")),
                                          ("XDG_STATE_HOME", &root.join("state")), ("COCKPIT_STATE_HOME", &root.join("state")), ("CLAUDE_AUDIT_JOURNAL", Path::new("off")),
                                          ("SHELL", Path::new("/usr/bin/fish"))]);
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
fn a_pending_approval_keeps_its_menu_and_answer_through_path_migration() {
    let (root, id) = request("true");
    fs::create_dir_all(root.join("home")).unwrap();
    fs::create_dir_all(root.join("run/claude-runs")).unwrap();
    let mut terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_cockpit-run-approve")), &[&id],
        &[("HOME", &root.join("home")), ("CLAUDE_ANSWERS_DIR", &root.join("answers")),
          ("XDG_RUNTIME_DIR", &root.join("run")), ("XDG_STATE_HOME", &root.join("state")),
          ("COCKPIT_STATE_HOME", &root.join("state")), ("CLAUDE_AUDIT_JOURNAL", Path::new("off"))]);
    terminal.pump(10.0, Some(b"Edit it first"));
    let asking = root.join(format!("run/claude-runs/{id}.asking"));
    let before = fs::read(&asking).unwrap();
    let migration = Command::new("python3").arg(common::sandbox::repo().join("tools/migrate-cockpit-names.py")).arg("--apply")
        .env("HOME", root.join("home")).env("XDG_RUNTIME_DIR", root.join("run"))
        .env("XDG_STATE_HOME", root.join("state")).env("COCKPIT_STATE_HOME", root.join("state"))
        .env("XDG_DATA_HOME", root.join("home/.local/share")).output().unwrap();
    assert!(migration.status.success(), "{}", String::from_utf8_lossy(&migration.stderr));
    assert_eq!(fs::read(&asking).unwrap(), before);
    assert_eq!(fs::read(root.join(format!("run/cockpit-runs/{id}.asking"))).unwrap(), before);
    fs::write(root.join(format!("run/cockpit-runs/{id}.remote")), "n").unwrap();
    assert!(terminal.wait_for_exit(20.0).success());
    let result: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("answers/proj-a/runs/{id}.result"))).unwrap()).unwrap();
    assert_eq!(result["status"], "declined");
    assert!(!asking.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn approved_command_keeps_shell_quoting_under_fish() {
    let (root, _) = request("true");
    let marker = root.join("ran");
    let command = format!("echo 'a\\\\' '; touch {} #'", marker.display());
    let (mut terminal, approval_root, id) = approve_in_terminal(&command);
    assert!(terminal.wait_for_exit(20.0).success());
    let result: serde_json::Value = serde_json::from_str(&fs::read_to_string(approval_root.join(format!("answers/proj-a/runs/{id}.result"))).unwrap()).unwrap();
    assert_eq!(result["exit"], 0, "{result}");
    assert!(!marker.exists(), "the second word ran as a command");
    fs::remove_dir_all(approval_root).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn approved_command_reports_its_last_failed_step() {
    let (mut terminal, root, id) = approve_in_terminal("echo one; and false; and echo three");
    assert!(terminal.wait_for_exit(20.0).success());
    let result: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("answers/proj-a/runs/{id}.result"))).unwrap()).unwrap();
    assert_eq!((result["exit"].as_i64(), result["failed_step"].as_str()), (Some(1), Some("false")));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn approval_tests_preserve_the_callers_terminal_when_commands_overlap() {
    use common::sandbox::write_executable;
    use std::os::fd::AsRawFd;

    let (root, _) = request("true");
    let commands = root.join("bin");
    fs::create_dir(&commands).unwrap();
    // Force the first script to exit before the second, which saved its raw terminal settings.
    write_executable(&commands.join("script"), r#"#!/bin/sh
case "$*" in
  *"echo one"*)
    timeout 10 sh -c 'until [ -e "$APPROVAL_TIMING_ROOT/first" ]; do sleep 0.01; done' || exit 1
    exec /usr/bin/script "$@";;
  *)
    /usr/bin/script "$@"
    status=$?
    touch "$APPROVAL_TIMING_ROOT/first-finished"
    exit "$status";;
esac
"#);
    write_executable(&commands.join("fish"), r#"#!/bin/sh
case "$*" in
  *"echo one"*) touch "$APPROVAL_TIMING_ROOT/second"; timeout 10 sh -c 'until [ -e "$APPROVAL_TIMING_ROOT/first-finished" ]; do sleep 0.01; done' || exit 1;;
  *) touch "$APPROVAL_TIMING_ROOT/first"; timeout 10 sh -c 'until [ -e "$APPROVAL_TIMING_ROOT/second" ]; do sleep 0.01; done' || exit 1;;
esac
exec /usr/bin/fish "$@"
"#);
    let path = format!("{}:{}", commands.display(), std::env::var("PATH").unwrap());
    let mut terminal = Terminal::start(&std::env::current_exe().unwrap(), &["approved_command_", "--test-threads=2"],
        &[("PATH", Path::new(&path)), ("SHELL", Path::new("/usr/bin/fish")), ("APPROVAL_TIMING_ROOT", &root)]);
    let status = terminal.wait_for_exit(30.0);
    assert!(status.success(), "{}", String::from_utf8_lossy(&terminal.screen));
    assert!(contains(&terminal.screen, b"2 passed; 0 failed"), "the overlapping command tests did not both run");
    let mut settings = std::mem::MaybeUninit::<libc::termios>::uninit();
    assert_eq!(unsafe { libc::tcgetattr(terminal.master.as_raw_fd(), settings.as_mut_ptr()) }, 0);
    let settings = unsafe { settings.assume_init() };
    assert_ne!(settings.c_iflag & libc::ICRNL, 0, "Enter no longer produces a newline");
    assert_ne!(settings.c_oflag & libc::OPOST, 0, "output is left stair-stepped");
    assert_eq!(settings.c_lflag & (libc::ICANON | libc::ECHO), libc::ICANON | libc::ECHO, "line editing or echo was disabled");
    fs::remove_dir_all(root).unwrap();
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
    terminal.type_keys(b"y\r");
    terminal.wait_for_exit(20.0);
    assert!(contains(&terminal.screen, b"got y"));
    let result: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("answers/proj-a/runs/{id}.result"))).unwrap()).unwrap();
    assert_eq!((result["status"].as_str(), result["exit"].as_i64()), (Some("ran"), Some(0)));
    assert!(result["output"].as_str().unwrap().contains("got y"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_waiting_question_is_left_for_the_phone_until_it_is_answered() {
    let command = "sh -c 'printf \"Apply these 1 commit(s)? [y/N] \"; read answer; echo \"got $answer\"'";
    let (mut terminal, root, id) = approve_in_terminal(command);
    terminal.pump(10.0, Some(b"waiting for your answer above"));
    let input_path = root.join(format!("run/cockpit-runs/{id}.input"));
    let input: serde_json::Value = serde_json::from_str(&fs::read_to_string(&input_path).unwrap()).unwrap();
    assert_eq!(input["prompt"], "Apply these 1 commit(s)? [y/N]");
    assert_eq!(input["command"], command);
    terminal.type_keys(b"y\n");
    terminal.wait_for_exit(20.0);
    assert!(!input_path.exists(), "the question stays offered after its answer");
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
    let output = Command::new(env!("CARGO_BIN_EXE_cockpit-run-approve")).arg("../../etc").env("TMUX", "1").output().unwrap();
    assert!(String::from_utf8_lossy(&output.stdout).contains("not a request id"));
}

#[test]
fn the_phone_page_answers_the_menu_and_the_command_runs() {
    // claude-remote writes <id>.remote while <id>.asking says the menu waits: as if Run were chosen here.
    let (root, id) = request("sh -c 'echo ran-from-the-phone'");
    let mut terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_cockpit-run-approve")), &[&id],
                                       &[("CLAUDE_ANSWERS_DIR", &root.join("answers")), ("XDG_RUNTIME_DIR", &root.join("run")),
                                         ("XDG_STATE_HOME", &root.join("state")), ("COCKPIT_STATE_HOME", &root.join("state")), ("CLAUDE_AUDIT_JOURNAL", Path::new("off"))]);
    terminal.pump(10.0, Some(b"Run it?"));
    let asking: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("run/cockpit-runs/{id}.asking"))).unwrap()).unwrap();
    assert_eq!(asking["command"].as_str(), Some("sh -c 'echo ran-from-the-phone'"));
    assert_eq!(asking["session"], "aaaaaaaa-0000-4000-8000-000000000000", "the phone must associate the command with its conversation");
    fs::write(root.join(format!("run/cockpit-runs/{id}.remote")), "y").unwrap();
    terminal.wait_for_exit(20.0);
    let result: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("answers/proj-a/runs/{id}.result"))).unwrap()).unwrap();
    assert!(result["output"].as_str().unwrap().contains("ran-from-the-phone"), "{result}");
    assert!(!root.join(format!("run/cockpit-runs/{id}.asking")).exists(), "still asking after the answer");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unverified_session_claims_do_not_associate_commands_with_a_mobile_chat() {
    let (root, id) = request("true");
    let path = root.join(format!("run/claude-requests/{id}.request"));
    let mut held: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    held.as_object_mut().unwrap().remove("verified_session");
    fs::write(path, held.to_string()).unwrap();
    let mut terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_cockpit-run-approve")), &[&id],
        &[("CLAUDE_ANSWERS_DIR", &root.join("answers")), ("XDG_RUNTIME_DIR", &root.join("run")),
            ("XDG_STATE_HOME", &root.join("state")), ("COCKPIT_STATE_HOME", &root.join("state")), ("CLAUDE_AUDIT_JOURNAL", Path::new("off"))]);
    terminal.pump(10.0, Some(b"Run it?"));
    let asking: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join(format!("run/cockpit-runs/{id}.asking"))).unwrap()).unwrap();
    assert_eq!(asking["session"], "");
    fs::write(root.join(format!("run/cockpit-runs/{id}.remote")), "n").unwrap();
    // Keep draining the PTY and bound the wait even if a loaded runner misses the decline.
    let status = terminal.wait_for_exit(20.0);
    assert!(status.success(), "approval exited with {status}");
    assert!(contains(&terminal.screen, b"Declined: Claude is told."), "{}", String::from_utf8_lossy(&terminal.screen));
    let _ = fs::remove_dir_all(root);
}
