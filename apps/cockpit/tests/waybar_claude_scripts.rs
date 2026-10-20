//! The bar's Claude pieces: the sessions chip, the usage reader and the bar visibility keeper.

mod common;
#[path = "common/codex_quota.rs"]
mod codex_quota;
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

fn codex_sandbox() -> Sandbox {
    codex_quota::sandbox()
}

fn fetch_codex(sandbox: &Sandbox, force: bool) -> std::process::Output {
    let script = waybar_scripts().join("codex-usage.sh");
    let mut arguments = vec![script.to_str().unwrap()];
    if force { arguments.push("--force"); }
    sandbox.run("bash", &arguments, &[])
}

#[test]
fn codex_quota_is_cached_without_persisting_or_printing_credentials() {
    let sandbox = codex_sandbox();
    let output = fetch_codex(&sandbox, false);
    assert!(output.status.success(), "{}", stderr(&output));
    let cached = fs::read_to_string(sandbox.runtime.join("codex-usage.json")).unwrap();
    let quota: Value = serde_json::from_str(&cached).unwrap();
    assert_eq!(quota["rate_limit"]["primary_window"]["used_percent"], 28);
    assert_eq!(quota["rate_limit"]["secondary_window"]["used_percent"], 86);
    for exposed in [cached, stdout(&output), stderr(&output), fs::read_to_string(&sandbox.log).unwrap()] {
        assert!(!exposed.contains("private@example.test"));
    }
}

#[test]
fn codex_quota_reuses_recent_cache_unless_forced() {
    let sandbox = codex_sandbox();
    assert!(fetch_codex(&sandbox, false).status.success());
    assert!(fetch_codex(&sandbox, false).status.success());
    assert_eq!(fs::read_to_string(&sandbox.log).unwrap().matches("account/rateLimits/read").count(), 1);
    assert!(fetch_codex(&sandbox, true).status.success());
    assert_eq!(fs::read_to_string(&sandbox.log).unwrap().matches("account/rateLimits/read").count(), 2);
}

#[test]
fn codex_quota_reports_missing_auth_without_using_another_account() {
    for account in [Value::Null, json!({"type":"apiKey"})] {
        let sandbox = codex_sandbox();
        let other_accounts = [sandbox.home.join(".local/share/opencode/auth.json"), sandbox.home.join("normal-codex/auth.json"), sandbox.home.join(".local/share/opencode/opencode.db")];
        for path in &other_accounts {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "unrelated-secret-credentials").unwrap();
        }
        fs::write(sandbox.home.join("account-response.json"), json!({"account":account,"requiresOpenaiAuth":true}).to_string()).unwrap();
        let output = fetch_codex(&sandbox, false);
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(sandbox.calls("curl").is_empty());
        let error: Value = serde_json::from_str(&fs::read_to_string(sandbox.runtime.join("codex-usage-error.json")).unwrap()).unwrap();
        assert!(!error["error"].as_str().unwrap().is_empty());
        assert!(!sandbox.runtime.join("codex-usage.json").exists());
        for path in other_accounts {
            assert_eq!(fs::read_to_string(path).unwrap(), "unrelated-secret-credentials");
        }
        assert!(!fs::read_to_string(&sandbox.log).unwrap().contains("account/login/start"));
    }
}

#[test]
fn failed_codex_refresh_keeps_the_last_successful_quota_and_timestamp() {
    for body in ["raise SystemExit(22)", "print('not-json', flush=True)", "print('{}', flush=True)"] {
        let sandbox = codex_sandbox();
        assert!(fetch_codex(&sandbox, false).status.success());
        let cache = sandbox.runtime.join("codex-usage.json");
        let original = fs::read(&cache).unwrap();
        let modified = fs::metadata(&cache).unwrap().modified().unwrap();
        codex_quota::vendor(&sandbox, body);
        assert!(fetch_codex(&sandbox, true).status.success());
        assert_eq!(fs::read(&cache).unwrap(), original);
        assert_eq!(fs::metadata(&cache).unwrap().modified().unwrap(), modified);
        assert!(sandbox.runtime.join("codex-usage-error.json").exists());
    }
}

