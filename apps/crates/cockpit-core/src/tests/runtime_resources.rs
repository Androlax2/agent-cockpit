use super::*;
use crate::{codex, paths, subscription_fallback};

#[test]
fn missing_user_collectors_resolve_to_the_private_arch_payload() {
    let _fixture = Fixture::new();
    for (name, resolve) in [
        ("claude-usage.sh", paths::usage_script as fn() -> PathBuf),
        ("codex-session.py", paths::codex_session_script),
        ("codex-usage.sh", paths::codex_usage_script),
        ("native-subscription.py", paths::native_subscription_script),
    ] {
        assert_eq!(
            resolve(),
            PathBuf::from(format!(
                "/usr/lib/agent-cockpit/config/waybar/.config/waybar/scripts/{name}"
            ))
        );
    }
}

#[test]
fn existing_user_collectors_keep_priority_over_packaged_defaults() {
    let fixture = Fixture::new();
    for (name, resolve) in [
        ("claude-usage.sh", paths::usage_script as fn() -> PathBuf),
        ("codex-session.py", paths::codex_session_script),
        ("codex-usage.sh", paths::codex_usage_script),
        ("native-subscription.py", paths::native_subscription_script),
    ] {
        let script = fixture.root.join("home/.config/waybar/scripts").join(name);
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "user collector").unwrap();
        assert_eq!(resolve(), script);
    }
}

#[test]
fn a_broken_user_collector_link_is_not_silently_replaced() {
    let fixture = Fixture::new();
    let script = fixture
        .root
        .join("home/.config/waybar/scripts/claude-usage.sh");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("missing-user-collector", &script).unwrap();
    assert_eq!(paths::usage_script(), script);
}

#[test]
fn codex_history_uses_the_packaged_collector_without_a_user_copy() {
    let fixture = Fixture::new();
    fixture.shim(
        "python3",
        &format!(
            "[ \"$1\" = /usr/lib/agent-cockpit/config/waybar/.config/waybar/scripts/codex-session.py ] || exit 2\necho '{}'",
            json!({"thread":SESSION_A,"messages":[]})
        ),
    );
    let history = codex::session_snapshot(SESSION_A).unwrap();
    assert_eq!(history["thread"], SESSION_A);
}

#[test]
fn native_subscription_uses_the_packaged_collector_without_a_user_copy() {
    let fixture = Fixture::new();
    fixture.shim(
        "python3",
        &format!(
            "[ \"$1\" = /usr/lib/agent-cockpit/config/waybar/.config/waybar/scripts/native-subscription.py ] || exit 2\necho '{}'",
            json!({"account":"a".repeat(64),"usage":{},"fetched":util::now() as i64})
        ),
    );
    let evidence =
        subscription_fallback::native::collect("claude", fixture.root.to_str().unwrap()).unwrap();
    assert_eq!(evidence["account"], "a".repeat(64));
}

#[test]
fn native_claude_discovery_uses_the_packaged_collector_without_a_user_copy() {
    let fixture = Fixture::new();
    fixture.shim(
        "python3",
        &format!(
            "[ \"$1\" = /usr/lib/agent-cockpit/config/waybar/.config/waybar/scripts/native-subscription.py ] || exit 2\nif [ \"$2\" = discover-claude ]; then echo '{}'; else echo '{}'; fi",
            json!({"model":"claude-opus-fixture","effort":"high"}),
            json!({"account":"a".repeat(64),"usage":{},"fetched":util::now() as i64})
        ),
    );
    let selection =
        subscription_fallback::native::candidate("claude", fixture.root.to_str().unwrap()).unwrap();
    assert_eq!(selection.model, "claude-opus-fixture");
}
