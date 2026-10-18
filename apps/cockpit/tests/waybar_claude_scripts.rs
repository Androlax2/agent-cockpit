//! The bar's Claude pieces: the sessions chip, the usage reader and the bar visibility keeper.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;

fn chip(sandbox: &Sandbox, snapshot: Value) -> Value {
    let path = sandbox.root.join("snapshot.json");
    fs::write(&path, snapshot.to_string()).unwrap();
    let output = sandbox.run(waybar_scripts().join("claude-sessions.sh"), &[], &[("CLAUDE_SESSIONS_SNAPSHOT", path.to_str().unwrap())]);
    serde_json::from_str(&stdout(&output)).unwrap()
}

#[test]
fn chip_counts_by_state_with_the_most_urgent_class() {
    let sandbox = Sandbox::new();
    let chip = chip(&sandbox, json!({"sessions": [{"state": "needs-you", "project": "a", "since": 1}, {"state": "working", "project": "b", "since": 1}]}));
    let text = chip["text"].as_str().unwrap();
    assert!(text.contains("● 1"), "{text}");
    assert!(text.contains("! 1"), "{text}");
    assert_eq!(chip["class"], "needs-you");
}

#[test]
fn chip_shows_the_idle_glyph_with_no_session() {
    let sandbox = Sandbox::new();
    let chip = chip(&sandbox, json!({"sessions": []}));
    assert_eq!(chip["class"], "idle");
    assert_eq!(chip["text"], "\u{F06A9}");
}

#[test]
fn chip_tooltip_escapes_markup() {
    let sandbox = Sandbox::new();
    let chip = chip(&sandbox, json!({"sessions": [{"state": "working", "project": "a", "task": "fix <totals>", "since": 1}]}));
    let tooltip = chip["tooltip"].as_str().unwrap();
    assert!(tooltip.contains("&lt;totals&gt;"), "{tooltip}");
}

fn usage(sandbox: &Sandbox, window: &str, fixture: &Value) -> Value {
    let path = sandbox.root.join("usage.json");
    fs::write(&path, fixture.to_string()).unwrap();
    let output = sandbox.run(waybar_scripts().join("claude-usage.sh"), &[window], &[("CLAUDE_USAGE_FIXTURE", path.to_str().unwrap())]);
    serde_json::from_str(&stdout(&output)).unwrap()
}

#[test]
fn each_usage_window_carries_its_glyph_and_threshold_class() {
    let sandbox = Sandbox::new();
    let fixture = json!({
        "five_hour": {"utilization": 42.3, "resets_at": "2026-10-16T21:00:00Z"},
        "seven_day": {"utilization": 91, "resets_at": "2026-10-20T09:00:00Z"},
    });
    let five = usage(&sandbox, "five_hour", &fixture);
    let seven = usage(&sandbox, "seven_day", &fixture);
    assert!(five["text"].as_str().unwrap().contains("42%"), "{five}");
    assert_eq!(five["class"], "ok");
    assert!(seven["text"].as_str().unwrap().contains("91%"), "{seven}");
    assert_eq!(seven["class"], "critical");
}

#[test]
fn unknown_usage_values_render_a_dash() {
    let sandbox = Sandbox::new();
    assert_eq!(usage(&sandbox, "five_hour", &json!({"five_hour": null, "seven_day": null}))["class"], "unknown");
}

struct BarVisibility {
    sandbox: Sandbox,
    workspace: std::path::PathBuf,
}

impl BarVisibility {
    fn new() -> BarVisibility {
        let sandbox = Sandbox::new();
        sandbox.shim("hyprctl", r#"[[ $1 == -j && $2 == activeworkspace ]] && echo "{\"id\": $(cat "$SHIM_WS_FILE")}"; exit 0"#);
        sandbox.shim("pkill", "");
        let workspace = sandbox.root.join("ws");
        fs::write(&workspace, "2\n").unwrap();
        BarVisibility { sandbox, workspace }
    }

    fn on_workspace(&self, workspace: u32) {
        fs::write(&self.workspace, format!("{workspace}\n")).unwrap();
    }

    fn visibility(&self, command: &str) {
        let output = self.sandbox.run(waybar_scripts().join("waybar-visibility.sh"), &[command], &[("SHIM_WS_FILE", self.workspace.to_str().unwrap())]);
        assert!(output.status.success(), "waybar-visibility.sh {command}: {}", stderr(&output));
    }

    fn toggles(&self) -> usize {
        self.sandbox.calls("pkill").len()
    }
}

#[test]
fn bar_hides_once_on_the_claude_workspace_and_shows_once_back() {
    let bar = BarVisibility::new();
    bar.visibility("reset");
    bar.visibility("sync");
    assert_eq!(bar.toggles(), 0);
    bar.on_workspace(6);
    bar.visibility("sync");
    bar.visibility("sync");
    assert_eq!(bar.toggles(), 1);
    bar.on_workspace(3);
    bar.visibility("sync");
    assert_eq!(bar.toggles(), 2);
}

#[test]
fn a_pre_hide_is_not_toggled_twice_by_the_following_event() {
    let bar = BarVisibility::new();
    bar.visibility("reset");
    bar.visibility("hide");
    bar.on_workspace(6);
    bar.visibility("sync");
    assert_eq!(bar.toggles(), 1);
    assert_eq!(bar.sandbox.calls("pkill"), ["pkill -x -USR1 waybar"]);
}
