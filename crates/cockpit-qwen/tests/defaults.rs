//! The system-defaults settings that subscribe the bridge to Qwen Code's hooks.

use std::path::Path;

use cockpit_qwen::{defaults_settings, hooks};

fn settings() -> serde_json::Value {
    serde_json::from_str(&defaults_settings(
        Path::new("/opt/torlek/bin/torlek-qwen-hook"),
        Path::new("/opt/torlek's bin/torlek"),
    ))
    .unwrap()
}

#[test]
fn every_normalized_event_runs_the_bridge_with_the_torlek_program() {
    let settings = settings();

    for event in hooks::HOOK_EVENTS {
        assert_eq!(
            settings["hooks"][event],
            serde_json::json!([{
                "hooks": [{
                    "type": "command",
                    "command": r#"'/opt/torlek/bin/torlek-qwen-hook' '/opt/torlek'\''s bin/torlek'"#,
                    "name": "torlek",
                }]
            }]),
            "{event}"
        );
    }
    assert_eq!(
        settings["hooks"].as_object().unwrap().len(),
        hooks::HOOK_EVENTS.len()
    );
}

/// The install is read-only in the sandbox, and an update is the owner's.
#[test]
fn the_update_check_is_off() {
    assert_eq!(
        settings()["general"],
        serde_json::json!({ "enableAutoUpdate": false })
    );
}

#[test]
fn the_defaults_name_nothing_else() {
    let settings = settings();
    let mut keys: Vec<&String> = settings.as_object().unwrap().keys().collect();
    keys.sort();

    assert_eq!(keys, ["general", "hooks"]);
}
