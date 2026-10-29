//! The craft apps' Hyprland rules (config/hyprland/.config/hypr/apps/craft.lua), run under lua5.4
//! with a stand-in `hl` that prints every window move it is asked to dispatch.

mod common;
use common::sandbox::*;

use std::process::{Command, Output};

/// Loads craft.lua, then runs `actions` (Lua), where `open(class)` maps a window, `clock` sets
/// the seconds os.time() returns, `windows` lists the mapped windows hl.get_windows() returns and
/// `tile(address, workspace)` adds a FigCraft window there. Each window gets a process of its own
/// unless `open(class, pid)` names one.
fn place(actions: &str) -> Output {
    let module = repo().join("config/hyprland/.config/hypr/apps/craft.lua");
    let script = format!(
        r#"
local handlers = {{}}
clock = 1000
os.time = function() return clock end
windows = {{}}
local function group_of(window)
    return {{
        current = window, locked = false, denied = false,
        add = function(self, added) print("tab " .. added.address .. " with " .. self.current.address) end,
    }}
end
hl = {{
    window_rule = function() end,
    on = function(event, handler) handlers[event] = handler end,
    get_windows = function() return windows end,
    notification = {{ create = function(spec) print("notify " .. spec.text) end }},
    dsp = {{
        window = {{ move = function(spec) spec.kind = "move"; return spec end }},
        group = {{ toggle = function(spec) spec.kind = "group"; return spec end }},
    }},
    dispatch = function(spec)
        if spec.kind == "group" then
            print("group " .. spec.window.address)
            spec.window.group = group_of(spec.window)
            return {{ ok = true }}
        end
        print("move " .. spec.window.class .. " to " .. spec.workspace .. " follow=" .. tostring(spec.follow))
        for _, window in ipairs(windows) do
            if window.address == spec.window.address then window.workspace = {{ id = tonumber(spec.workspace) }} end
        end
        return {{ ok = true }}
    end,
}}
dofile({module:?})
local next_address, next_pid = 0, 100
local function new_pid()
    next_pid = next_pid + 1
    return next_pid
end
function tile(address, workspace)
    local window = {{ class = "ai.storyteller.figcraft", address = address, workspace = {{ id = workspace }}, floating = false, pid = new_pid() }}
    table.insert(windows, window)
    return window
end
function open(class, pid)
    next_address = next_address + 1
    local window = {{ class = class, address = "new" .. next_address, workspace = {{ id = 6 }}, floating = false, pid = pid or new_pid() }}
    table.insert(windows, window)
    handlers["window.open"](window)
end
{actions}
"#,
        module = module.display().to_string()
    );
    Command::new("lua5.4")
        .args(["-e", &script])
        .output()
        .expect("lua5.4 runs the Hyprland config's Lua")
}

fn moves(actions: &str) -> Vec<String> {
    let output = place(actions);
    assert!(output.status.success(), "{}", stderr(&output));
    stdout(&output).lines().map(str::to_string).collect()
}

const OPEN_ON_3: &str = r#"FIGCRAFT_OPEN_ON_WORKSPACE(3); open("ai.storyteller.figcraft")"#;
const MOVED_TO_3: &str = "move ai.storyteller.figcraft to 3 follow=false";

