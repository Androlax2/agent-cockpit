//! Exercise the host layout module without connecting to a compositor or starting applications.
mod common;
use common::sandbox::*;
use std::process::Command;

fn review(before: &str, actions: &str) -> Vec<String> {
    let module = repo().join("config/hyprland/.config/hypr/figcraft-review.lua");
    let script = format!(r#"
local monitor = {{id = 0, x = 1920, y = -100, width = 3840, height = 2160,
    scale = 2, transform = 0, reserved = {{left = 0, top = 24, right = 0, bottom = 0}}}}
local cockpit = {{class = "claude-sessions", address = "0xa", stable_id = "a", pid = 10,
    workspace = {{id = 6, name = "6"}}, monitor = monitor, floating = false,
    fullscreen = 1, fullscreen_client = 2, at = {{x = 1920, y = -76}}, size = {{x = 1920, y = 1056}}}}
local figcraft = {{class = "ai.storyteller.figcraft", address = "0xb", stable_id = "b", pid = 20,
    workspace = {{id = 2, name = "2"}}, monitor = monitor, floating = true,
    fullscreen = 0, fullscreen_client = 0, at = {{x = 2100, y = 100}}, size = {{x = 900, y = 600}}}}
local windows = {{cockpit, figcraft}}
local focused = cockpit
local fail_when
local function action(kind)
    return function(spec) return {{kind = kind, spec = spec}} end
end
hl = {{
    get_windows = function() return windows end,
    get_active_window = function() return focused end,
    get_monitor = function() return monitor end,
    notification = {{create = function(spec) print("notification " .. spec.text) end}},
    dsp = {{focus = action("focus"), window = {{
        float = action("float"), fullscreen_state = action("fullscreen"),
        move = action("move"), resize = action("resize"),
    }}}},
    dispatch = function(command)
        local spec = command.spec
        local window = spec.window
        assert(window, "every operation must target an explicit window")
        if fail_when and fail_when(command) then return {{ok = false, error = 'injected compositor failure'}} end
        local fields = {{command.kind, window.stable_id}}
        for _, key in ipairs({{"action", "workspace", "x", "y", "relative", "internal", "client", "follow"}}) do
            if spec[key] ~= nil then table.insert(fields, key .. "=" .. tostring(spec[key])) end
        end
        print(table.concat(fields, " "))
        if command.kind == "float" then window.floating = spec.action == "set"
        elseif command.kind == "fullscreen" then window.fullscreen = spec.internal; window.fullscreen_client = spec.client
        elseif command.kind == "resize" then window.size = {{x = spec.x, y = spec.y}}
        elseif command.kind == "move" then
            if spec.workspace then window.workspace = {{id = spec.workspace, name = tostring(spec.workspace)}}
            else window.at = {{x = spec.x, y = spec.y}} end
        end
        return {{ok = true}}
    end,
}}
{before}
local review = dofile({module:?})
{actions}
"#, module = module.display().to_string());
    let output = Command::new("lua5.4").args(["-e", &script]).output().expect("lua5.4 runs the layout module");
    assert!(output.status.success(), "{}", stderr(&output));
    stdout(&output).lines().map(str::to_string).collect()
}

#[test]
fn review_places_existing_windows_in_logical_left_and_right_halves() {
    let calls = review("", "review.start()");
    for expected in ["move a workspace=6 follow=false", "move b workspace=6 follow=false",
        "move a x=1920 y=-76 relative=false", "move b x=2880 y=-76 relative=false",
        "resize a x=960 y=1056 relative=false", "resize b x=960 y=1056 relative=false"] {
        assert!(calls.contains(&expected.to_string()), "missing {expected}: {calls:?}");
    }
    assert_eq!(calls.last().unwrap(), "focus a");
}

#[test]
fn stop_restores_workspaces_floating_geometry_and_fullscreen_states() {
    let calls = review("", "review.start(); print('restore'); review.stop()");
    let restored = &calls[calls.iter().position(|call| call == "restore").unwrap() + 1..];
    for expected in ["move b workspace=2 follow=false", "move b x=2100 y=100 relative=false",
        "resize b x=900 y=600 relative=false", "float a action=unset", "float b action=set",
        "fullscreen a action=set internal=1 client=2", "fullscreen b action=set internal=0 client=0"] {
        assert!(restored.contains(&expected.to_string()), "missing {expected}: {restored:?}");
    }
}

#[test]
fn repeated_start_does_not_replace_the_original_arrangement() {
    let calls = review("", "review.start(); print('repeat'); review.start(); print('restore'); review.stop(); print('stopped'); review.stop()");
    assert_eq!(calls.iter().position(|call| call == "restore").unwrap(), calls.iter().position(|call| call == "repeat").unwrap() + 1);
    assert!(calls.contains(&"move b workspace=2 follow=false".into()));
    assert_eq!(calls.last().unwrap(), "stopped");
}

#[test]
fn missing_or_ambiguous_windows_are_refused_before_any_movement() {
    for before in ["windows = {cockpit}", "windows = {figcraft}",
        "windows = {cockpit, figcraft, {class = 'ai.storyteller.figcraft', stable_id = 'c'}}",
        "windows = {cockpit, figcraft, {class = 'claude-sessions', stable_id = 'c'}}"] {
        let calls = review(before, "assert(not pcall(review.start))");
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(calls[0].starts_with("notification "), "{calls:?}");
    }
}

#[test]
fn focused_figcraft_disambiguates_multiple_open_design_windows() {
    let calls = review("windows[3] = {class = 'ai.storyteller.figcraft', stable_id = 'c'}; focused = figcraft", "review.start()");
    assert!(calls.contains(&"move b workspace=6 follow=false".into()));
    assert!(!calls.iter().any(|call| call.starts_with("move c")));
}

#[test]
fn portrait_monitor_uses_transformed_scaled_dimensions_and_reserved_edges() {
    let calls = review("monitor.transform = 1; monitor.reserved.left = 10; monitor.reserved.right = 10", "review.start()");
    for expected in ["move a x=1930 y=-76 relative=false", "move b x=2460 y=-76 relative=false",
        "resize b x=530 y=1896 relative=false"] {
        assert!(calls.contains(&expected.to_string()), "missing {expected}: {calls:?}");
    }
}

#[test]
fn stop_restores_survivors_without_touching_a_replacement_window() {
    let calls = review("", "review.start(); windows = {cockpit, {address = '0xb', stable_id = 'new', pid = 20}}; print('restore'); review.stop()");
    let restored = &calls[calls.iter().position(|call| call == "restore").unwrap() + 1..];
    assert!(restored.contains(&"float a action=unset".into()));
    assert!(!restored.iter().any(|call| call.contains(" new ") || call.contains(" b ")));
}

#[test]
fn grouped_or_pinned_windows_are_refused_without_breaking_their_layout() {
    for before in ["figcraft.group = {}", "cockpit.pinned = true"] {
        let calls = review(before, "assert(not pcall(review.start))");
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(calls[0].starts_with("notification "));
    }
}

#[test]
fn stop_returns_a_design_to_its_named_special_workspace() {
    let calls = review("figcraft.workspace = {id = -99, name = 'special:design'}", "review.start(); review.stop()");
    assert!(calls.contains(&"move b workspace=special:design follow=false".into()), "{calls:?}");
}

#[test]
fn stop_uses_an_explicit_named_selector_instead_of_a_reserved_workspace_keyword() {
    let calls = review("figcraft.workspace = {id = -1337, name = 'previous'}", "review.start(); review.stop()");
    assert!(calls.contains(&"move b workspace=name:previous follow=false".into()), "{calls:?}");
}

#[test]
fn failed_restore_reports_the_error_and_preserves_the_original_arrangement_for_retry() {
    let calls = review("", r#"review.start()
fail_when = function(command) return command.kind == 'move' and command.spec.workspace == 2 end
assert(not pcall(review.stop), 'failed restore must not report success')
fail_when = nil
print('retry')
review.stop()"#);
    let retry = &calls[calls.iter().position(|call| call == "retry").unwrap() + 1..];
    assert!(calls.iter().any(|call| call.starts_with("notification ") && call.contains("injected compositor failure")));
    assert!(retry.contains(&"move b workspace=2 follow=false".into()), "{retry:?}");
    assert!(retry.contains(&"move b x=2100 y=100 relative=false".into()));
}

#[test]
fn failed_start_stops_dispatching_and_requires_recovery_before_another_start() {
    let calls = review("fail_when = function(command) return command.kind == 'move' and command.spec.window == figcraft end", r#"
assert(not pcall(review.start), 'failed start must not report success')
assert(not pcall(review.start), 'partial start must not silently report an active layout')
fail_when = nil
print('recover')
review.stop()"#);
    let failure = &calls[..calls.iter().position(|call| call == "recover").unwrap()];
    assert!(!failure.iter().any(|call| call.starts_with("resize b") || call == "focus a"));
    assert!(calls.contains(&"move b workspace=2 follow=false".into()));
}
