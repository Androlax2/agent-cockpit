//! game-performance (config/bin/.local/bin/game-performance): what GameMode runs at a game's start
//! and end, against a fake systemctl.

mod common;
use common::sandbox::*;

use std::fs;

/// systemctl answers is-active from $SHIM_ACTIVE (a space-separated list of active units).
const FAKE_SYSTEMCTL: &str = r#"[[ $2 == is-active ]] && [[ " ${SHIM_ACTIVE:-} " == *" $4 "* ]] && exit 0
[[ $2 == is-active ]] && exit 3
exit 0"#;

fn sandbox() -> Sandbox {
    let sandbox = Sandbox::new();
    sandbox.shim("systemctl", FAKE_SYSTEMCTL);
    sandbox.shim("hyprctl", "");
    sandbox.shim("claude-sessions", "");
    // Two Claude sessions in their tmux scopes, one of them with two processes (launcher, session).
    sandbox.shim("ps", r#"printf '%s\n' "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-aaa.scope" \
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-aaa.scope" \
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-bbb.scope""#);
    sandbox
}

fn run(sandbox: &Sandbox, action: &str, active: &str) -> std::process::Output {
    sandbox.run(bin().join("game-performance"), &[action], &[("SHIM_ACTIVE", active)])
}

#[test]
fn a_game_start_pauses_only_the_timers_that_were_running() {
    let sandbox = sandbox();
    run(&sandbox, "on", "restic-backup.timer jobpilot-apply.timer");
    let stopped: Vec<String> = sandbox.calls("systemctl").into_iter().filter(|call| call.contains(" stop ")).collect();
    assert_eq!(stopped, ["systemctl --user stop restic-backup.timer", "systemctl --user stop jobpilot-apply.timer"]);
}

#[test]
fn a_game_start_leaves_the_sessions_running_while_freeze_while_gaming_is_off() {
    let sandbox = sandbox();
    sandbox.shim("claude-sessions", r#"[[ $1 == setting ]] && echo "$2: off"; exit 0"#);
    run(&sandbox, "on", "");
    assert!(sandbox.calls("systemctl").iter().all(|call| !call.contains(" freeze ")));
}

#[test]
fn a_game_start_pauses_each_claude_sessions_scope_once() {
    let sandbox = sandbox();
    run(&sandbox, "on", "");
    let frozen: Vec<String> = sandbox.calls("systemctl").into_iter().filter(|call| call.contains(" freeze ")).collect();
    assert_eq!(frozen, ["systemctl --user freeze tmux-spawn-aaa.scope", "systemctl --user freeze tmux-spawn-bbb.scope"]);
}

#[test]
fn a_game_start_drops_running_jobs_to_idle_priority() {
    let sandbox = sandbox();
    run(&sandbox, "on", "");
    assert!(sandbox.calls("systemctl").contains(&"systemctl --user set-property --runtime restic-backup.service CPUWeight=idle IOWeight=1".to_string()));
}

#[test]
fn a_game_end_resumes_the_sessions_it_paused() {
    let sandbox = sandbox();
    run(&sandbox, "on", "");
    sandbox.reset_log();
    run(&sandbox, "off", "");
    let thawed: Vec<String> = sandbox.calls("systemctl").into_iter().filter(|call| call.contains(" thaw ")).collect();
    assert_eq!(thawed, ["systemctl --user thaw tmux-spawn-aaa.scope", "systemctl --user thaw tmux-spawn-bbb.scope"]);
    assert!(!sandbox.runtime.join("game-performance.frozen").exists());
}

#[test]
fn a_game_end_restarts_what_it_paused_and_nothing_else() {
    let sandbox = sandbox();
    run(&sandbox, "on", "restic-backup.timer");
    sandbox.reset_log();
    run(&sandbox, "off", "");
    let started: Vec<String> = sandbox.calls("systemctl").into_iter().filter(|call| call.contains(" start ")).collect();
    assert_eq!(started, ["systemctl --user start restic-backup.timer"]);
    assert!(!sandbox.runtime.join("game-performance.paused").exists());
}

#[test]
fn a_game_end_puts_priorities_back_and_leaves_game_mode() {
    let sandbox = sandbox();
    run(&sandbox, "off", "");
    assert!(sandbox.calls("systemctl").contains(&"systemctl --user set-property --runtime restic-backup.service CPUWeight= IOWeight=".to_string()));
    assert_eq!(sandbox.calls("hyprctl"), ["hyprctl eval EXIT_GAME_MODE()"]);
}

#[test]
fn a_game_end_without_a_start_restarts_nothing() {
    let sandbox = sandbox();
    let output = run(&sandbox, "off", "");
    assert!(output.status.success());
    assert!(!sandbox.calls("systemctl").iter().any(|call| call.contains(" start ")));
    assert!(fs::read_dir(&sandbox.runtime).unwrap().next().is_none());
}

#[test]
fn an_unknown_action_is_refused() {
    let sandbox = sandbox();
    assert_eq!(run(&sandbox, "maybe", "").status.code(), Some(2));
}
