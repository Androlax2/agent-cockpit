//! Hyprland's game mode (config/hyprland/.config/hypr/game-mode.lua), run under lua5.4 with a
//! stand-in for Hyprland's `hl` API that records what it is asked to run.

mod common;
use common::sandbox::*;

use std::process::Command;

/// Loads game-mode.lua with a stand-in `hl`, opens `windows` ("class|title|pid" each), then runs
/// `after` (Lua); prints every command it dispatched, one per line.
fn play(windows: &[&str], after: &str) -> Vec<String> {
    let module = repo().join("config/hyprland/.config/hypr/game-mode.lua");
    let opened: Vec<String> = windows.iter().map(|window| {
        let mut parts = window.split('|');
        let (class, title, pid) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
        format!("open({{class = {class:?}, title = {title:?}, pid = {pid}}})")
    }).collect();
    let script = format!(r#"
local submap = "reset"
local handlers = {{}}
hl = {{
    config = function() end,
    dispatch = function(action) if action.exec then print(action.exec) end; if action.submap then submap = action.submap end end,
    dsp = {{ submap = function(name) return {{submap = name}} end, exec_cmd = function(command) return {{exec = command}} end }},
    get_current_submap = function() return submap end,
    on = function(event, handler) handlers[event] = handler end,
}}
local game_mode = dofile({module:?})
local function open(window) handlers["window.open"](window) end
{opened}
{after}
"#, module = module.display().to_string(), opened = opened.join("\n"));
    let output = Command::new("lua5.4").args(["-e", &script]).output().expect("lua5.4 runs the Hyprland config's Lua");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).lines().filter(|line| !line.starts_with("notify-send")).map(str::to_string).collect()
}

#[test]
fn a_game_window_registers_its_process_with_the_pid_attached() {
    // "gamemoded -r 4242" is -r without a PID: gamemoded registered itself and waited forever, so
    // GameMode, and game mode with it, never ended with the game.
    assert_eq!(play(&["steam_app_1086940|Baldur's Gate 3|4242"], ""), ["gamemoded --request=4242"]);
}

#[test]
fn a_games_second_window_does_not_register_its_process_again() {
    // --request toggles: a second request for the same process would unregister it.
    let dispatched = play(&["steam_app_1|Game|4242", "steam_app_1|Game launcher|4242", "retroarch|RetroArch|5151"], "");
    assert_eq!(dispatched, ["gamemoded --request=4242", "gamemoded --request=5151"]);
}

#[test]
fn a_launcher_or_another_window_registers_nothing() {
    assert!(play(&["steam_app_1|Battle.net|4242", "kitty|fish|77"], "").is_empty());
}

#[test]
fn the_end_hook_leaves_game_mode_and_a_new_game_registers_again() {
    let dispatched = play(&["steam_app_1|Game|4242"], r#"EXIT_GAME_MODE(); open({class = "steam_app_1", title = "Game", pid = 4242})"#);
    assert_eq!(dispatched, ["gamemoded --request=4242", "hyprctl reload", "gamemoded --request=4242"]);
}
