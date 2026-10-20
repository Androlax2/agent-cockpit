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

fn codex_sandbox() -> Sandbox {
    let mut sandbox = Sandbox::new();
    sandbox.env.insert("XDG_DATA_HOME".into(), sandbox.home.join(".local/share").display().to_string());
    sandbox.env.insert("CODEX_HOME".into(), sandbox.home.join(".codex").display().to_string());
    sandbox.env.insert("OPENCODE_DB".into(), sandbox.home.join(".local/share/opencode/opencode.db").display().to_string());
    let credentials = sandbox.home.join(".local/share/opencode/auth.json");
    fs::create_dir_all(credentials.parent().unwrap()).unwrap();
    fs::write(credentials, json!({"openai": {"type": "oauth", "access": "secret-test-token", "expires": 4102444800000i64,
                                            "accountId": "test-account"}}).to_string()).unwrap();
    sandbox.shim("curl", r#"
headers=$(cat)
[[ $headers == *'Authorization: Bearer secret-test-token'* && $headers == *'ChatGPT-Account-Id: test-account'* ]] || exit 2
[[ "$*" == *'https://chatgpt.com/backend-api/wham/usage'* && "$*" != *secret-test-token* ]] || exit 3
cat "$HOME/response.json"
"#);
    fs::write(sandbox.home.join("response.json"), json!({"rate_limit": {"allowed": true, "limit_reached": false,
        "primary_window": {"used_percent": 28, "limit_window_seconds": 18000, "reset_at": 4102444800i64},
        "secondary_window": {"used_percent": 86, "limit_window_seconds": 604800, "reset_at": 4102444800i64}}}).to_string()).unwrap();
    sandbox
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
    let credentials = sandbox.home.join(".local/share/opencode/auth.json");
    let original = fs::read(&credentials).unwrap();
    let output = fetch_codex(&sandbox, false);
    assert!(output.status.success(), "{}", stderr(&output));
    let cached = fs::read_to_string(sandbox.runtime.join("codex-usage.json")).unwrap();
    let quota: Value = serde_json::from_str(&cached).unwrap();
    assert_eq!(quota["rate_limit"]["primary_window"]["used_percent"], 28);
    assert_eq!(quota["rate_limit"]["secondary_window"]["used_percent"], 86);
    assert_eq!(fs::read(credentials).unwrap(), original);
    for exposed in [cached, stdout(&output), stderr(&output), fs::read_to_string(&sandbox.log).unwrap()] {
        assert!(!exposed.contains("secret-test-token"));
    }
}

#[test]
fn codex_quota_reuses_recent_cache_unless_forced() {
    let sandbox = codex_sandbox();
    assert!(fetch_codex(&sandbox, false).status.success());
    assert!(fetch_codex(&sandbox, false).status.success());
    assert_eq!(sandbox.calls("curl").len(), 1);
    assert!(fetch_codex(&sandbox, true).status.success());
    assert_eq!(sandbox.calls("curl").len(), 2);
}

#[test]
fn codex_quota_reports_missing_or_expired_auth_without_using_another_account() {
    for credentials in [json!({}), json!({"openai": {"type": "oauth", "access": "expired-token", "expires": 1}})] {
        let sandbox = codex_sandbox();
        fs::write(sandbox.home.join(".local/share/opencode/auth.json"), credentials.to_string()).unwrap();
        // An expired OpenCode account must not fall back to another Codex CLI account.
        if credentials.get("openai").is_some() {
            fs::create_dir_all(sandbox.home.join(".codex")).unwrap();
            fs::write(sandbox.home.join(".codex/auth.json"), r#"{"tokens":{"access_token":"other-token","account_id":"other-account"}}"#).unwrap();
        }
        let output = fetch_codex(&sandbox, false);
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(sandbox.calls("curl").is_empty());
        let error: Value = serde_json::from_str(&fs::read_to_string(sandbox.runtime.join("codex-usage-error.json")).unwrap()).unwrap();
        assert!(!error["error"].as_str().unwrap().is_empty());
        assert!(!sandbox.runtime.join("codex-usage.json").exists());
    }
}

#[test]
fn failed_codex_refresh_keeps_the_last_successful_quota_and_timestamp() {
    for body in ["exit 22", "printf not-json", "printf '{}'", "printf '{\"rate_limit\": null}'"] {
        let sandbox = codex_sandbox();
        assert!(fetch_codex(&sandbox, false).status.success());
        let cache = sandbox.runtime.join("codex-usage.json");
        let original = fs::read(&cache).unwrap();
        let modified = fs::metadata(&cache).unwrap().modified().unwrap();
        sandbox.shim("curl", body);
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
fn codex_quota_uses_cli_credentials_when_opencode_has_no_oauth_account() {
    let sandbox = codex_sandbox();
    fs::write(sandbox.home.join(".local/share/opencode/auth.json"), r#"{"openai":{"type":"api","key":"api-key"}}"#).unwrap();
    fs::create_dir_all(sandbox.home.join(".codex")).unwrap();
    let credentials = sandbox.home.join(".codex/auth.json");
    let original = r#"{"tokens":{"access_token":"secret-test-token","account_id":"test-account"}}"#;
    fs::write(&credentials, original).unwrap();
    let output = fetch_codex(&sandbox, false);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(sandbox.runtime.join("codex-usage.json").exists());
    assert_eq!(fs::read_to_string(credentials).unwrap(), original);
}

fn opencode_credentials(sandbox: &Sandbox, rows: &[(&str, Option<bool>, i64, Value)]) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(&sandbox.env["OPENCODE_DB"]);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let database = rusqlite::Connection::open(&path).unwrap();
    database.execute_batch("CREATE TABLE credential (id TEXT PRIMARY KEY, integration_id TEXT, label TEXT NOT NULL,
        value TEXT NOT NULL, connector_id TEXT, method_id TEXT, active INTEGER, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL)").unwrap();
    for (id, active, created, value) in rows {
        database.execute("INSERT INTO credential (id,integration_id,label,value,active,time_created,time_updated) VALUES (?1,'openai','test',?2,?3,?4,?4)",
            rusqlite::params![id, value.to_string(), active, created]).unwrap();
    }
    path
}

fn v2_oauth(token: &str, expires: i64) -> Value {
    json!({"type":"oauth", "access":token, "refresh":"secret-test-refresh", "expires":expires,
        "methodID":"test-method", "metadata":{"accountID":"test-account"}})
}

#[test]
fn codex_quota_reads_the_active_v2_account_instead_of_expired_legacy_credentials() {
    let sandbox = codex_sandbox();
    fs::write(sandbox.home.join(".local/share/opencode/auth.json"),
        r#"{"openai":{"type":"oauth","access":"stale-legacy-token","expires":1}}"#).unwrap();
    let path = opencode_credentials(&sandbox, &[("current", Some(true), 1, v2_oauth("secret-test-token", 4102444800000))]);
    let original = fs::read(&path).unwrap();
    let output = fetch_codex(&sandbox, false);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(sandbox.runtime.join("codex-usage.json").exists(), "{}", stdout(&output));
    assert_eq!(fs::read(path).unwrap(), original, "quota reading changed the credential database");
    for exposed in [stdout(&output), stderr(&output), fs::read_to_string(&sandbox.log).unwrap(),
                    fs::read_to_string(sandbox.runtime.join("codex-usage.json")).unwrap()] {
        for secret in ["secret-test-token", "secret-test-refresh", "stale-legacy-token"] {
            assert!(!exposed.contains(secret));
        }
    }
}

#[test]
fn codex_quota_prefers_an_explicitly_active_account_over_a_newer_inactive_account() {
    let sandbox = codex_sandbox();
    fs::remove_file(sandbox.home.join(".local/share/opencode/auth.json")).unwrap();
    opencode_credentials(&sandbox, &[("active", Some(true), 1, v2_oauth("secret-test-token", 4102444800000)),
        ("inactive", Some(false), 2, v2_oauth("wrong-account-token", 4102444800000))]);
    assert!(fetch_codex(&sandbox, false).status.success());
    assert!(sandbox.runtime.join("codex-usage.json").exists(), "the active account was not used");
    assert_eq!(sandbox.calls("curl").len(), 1);
}

#[test]
fn codex_quota_matches_v2_selection_for_accounts_without_an_explicit_active_flag() {
    let sandbox = codex_sandbox();
    fs::remove_file(sandbox.home.join(".local/share/opencode/auth.json")).unwrap();
    opencode_credentials(&sandbox, &[("old", None, 1, v2_oauth("wrong-account-token", 4102444800000)),
        ("a-new", None, 2, v2_oauth("wrong-account-token", 4102444800000)),
        ("z-new", None, 2, v2_oauth("secret-test-token", 4102444800000))]);
    assert!(fetch_codex(&sandbox, false).status.success());
    assert!(sandbox.runtime.join("codex-usage.json").exists(), "the newest account (then highest id) must win");
}

#[test]
fn codex_quota_does_not_fall_back_to_legacy_auth_for_an_expired_or_non_subscription_v2_account() {
    for value in [v2_oauth("expired-v2-token", 1), json!({"type":"api", "key":"secret-test-key"})] {
        let sandbox = codex_sandbox();
        opencode_credentials(&sandbox, &[("selected", Some(true), 1, value)]);
        assert!(fetch_codex(&sandbox, false).status.success());
        assert!(sandbox.calls("curl").is_empty(), "must not silently switch to the valid legacy account");
        assert!(sandbox.runtime.join("codex-usage-error.json").exists());
        assert!(!sandbox.runtime.join("codex-usage.json").exists());
    }
}

#[test]
fn codex_quota_does_not_revive_legacy_auth_after_the_v2_account_was_removed() {
    let sandbox = codex_sandbox();
    opencode_credentials(&sandbox, &[]);
    assert!(fetch_codex(&sandbox, false).status.success());
    assert!(sandbox.calls("curl").is_empty());
    assert!(!sandbox.runtime.join("codex-usage.json").exists());
}

#[test]
fn codex_quota_reports_an_unreadable_v2_store_without_using_legacy_credentials() {
    let sandbox = codex_sandbox();
    fs::write(&sandbox.env["OPENCODE_DB"], "not a database").unwrap();
    assert!(fetch_codex(&sandbox, false).status.success());
    assert!(sandbox.calls("curl").is_empty());
    assert!(sandbox.runtime.join("codex-usage-error.json").exists());
}

#[test]
fn codex_quota_honors_the_configured_v2_database_path() {
    let mut sandbox = codex_sandbox();
    sandbox.env.insert("OPENCODE_DB".into(), sandbox.root.join("custom.db").display().to_string());
    fs::write(sandbox.home.join(".local/share/opencode/auth.json"), "{}").unwrap();
    opencode_credentials(&sandbox, &[("current", Some(true), 1, v2_oauth("secret-test-token", 4102444800000))]);
    assert!(fetch_codex(&sandbox, false).status.success());
    assert!(sandbox.runtime.join("codex-usage.json").exists());
}

#[test]
fn codex_quota_supports_token_sharing_sign_ins_without_an_account_identifier() {
    let sandbox = codex_sandbox();
    opencode_credentials(&sandbox, &[("current", Some(true), 1, json!({"type":"oauth", "methodID":"chatgpt-token-sharing",
        "access":"secret-test-token", "refresh":"secret-test-refresh", "expires":4102444800000i64,
        "metadata":{"clientID":"test-client", "scopes":["test-scope"]}}))]);
    sandbox.shim("curl", r#"
headers=$(cat)
[[ $headers == *'Authorization: Bearer secret-test-token'* && $headers != *'ChatGPT-Account-Id:'* ]] || exit 2
cat "$HOME/response.json"
"#);
    let output = fetch_codex(&sandbox, false);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(sandbox.runtime.join("codex-usage.json").exists(), "quota polling must not require an optional account id");
    assert!(!sandbox.runtime.join("codex-usage-error.json").exists());
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
