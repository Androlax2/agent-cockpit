//! The hook subscription a launch passes, and the trust hash Codex checks it against.

mod common;

use cockpit_codex::hooks::{self, HOOK_EVENTS, HookEvent};

fn event(name: &str) -> HookEvent {
    *HOOK_EVENTS.iter().find(|event| event.name == name).unwrap()
}

/// Hashes `hooks/list` reported for `/tmp/codex-exp/hook.sh` on Codex 0.161.0 with the
/// default ten-minute timeout, before the adapter chose its own.
#[test]
fn the_hash_is_the_one_codex_reports_for_the_same_hook() {
    let command = "/tmp/codex-exp/hook.sh";
    let with_default_timeout = |name: &str| HookEvent {
        timeout_seconds: 600,
        ..event(name)
    };

    assert_eq!(
        hooks::trusted_hash(&with_default_timeout("SessionStart"), command),
        "sha256:c2bca538878cb170922de8e124ae2bd55fa3e40a02b76e333584a44dd20a4f2c"
    );
    assert_eq!(
        hooks::trusted_hash(&with_default_timeout("UserPromptSubmit"), command),
        "sha256:80e6659a869899f78e13f4f8709dddb4f21cf4e47e2178300d57c3d5f0600fc9"
    );
    assert_eq!(
        hooks::trusted_hash(&with_default_timeout("Stop"), command),
        "sha256:6adf7eaae598616f93ba6402c49dffb70f91d7f7f096babfad62f83950594539"
    );
}

#[test]
fn another_command_gets_another_hash() {
    let start = event("SessionStart");

    assert_ne!(
        hooks::trusted_hash(&start, "'/a/cockpit-codex-hook' '/a/cockpit'"),
        hooks::trusted_hash(&start, "'/b/cockpit-codex-hook' '/a/cockpit'")
    );
}

#[test]
fn the_trust_key_is_codexs_session_flags_key_for_the_first_hook_of_the_event() {
    assert_eq!(
        hooks::trust_key(&event("PostToolUse")),
        "/<session-flags>/config.toml:post_tool_use:0:0"
    );
}

#[test]
fn the_subscription_configures_each_event_then_trusts_each_hook() {
    let command = common::bridge().hook_command();

    let values = hooks::subscription(&command);

    assert_eq!(values.len(), HOOK_EVENTS.len() + 1);
    assert_eq!(
        values[0],
        "hooks.SessionStart=[{hooks=[{type=\"command\",\
         command=\"'/opt/cockpit/bin/cockpit-codex-hook' '/opt/cockpit/bin/cockpit'\",\
         timeout=10}]}]"
    );
    let state = values.last().unwrap();
    for event in HOOK_EVENTS {
        let entry = format!(
            "\"{}\"={{trusted_hash=\"{}\"}}",
            hooks::trust_key(&event),
            hooks::trusted_hash(&event, &command)
        );
        assert!(state.contains(&entry), "{event:?} in {state}");
    }
}

#[test]
fn quotes_and_backslashes_in_the_command_are_escaped_for_toml() {
    let values = hooks::subscription(r#"'/opt/a"b\c/cockpit-codex-hook' '/x'"#);

    assert!(
        values[0].contains(r#"command="'/opt/a\"b\\c/cockpit-codex-hook' '/x'""#),
        "{}",
        values[0]
    );
}

/// Codex caps an `Interrupt` hook at 3 seconds; a larger value would hash differently from
/// the one it runs.
#[test]
fn the_interrupt_hook_asks_no_more_than_codexs_cap() {
    assert_eq!(event("Interrupt").timeout_seconds, 3);
}
