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
    // Three greywall sandboxes: two in tmux scopes, one of those with two processes, and one in a
    // kitty window's scope.
    sandbox.shim("ps", r#"printf '%s\n' "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-aaa.scope" \
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-aaa.scope" \
        "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-bbb.scope" \
        "0::/user.slice/user-1000.slice/user@1000.service/kitty-526640-0.scope""#);
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
fn a_game_start_pauses_each_sandboxs_scope_once() {
    let sandbox = sandbox();
    run(&sandbox, "on", "");
    let frozen: Vec<String> = sandbox.calls("systemctl").into_iter().filter(|call| call.contains(" freeze ")).collect();
    assert_eq!(frozen, [
        "systemctl --user freeze kitty-526640-0.scope",
        "systemctl --user freeze tmux-spawn-aaa.scope",
        "systemctl --user freeze tmux-spawn-bbb.scope",
    ]);
}

/// Every agent runs under greywall: looking for greywall, not one agent's binary, freezes them all.
#[test]
fn a_game_start_looks_for_the_greywall_sandboxes_not_one_agent() {
    let sandbox = sandbox();
    run(&sandbox, "on", "");
    assert_eq!(sandbox.calls("ps"), ["ps -C greywall -o cgroup="]);
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
    assert_eq!(thawed, [
        "systemctl --user thaw kitty-526640-0.scope",
        "systemctl --user thaw tmux-spawn-aaa.scope",
        "systemctl --user thaw tmux-spawn-bbb.scope",
    ]);
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
fn a_game_end_tells_the_running_hyprland_when_gamemodes_signature_is_a_past_ones() {
    // GameMode's service outlives a Hyprland restart and keeps the old instance's signature.
    let sandbox = sandbox();
    sandbox.shim("hyprctl", r#"echo "hyprctl-instance $HYPRLAND_INSTANCE_SIGNATURE" >> "$SHIM_LOG""#);
    fs::create_dir_all(sandbox.runtime.join("hypr/live_instance")).unwrap();
    fs::write(sandbox.runtime.join("hypr/live_instance/.socket.sock"), "").unwrap();
    sandbox.run(bin().join("game-performance"), &["off"], &[("HYPRLAND_INSTANCE_SIGNATURE", "gone_instance")]);
    assert_eq!(sandbox.calls("hyprctl-instance"), ["hyprctl-instance live_instance"]);
}

#[test]
fn a_game_end_that_cannot_reach_hyprland_says_so_in_the_log() {
    let sandbox = sandbox();
    sandbox.shim("hyprctl", "exit 1");
    run(&sandbox, "off", "");
    assert!(sandbox.calls("claude-sessions").iter().any(|call| call.contains("game WARN") && call.contains("could not be told to leave game mode")),
            "{:?}", sandbox.calls("claude-sessions"));
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
