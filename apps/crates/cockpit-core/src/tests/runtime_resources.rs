use super::*;
use crate::{codex, paths, subscription_fallback};

#[test]
fn desktop_assets_resolve_to_package_defaults_without_user_copies() {
    let _fixture = Fixture::new();
    assert_eq!(
        paths::osd_style(),
        PathBuf::from("/usr/lib/agent-cockpit/assets/osd/style.css")
    );
    assert_eq!(
        paths::idle_style(),
        PathBuf::from("/usr/lib/agent-cockpit/assets/idle/style.css")
    );
    assert_eq!(
        paths::osd_icon("bell.svg"),
        PathBuf::from("/usr/lib/agent-cockpit/assets/icons/bell.svg")
    );
}

#[test]
fn desktop_overrides_keep_priority_per_file_including_broken_links() {
    let fixture = Fixture::new();
    for (relative, resolve) in [
        (".config/osd/style.css", paths::osd_style as fn() -> PathBuf),
        (".config/hypr/scripts/claude-idle.css", paths::idle_style),
    ] {
        let user_path = fixture.root.join("home").join(relative);
        fs::create_dir_all(user_path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("missing-user-style", &user_path).unwrap();
        assert_eq!(resolve(), user_path);
    }
    let user_icon = fixture.root.join("home/.config/waybar/icons/bell.svg");
    fs::create_dir_all(user_icon.parent().unwrap()).unwrap();
    fs::write(&user_icon, "user icon").unwrap();
    assert_eq!(paths::osd_icon("bell.svg"), user_icon);
    assert_eq!(
        paths::osd_icon("brightness.svg"),
        PathBuf::from("/usr/lib/agent-cockpit/assets/icons/brightness.svg")
    );
}

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
