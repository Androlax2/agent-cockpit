//! Run the host floating-chat module without a compositor or starting an agent.
mod common;
use common::sandbox::*;
use std::process::Command;

fn review(before: &str, actions: &str) -> Vec<String> {
    let module = repo().join("config/hyprland/.config/hypr/figcraft-review.lua");
    let script = format!(r#"
local monitor = {{id = 0, x = 1920, y = -100, width = 3840, height = 2160,
    scale = 2, transform = 0, reserved = {{left = 0, top = 24, right = 0, bottom = 0}}}}
local cockpit = {{class = "claude-sessions", stable_id = "a", workspace = {{id = 6, name = "6"}}}}
local figcraft = {{class = "ai.storyteller.figcraft", address = "0xb", stable_id = "b", workspace = {{id = 2, name = "2"}}, monitor = monitor}}
local chat = {{class = "figcraft-chat.{SESSION_A}", stable_id = "chat", workspace = {{id = 2, name = "2"}}}}
local windows = {{cockpit, figcraft}}
local focused = cockpit
local events = {{}}
local fail_when
local function action(kind)
    return function(spec) return {{kind = kind, spec = spec}} end
end
hl = {{
    get_windows = function() return windows end,
    get_active_window = function() return focused end,
    notification = {{create = function(spec) print("notification " .. spec.text) end}},
    on = function(name, callback) events[name] = callback end,
    exec_cmd = function(command, rules)
        if fail_when and fail_when({{kind = "exec"}}) then error("injected launch failure") end
        print("exec " .. command)
        for _, key in ipairs({{"workspace", "float", "size", "move"}}) do
            if rules[key] ~= nil then print("rule " .. key .. "=" .. tostring(rules[key])) end
        end
    end,
    dsp = {{focus = action("focus"), window = {{float = action("float"), move = action("move"),
        resize = action("resize"), close = action("close"), fullscreen_state = action("fullscreen")}}}},
    dispatch = function(command)
        local spec, kind = command.spec, command.kind
        local window = spec.window
        assert(window, "every operation must target an explicit window")
        if fail_when and fail_when(command) then return {{ok = false, error = "injected compositor failure"}} end
        local fields = {{kind, window.stable_id}}
        for _, key in ipairs({{"action", "workspace", "x", "y", "relative", "follow"}}) do
            if spec[key] ~= nil then table.insert(fields, key .. "=" .. tostring(spec[key])) end
        end
        print(table.concat(fields, " "))
        if kind == "close" then
            for index, candidate in ipairs(windows) do
                if candidate == window then table.remove(windows, index); break end
            end
        elseif kind == "move" and spec.workspace then
            window.workspace = {{id = tonumber(spec.workspace) or -99, name = tostring(spec.workspace):gsub("^name:", "")}}
        end
        return {{ok = true}}
    end,
}}
local function open_chat()
    table.insert(windows, chat)
    if events["window.open"] then events["window.open"](chat) end
end
{before}
local review = dofile({module:?})
local function start() review.start("{SESSION_A}", "claude-retry-9-9") end
{actions}
"#, module = module.display().to_string());
    let output = Command::new("lua5.4").args(["-e", &script]).output().expect("lua5.4 runs the floating-chat module");
    assert!(output.status.success(), "{}", stderr(&output));
    stdout(&output).lines().map(str::to_string).collect()
}

#[test]
fn review_opens_only_the_interactive_agent_client_on_figcrafts_workspace() {
    let calls = review("", "start(); open_chat()");
    assert!(calls.contains(&format!("exec kitty --class figcraft-chat.{SESSION_A} -e env -u TMUX tmux attach-session -t =claude-retry-9-9")), "{calls:?}");
    for expected in ["rule workspace=2 silent", "rule float=true", "rule size=560 897",
        "move chat workspace=2 follow=false", "resize chat x=560 y=897 relative=false", "move chat x=3256 y=8 relative=false", "focus chat"] {
        assert!(calls.contains(&expected.to_string()), "missing {expected}: {calls:?}");
    }
    assert!(!calls.iter().any(|call| call.contains(" a ") || call.contains(" b ") || call.ends_with(" a") || call.ends_with(" b")), "{calls:?}");
}

#[test]
fn chat_launch_does_not_require_the_cockpit_to_be_open() {
    let calls = review("windows = {figcraft}", "start()");
    assert!(calls.iter().any(|call| call.starts_with("exec kitty ")));
}

#[test]
fn missing_or_ambiguous_figcraft_is_refused_before_launch() {
    for before in ["windows = {cockpit}",
        "windows[3] = {class = 'ai.storyteller.figcraft', stable_id = 'other'}",
        "figcraft.monitor = nil"] {
        let calls = review(before, "assert(not pcall(start))");
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(calls[0].starts_with("notification "));
    }
}

#[test]
fn focused_figcraft_disambiguates_multiple_projects_without_moving_them() {
    let calls = review("windows[3] = {class = 'ai.storyteller.figcraft', stable_id = 'other'}; focused = figcraft", "start(); open_chat()");
    assert!(calls.contains(&"move chat workspace=2 follow=false".into()));
    assert!(!calls.iter().any(|call| call.contains("other")));
}

#[test]
fn an_explicit_window_works_after_the_approval_terminal_has_taken_focus() {
    let calls = review("windows[3] = {class = 'ai.storyteller.figcraft', address = '0xc', stable_id = 'other'}",
        &format!("review.start('{SESSION_A}', 'claude-retry-9-9', '0xb'); open_chat()"));
    assert!(calls.contains(&"move chat workspace=2 follow=false".into()));
    assert!(!calls.iter().any(|call| call.contains("other")));
    let calls = review("", &format!("assert(not pcall(function() review.start('{SESSION_A}', 'claude-retry-9-9', '0xc') end))"));
    assert_eq!(calls.len(), 1);
    assert!(calls[0].starts_with("notification "));
}

#[test]
fn repeated_start_reuses_the_chat_and_keeps_its_user_adjusted_geometry() {
    let calls = review("", "start(); start(); open_chat(); print('repeat'); start()");
    assert_eq!(calls.iter().filter(|call| call.starts_with("exec ")).count(), 1);
    let repeated = &calls[calls.iter().position(|call| call == "repeat").unwrap() + 1..];
    assert_eq!(repeated, ["focus chat"]);
}

#[test]
fn stop_closes_only_the_float_and_is_idempotent_without_figcraft() {
    let calls = review("", "start(); open_chat(); windows = {cockpit, chat}; print('stop'); review.stop(); review.stop()");
    assert_eq!(&calls[calls.iter().position(|call| call == "stop").unwrap() + 1..], ["close chat"]);
}

#[test]
fn stop_refuses_ambiguity_but_accepts_an_exact_conversation_id() {
    let calls = review("windows[3] = chat; windows[4] = {class = 'figcraft-chat.other', stable_id = 'other'}",
        &format!("assert(not pcall(review.stop)); review.stop('{SESSION_A}')"));
    assert!(calls[0].starts_with("notification "));
    assert_eq!(calls.last().unwrap(), "close chat");
    assert!(!calls.iter().any(|call| call == "close other"));
}

#[test]
fn stop_cancels_a_launch_that_has_not_mapped_yet() {
    let calls = review("", "start(); review.stop(); open_chat()");
    assert_eq!(calls.last().unwrap(), "close chat");
    assert!(!calls.iter().any(|call| call.starts_with("move ") || call.starts_with("resize ")));
}

#[test]
fn retry_after_a_slow_start_does_not_leave_duplicate_chat_windows() {
    let calls = review("local clock = 100; os.time = function() return clock end; advance_clock = function() clock = clock + 11 end", r#"
start(); advance_clock(); start(); open_chat()
local duplicate = {class = chat.class, stable_id = 'duplicate', workspace = chat.workspace}
table.insert(windows, duplicate); events['window.open'](duplicate)
"#);
    assert_eq!(calls.iter().filter(|call| call.starts_with("exec ")).count(), 2);
    assert_eq!(calls.last().unwrap(), "close duplicate");
}

#[test]
fn stopped_slow_launches_remain_cancelled_until_the_next_explicit_start() {
    let calls = review("local clock = 100; os.time = function() return clock end; advance_clock = function() clock = clock + 11 end", r#"
start(); advance_clock(); start(); review.stop(); open_chat()
local late = {class = chat.class, stable_id = 'late', workspace = chat.workspace}
table.insert(windows, late); events['window.open'](late)
"#);
    assert!(calls.contains(&"close chat".into()));
    assert_eq!(calls.last().unwrap(), "close late");
    assert!(!calls.iter().any(|call| call.starts_with("move ")));
}

#[test]
fn stopping_a_mapped_chat_also_cancels_any_outstanding_slow_client() {
    let calls = review("local clock = 100; os.time = function() return clock end; advance_clock = function() clock = clock + 11 end", r#"
start(); advance_clock(); start(); open_chat(); review.stop()
local late = {class = chat.class, stable_id = 'late', workspace = chat.workspace}
table.insert(windows, late); events['window.open'](late)
"#);
    assert!(calls.contains(&"close chat".into()));
    assert_eq!(calls.last().unwrap(), "close late");
}

#[test]
fn launch_failure_is_reported_and_allows_retry() {
    let calls = review("fail_when = function(command) return command.kind == 'exec' end",
        "assert(not pcall(start)); fail_when = nil; start(); open_chat()");
    assert_eq!(calls.iter().filter(|call| call.starts_with("exec ")).count(), 1);
    assert!(calls.contains(&"focus chat".into()));
}

#[test]
fn closed_or_replaced_figcraft_during_launch_does_not_place_the_chat_elsewhere() {
    let calls = review("", "start(); windows = {cockpit, {class = 'ai.storyteller.figcraft', stable_id = 'replacement'}}; open_chat()");
    assert!(calls.contains(&"close chat".into()));
    assert!(!calls.iter().any(|call| call.starts_with("move ")));
}

#[test]
fn named_and_special_workspaces_are_selected_without_using_reserved_keywords() {
    for (name, selector) in [("special:design", "special:design"), ("previous", "name:previous"), ("Bob's design", "name:Bob's design")] {
        let calls = review(&format!("figcraft.workspace = {{id = -99, name = {name:?}}}"), "start(); open_chat()");
        assert!(calls.contains(&format!("rule workspace={selector} silent")), "{calls:?}");
        assert!(calls.contains(&format!("move chat workspace={selector} follow=false")), "{calls:?}");
    }
}

#[test]
fn rotated_scaled_monitors_and_small_displays_keep_the_chat_in_the_usable_area() {
    let calls = review("monitor.transform = 1; monitor.reserved.left = 10; monitor.reserved.right = 10", "start(); open_chat()");
    assert!(calls.contains(&"resize chat x=560 y=1611 relative=false".into()), "{calls:?}");
    assert!(calls.contains(&"move chat x=2406 y=75 relative=false".into()), "{calls:?}");
    let calls = review("monitor.width = 640; monitor.height = 480", "start(); open_chat()");
    assert!(calls.contains(&"resize chat x=320 y=183 relative=false".into()), "{calls:?}");
    assert!(calls.contains(&"move chat x=1920 y=-59 relative=false".into()), "{calls:?}");
}

#[test]
fn invalid_ids_or_targets_are_refused_before_executing_shell_source() {
    for arguments in ["'safe', 'claude-manager'", "'bad;touch /tmp/no', 'claude-retry-9-9'", "'safe', 'claude-retry-9;touch /tmp/no'"] {
        let calls = review("", &format!("assert(not pcall(function() review.start({arguments}) end))"));
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(calls[0].starts_with("notification "));
    }
}

#[test]
fn failed_close_keeps_the_float_available_for_an_explicit_retry() {
    let calls = review("windows[3] = chat; fail_when = function(command) return command.kind == 'close' end",
        "assert(not pcall(review.stop)); fail_when = nil; review.stop()");
    assert!(calls[0].starts_with("notification "));
    assert_eq!(calls.last().unwrap(), "close chat");
}
