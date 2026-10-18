//! The Claude workspace's idle prompt (hypr/scripts/claude-idle.sh) around its screen.

mod common;
use common::sandbox::*;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::{Child, Output};
use std::time::{Duration, Instant};

fn script() -> PathBuf {
    repo().join("config/hyprland/.config/hypr/scripts/claude-idle.sh")
}

struct IdleTest {
    sandbox: Sandbox,
    prompt: PathBuf,
}

impl IdleTest {
    fn new() -> IdleTest {
        let sandbox = Sandbox::new();
        // The script finds its neighbours through $HOME (the sandbox already links the bar's); the
        // screen itself (GTK) is a shim whose exit code plays the answer or the timeout.
        let scripts = sandbox.home.join(".config/hypr/scripts");
        fs::create_dir_all(&scripts).unwrap();
        symlink(repo().join("config/hyprland/.config/hypr/scripts/is-gaming.sh"), scripts.join("is-gaming.sh")).unwrap();
        let prompt = sandbox.home.join(".local/bin/claude-idle-prompt");
        sandbox.shim("loginctl", "");
        sandbox.shim("pidof", "exit 1");
        IdleTest { sandbox, prompt }
    }

    fn screen(&self, body: &str) {
        write_executable(&self.prompt, &format!("#!/bin/sh\n{body}\n"));
    }

    fn on_workspace(&self, workspace: u32) {
        self.sandbox.shim(
            "hyprctl",
            &format!(
                r#"[[ $* == *activeworkspace* ]] && echo '{{"id": {workspace}}}'; [[ $* == *activewindow* ]] && echo '{{"class": "kitty", "fullscreen": 0}}'; exit 0"#
            ),
        );
    }

    fn run_script(&self, action: &str) -> Output {
        self.sandbox.run(
            "bash",
            &[script().to_str().unwrap(), action],
            &[("CLAUDE_IDLE_GRACE_S", "1"), ("CLAUDE_IDLE_SCREEN_OFF_AFTER_S", "0")],
        )
    }

    fn shown(&self) -> PathBuf {
        self.sandbox.home.join("shown")
    }
}

/// Kills the background run if the test fails before it ends.
struct Background(Child);

impl Drop for Background {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Background {
    fn wait(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "still running after {timeout:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[test]
fn elsewhere_the_prompt_does_nothing() {
    let test = IdleTest::new();
    test.on_workspace(2);
    test.screen(r#"echo "shown $1" > "$HOME/shown"; exit 1"#);
    test.run_script("prompt");
    assert!(!test.shown().exists());
    assert_eq!(test.sandbox.calls("loginctl"), Vec::<String>::new());
}

#[test]
fn on_the_claude_workspace_an_unanswered_screen_locks() {
    let test = IdleTest::new();
    test.on_workspace(6);
    test.screen(r#"echo "shown $1" > "$HOME/shown"; exit 1"#);
    test.run_script("prompt");
    assert_eq!(fs::read_to_string(test.shown()).unwrap().trim(), "shown 1");
    assert_eq!(test.sandbox.calls("loginctl"), ["loginctl lock-session"]);
}

#[test]
fn an_answered_screen_keeps_the_session_unlocked() {
    let test = IdleTest::new();
    test.on_workspace(6);
    test.screen("exit 0");
    assert_eq!(test.run_script("prompt").status.code(), Some(0));
    assert_eq!(test.sandbox.calls("loginctl"), Vec::<String>::new());
}

#[test]
fn resumed_activity_closes_the_screen_and_forgets_the_lock() {
    let test = IdleTest::new();
    test.on_workspace(6);
    test.screen("trap 'exit 0' TERM; sleep 10 & wait $!; exit 1");
    let child = test
        .sandbox
        .command("bash", &[("CLAUDE_IDLE_GRACE_S", "10")])
        .args([script().to_str().unwrap(), "prompt"])
        .spawn()
        .unwrap();
    let mut pending = Background(child);
    std::thread::sleep(Duration::from_millis(500));
    test.run_script("cancel");
    assert_eq!(pending.wait(Duration::from_secs(5)).code(), Some(0));
    assert_eq!(test.sandbox.calls("loginctl"), Vec::<String>::new());
    assert!(!test.sandbox.runtime.join("claude-idle.pid").exists());
}
