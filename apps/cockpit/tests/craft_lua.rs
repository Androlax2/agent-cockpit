//! The craft apps' Hyprland rules (config/hyprland/.config/hypr/apps/craft.lua), run under lua5.4
//! with a stand-in `hl` that prints every window move it is asked to dispatch.

mod common;
use common::sandbox::*;

use std::process::{Command, Output};

/// Loads craft.lua, then runs `actions` (Lua), where `open(class)` maps a window and `clock`
/// sets the seconds os.time() returns.
fn place(actions: &str) -> Output {
    let module = repo().join("config/hyprland/.config/hypr/apps/craft.lua");
    let script = format!(r#"
local handlers = {{}}
clock = 1000
os.time = function() return clock end
hl = {{
    window_rule = function() end,
    on = function(event, handler) handlers[event] = handler end,
    dsp = {{ window = {{ move = function(spec) return spec end }} }},
    dispatch = function(spec)
        print("move " .. spec.window.class .. " to " .. spec.workspace .. " follow=" .. tostring(spec.follow))
    end,
}}
dofile({module:?})
function open(class) handlers["window.open"]({{class = class}}) end
{actions}
"#, module = module.display().to_string());
    Command::new("lua5.4").args(["-e", &script]).output().expect("lua5.4 runs the Hyprland config's Lua")
}

fn moves(actions: &str) -> Vec<String> {
    let output = place(actions);
    assert!(output.status.success(), "{}", stderr(&output));
    stdout(&output).lines().map(str::to_string).collect()
}

#[test]
fn the_requested_figcraft_window_opens_on_its_workspace_without_following_it() {
    assert_eq!(moves(r#"FIGCRAFT_OPEN_ON_WORKSPACE(3); open("ai.storyteller.figcraft")"#),
        ["move ai.storyteller.figcraft to 3 follow=false"]);
}

#[test]
fn only_the_next_figcraft_window_is_moved() {
    let dispatched = moves(r#"FIGCRAFT_OPEN_ON_WORKSPACE(3); open("kitty"); open("ai.storyteller.wordcraft")
open("ai.storyteller.figcraft"); open("ai.storyteller.figcraft")"#);
    assert_eq!(dispatched, ["move ai.storyteller.figcraft to 3 follow=false"]);
}

#[test]
fn a_figcraft_window_without_a_request_stays_where_hyprland_puts_it() {
    assert!(moves(r#"open("ai.storyteller.figcraft")"#).is_empty());
}

#[test]
fn an_expired_request_moves_nothing() {
    assert!(moves(r#"FIGCRAFT_OPEN_ON_WORKSPACE(3); clock = clock + 31; open("ai.storyteller.figcraft")"#).is_empty());
}

#[test]
fn a_request_without_a_workspace_number_is_an_error() {
    for workspace in ["0", "2.5", "\"2\"", "nil"] {
        let output = place(&format!("FIGCRAFT_OPEN_ON_WORKSPACE({workspace})"));
        assert!(!output.status.success(), "{workspace}");
        assert!(stderr(&output).contains("not a workspace number"), "{workspace}: {}", stderr(&output));
    }
}