#[test]
fn the_requested_figcraft_window_opens_on_its_workspace_without_following_it() {
    assert_eq!(
        moves(r#"FIGCRAFT_OPEN_ON_WORKSPACE(3); open("ai.storyteller.figcraft")"#),
        ["move ai.storyteller.figcraft to 3 follow=false"]
    );
}

#[test]
fn a_request_moves_only_the_next_figcraft_window() {
    let dispatched = moves(
        r#"FIGCRAFT_OPEN_ON_WORKSPACE(3); open("kitty"); open("ai.storyteller.wordcraft")
open("ai.storyteller.figcraft"); open("ai.storyteller.figcraft")"#,
    );
    assert_eq!(
        dispatched,
        ["move ai.storyteller.figcraft to 3 follow=false", MOVED_TO_7]
    );
}

const MOVED_TO_7: &str = "move ai.storyteller.figcraft to 7 follow=false";

#[test]
fn a_directly_launched_figcraft_opens_on_the_shared_workspace_7() {
    assert_eq!(moves(r#"open("ai.storyteller.figcraft")"#), [MOVED_TO_7]);
}

#[test]
fn another_window_of_an_open_figcraft_stays_where_hyprland_puts_it() {
    assert!(
        moves(r#"tile("opusline", 3).pid = 41; open("ai.storyteller.figcraft", 41)"#).is_empty()
    );
}

#[test]
fn a_directly_launched_figcraft_joins_the_one_on_workspace_7_as_a_tab() {
    assert_eq!(
        moves(r#"tile("opusline", 7); open("ai.storyteller.figcraft")"#),
        [MOVED_TO_7, "group opusline", "tab new1 with opusline"]
    );
}

#[test]
fn an_expired_request_leaves_the_window_to_the_shared_workspace() {
    assert_eq!(
        moves(
            r#"FIGCRAFT_OPEN_ON_WORKSPACE(3); clock = clock + 31; open("ai.storyteller.figcraft")"#
        ),
        [MOVED_TO_7]
    );
}

#[test]
fn a_request_without_a_workspace_number_is_an_error() {
    for workspace in ["0", "2.5", "\"2\"", "nil"] {
        let output = place(&format!("FIGCRAFT_OPEN_ON_WORKSPACE({workspace})"));
        assert!(!output.status.success(), "{workspace}");
        assert!(
            stderr(&output).contains("not a workspace number"),
            "{workspace}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn a_new_figcraft_joins_the_one_already_on_its_workspace_as_a_tab() {
    assert_eq!(
        moves(&format!(r#"tile("opusline", 3); {OPEN_ON_3}"#)),
        [MOVED_TO_3, "group opusline", "tab new1 with opusline"]
    );
}

#[test]
fn a_new_figcraft_joins_an_existing_tab_group_without_regrouping_it() {
    let dispatched = moves(&format!(
        r#"
local opusline, jobpilot = tile("opusline", 3), tile("jobpilot", 3)
local group = group_of(opusline)
opusline.group, jobpilot.group = group, group
{OPEN_ON_3}"#
    ));
    assert_eq!(dispatched, [MOVED_TO_3, "tab new1 with opusline"]);
}

#[test]
fn figcraft_windows_elsewhere_or_floating_and_other_apps_get_no_tab() {
    let dispatched = moves(&format!(
        r#"
tile("other-workspace", 2)
tile("floating", 3).floating = true
table.insert(windows, {{ class = "ai.storyteller.wordcraft", address = "wordcraft", workspace = {{ id = 3 }}, floating = false }})
{OPEN_ON_3}"#
    ));
    assert_eq!(dispatched, [MOVED_TO_3]);
}

#[test]
fn separate_figcraft_tiles_leave_the_new_window_beside_them() {
    let dispatched = moves(&format!(
        r#"tile("opusline", 3); tile("jobpilot", 3); {OPEN_ON_3}"#
    ));
    assert_eq!(
        dispatched,
        [
            MOVED_TO_3,
            "notify FigCraft opened beside the others: several FigCraft windows already share workspace 3"
        ]
    );
}

#[test]
fn a_locked_tab_group_takes_no_new_tab() {
    let dispatched = moves(&format!(
        r#"
local opusline = tile("opusline", 3)
opusline.group = {{ current = opusline, locked = true, denied = false, add = function() print("tab added") end }}
{OPEN_ON_3}"#
    ));
    assert_eq!(
        dispatched,
        [
            MOVED_TO_3,
            "notify FigCraft opened beside the others: the FigCraft tabs on workspace 3 are locked"
        ]
    );
}

#[test]
fn a_figcraft_opened_without_a_request_joins_no_tab_elsewhere() {
    assert_eq!(
        moves(r#"tile("opusline", 3); open("ai.storyteller.figcraft")"#),
        [MOVED_TO_7]
    );
}