#[test]
fn successful_codex_refresh_clears_the_previous_error() {
    let sandbox = codex_sandbox();
    fs::write(sandbox.runtime.join("codex-usage-error.json"), r#"{"error":"request failed"}"#).unwrap();
    assert!(fetch_codex(&sandbox, true).status.success());
    assert!(!sandbox.runtime.join("codex-usage-error.json").exists());
}

#[test]
fn codex_login_refreshes_a_recent_cache_without_using_the_old_account() {
    let sandbox = codex_sandbox();
    fs::write(sandbox.runtime.join("codex-usage.json"), r#"{"rate_limit":{"primary_window":{"used_percent":99}}}"#).unwrap();
    let script = waybar_scripts().join("codex-usage.sh");
    let output = sandbox.run("bash", &[script.to_str().unwrap(), "--login"], &[]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("https://auth.openai.com/test-login"));
    let cached: Value = serde_json::from_str(&fs::read_to_string(sandbox.runtime.join("codex-usage.json")).unwrap()).unwrap();
    assert_eq!(cached["rate_limit"]["primary_window"]["used_percent"], 28);
}

#[test]
fn claude_forced_refresh_updates_recent_cache_and_clears_the_error() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.join(".claude")).unwrap();
    fs::write(sandbox.home.join(".claude/.credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"claude-test-token","expiresAt":4102444800000}}"#).unwrap();
    fs::write(sandbox.runtime.join("claude-usage.json"), r#"{"five_hour":{"utilization":28}}"#).unwrap();
    fs::write(sandbox.runtime.join("claude-usage-error.json"), r#"{"error":"unknown"}"#).unwrap();
    sandbox.shim("curl", r#"cat >/dev/null; printf '{"five_hour":{"utilization":86}}'"#);
    let output = sandbox.run(waybar_scripts().join("claude-usage.sh"), &["five_hour", "--force"], &[]);
    assert!(output.status.success());
    let cached: Value = serde_json::from_str(&fs::read_to_string(sandbox.runtime.join("claude-usage.json")).unwrap()).unwrap();
    assert_eq!(cached["five_hour"]["utilization"], 86);
    assert!(!sandbox.runtime.join("claude-usage-error.json").exists());
    assert!(!fs::read_to_string(&sandbox.log).unwrap().contains("claude-test-token"));
}

#[test]
fn claude_forced_refresh_preserves_its_cache_when_the_request_fails() {
    let sandbox = Sandbox::new();
    fs::create_dir_all(sandbox.home.join(".claude")).unwrap();
    fs::write(sandbox.home.join(".claude/.credentials.json"),
        r#"{"claudeAiOauth":{"accessToken":"claude-test-token","expiresAt":4102444800000}}"#).unwrap();
    let cache = sandbox.runtime.join("claude-usage.json");
    let original = r#"{"five_hour":{"utilization":28}}"#;
    fs::write(&cache, original).unwrap();
    sandbox.shim("curl", "exit 22");
    let output = sandbox.run(waybar_scripts().join("claude-usage.sh"), &["five_hour", "--force"], &[]);
    assert!(output.status.success());
    assert_eq!(sandbox.calls("curl").len(), 1, "a forced refresh must bypass the recent cache");
    assert_eq!(fs::read_to_string(cache).unwrap(), original);
}

#[test]
fn invalid_claude_refresh_preserves_the_last_successful_quota_and_timestamp() {
    for response in ["not-json", r#"{"error":{"message":"try later"}}"#, "{}"] {
        let sandbox = Sandbox::new();
        fs::create_dir_all(sandbox.home.join(".claude")).unwrap();
        fs::write(sandbox.home.join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"claude-test-token","expiresAt":4102444800000}}"#).unwrap();
        let cache = sandbox.runtime.join("claude-usage.json");
        let original = r#"{"five_hour":{"utilization":28}}"#;
        fs::write(&cache, original).unwrap();
        let modified = fs::metadata(&cache).unwrap().modified().unwrap();
        fs::write(sandbox.home.join("response.json"), response).unwrap();
        sandbox.shim("curl", r#"cat >/dev/null; cat "$HOME/response.json""#);
        let output = sandbox.run(waybar_scripts().join("claude-usage.sh"), &["five_hour", "--force"], &[]);
        assert!(output.status.success());
        assert_eq!(fs::read_to_string(&cache).unwrap(), original);
        assert_eq!(fs::metadata(&cache).unwrap().modified().unwrap(), modified);
        assert!(sandbox.runtime.join("claude-usage-error.json").exists());
    }
}

struct BarVisibility {
    sandbox: Sandbox,
    workspace: std::path::PathBuf,
}

impl BarVisibility {
    fn new() -> BarVisibility {
        let sandbox = Sandbox::new();
        // Hyprland lists the bar on the top layer while shown and on the bottom one while hidden;
        // waybar's SIGUSR1 moves it from one to the other.
        sandbox.shim("hyprctl", r#"[[ $1 == -j && $2 == activeworkspace ]] && echo "{\"id\": $(cat "$SHIM_WS_FILE")}"
[[ $1 == layers && -s $SHIM_LAYER_FILE ]] && printf '\tLayer level %s (x):\n\t\tLayer 1: xywh: 0 0 10 10, a: 1, namespace: waybar, pid: 1\n' "$(cat "$SHIM_LAYER_FILE")"
exit 0"#);
        sandbox.shim("pkill", r#"[[ -s $SHIM_LAYER_FILE ]] && { [[ $(cat "$SHIM_LAYER_FILE") == 2 ]] && echo 1 > "$SHIM_LAYER_FILE" || echo 2 > "$SHIM_LAYER_FILE"; }; exit 0"#);
        let workspace = sandbox.root.join("ws");
        fs::write(&workspace, "2\n").unwrap();
        fs::write(sandbox.root.join("layer"), "2\n").unwrap();
        BarVisibility { sandbox, workspace }
    }

    fn layer(&self) -> String {
        fs::read_to_string(self.sandbox.root.join("layer")).unwrap().trim().to_string()
    }

    fn set_layer(&self, level: &str) {
        fs::write(self.sandbox.root.join("layer"), format!("{level}\n")).unwrap();
    }

    fn on_workspace(&self, workspace: u32) {
        fs::write(&self.workspace, format!("{workspace}\n")).unwrap();
    }

    fn visibility(&self, command: &str) {
        let layer = self.sandbox.root.join("layer");
        let output = self.sandbox.run(waybar_scripts().join("waybar-visibility.sh"), &[command],
                                      &[("SHIM_WS_FILE", self.workspace.to_str().unwrap()), ("SHIM_LAYER_FILE", layer.to_str().unwrap())]);
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

#[test]
fn a_lost_toggle_does_not_leave_the_bar_inverted() {
    // At boot a toggle can reach waybar before it listens: the stored state then says hidden while
    // the bar shows, and every later switch flipped it the wrong way.
    let bar = BarVisibility::new();
    bar.visibility("reset");
    bar.visibility("hide");
    bar.set_layer("2");
    bar.on_workspace(3);
    bar.visibility("sync");
    assert_eq!(bar.layer(), "2", "hidden off the Claude workspace");
    bar.on_workspace(6);
    bar.visibility("sync");
    assert_eq!(bar.layer(), "1", "shown on the Claude workspace");
}

#[test]
fn without_hyprland_the_stored_state_decides() {
    let bar = BarVisibility::new();
    fs::write(bar.sandbox.root.join("layer"), "").unwrap();
    bar.visibility("reset");
    bar.on_workspace(6);
    bar.visibility("sync");
    bar.visibility("sync");
    assert_eq!(bar.toggles(), 1);
}
