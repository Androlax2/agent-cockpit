//! The words a launch gives OpenCode: its own server, the conversation, the first message
//! and the plugin bridge.

mod common;

use std::ffi::{OsStr, OsString};
use std::path::Path;

use cockpit_opencode::{OpenCode, PLUGIN_VARIABLES, SERVER_HOSTNAME, SERVER_USERNAME, Server};
use cockpit_provider_api::{Conversation, ConversationId, LaunchRequest, Provider};

fn request(conversation: &Conversation) -> LaunchRequest<'_> {
    LaunchRequest {
        working_dir: Path::new("/work"),
        session_dir: Path::new("/run/cockpit/sessions/s-1"),
        conversation,
        first_message: None,
        effort: None,
        model: None,
        remote_control: false,
        compaction_threshold: None,
        inherited_trust: None,
    }
}

fn words(arguments: &[OsString]) -> Vec<String> {
    arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_new_conversation_runs_its_own_server_on_a_loopback_port() {
    let launch = common::opencode()
        .launch(&request(&Conversation::New))
        .unwrap();
    let words = words(&launch.arguments);

    assert_eq!(words[0], "--port");
    assert!(words[1].parse::<u16>().unwrap() > 0, "{words:?}");
    assert_eq!(words[2..4], ["--hostname", SERVER_HOSTNAME]);
    assert_eq!(launch.program, Path::new("/usr/bin/opencode"));
}

#[test]
fn a_new_conversation_is_unnamed_until_its_first_report() {
    let launch = common::opencode()
        .launch(&request(&Conversation::New))
        .unwrap();

    assert_eq!(launch.conversation, None);
    assert!(!words(&launch.arguments).contains(&"--session".to_owned()));
}

#[test]
fn each_launch_gets_its_own_password() {
    let opencode = common::opencode();
    let first = opencode.launch(&request(&Conversation::New)).unwrap();
    let second = opencode.launch(&request(&Conversation::New)).unwrap();

    let password = common::variable(&first, "OPENCODE_SERVER_PASSWORD").unwrap();
    assert_eq!(password.len(), 32);
    assert!(password.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(
        Some(password),
        common::variable(&second, "OPENCODE_SERVER_PASSWORD")
    );
    assert_eq!(
        common::variable(&first, "OPENCODE_SERVER_USERNAME"),
        Some(SERVER_USERNAME)
    );
}

#[test]
fn the_server_is_read_back_from_its_launch() {
    let launch = common::opencode()
        .launch(&request(&Conversation::New))
        .unwrap();
    let port: u16 = words(&launch.arguments)[1].parse().unwrap();

    let server = Server::from_launch(&launch).unwrap();

    assert_eq!(server.port(), port);
    assert_eq!(
        Server::from_launch(&launch),
        Some(Server::new(
            port,
            common::variable(&launch, "OPENCODE_SERVER_PASSWORD")
                .unwrap()
                .to_owned()
        ))
    );
}

#[test]
fn a_resumed_conversation_runs_its_session_and_keeps_its_id() {
    let id = ConversationId::new("ses_1c4f2e8a3ffeJ7qkR2mWbXzD0v").unwrap();
    let conversation = Conversation::Resume(id.clone());

    let launch = common::opencode().launch(&request(&conversation)).unwrap();

    assert_eq!(
        words(&launch.arguments)[4..],
        ["--session", "ses_1c4f2e8a3ffeJ7qkR2mWbXzD0v"]
    );
    assert_eq!(launch.conversation, Some(id));
}

#[test]
fn a_resumed_id_that_is_not_an_opencode_session_is_refused() {
    for id in ["conversation-1", "ses/../../x", "ses_a?b"] {
        let conversation = Conversation::Resume(ConversationId::new(id).unwrap());

        let error = common::opencode()
            .launch(&request(&conversation))
            .unwrap_err();

        assert!(
            error.reason.contains("not an OpenCode session id"),
            "{error}"
        );
    }
}

#[test]
fn the_first_message_goes_as_the_prompt_in_one_word_even_when_it_starts_with_a_dash() {
    let launch = common::opencode()
        .launch(&LaunchRequest {
            first_message: Some("--fix the parser"),
            ..request(&Conversation::New)
        })
        .unwrap();

    assert_eq!(words(&launch.arguments)[4..], ["--prompt=--fix the parser"]);
}

#[test]
fn a_launch_with_the_bridge_loads_the_plugin_into_its_own_server() {
    let launch = common::opencode()
        .launch(&request(&Conversation::New))
        .unwrap();
    let port = words(&launch.arguments)[1].clone();
    let config: serde_json::Value =
        serde_json::from_str(common::variable(&launch, "OPENCODE_CONFIG_CONTENT").unwrap())
            .unwrap();
    let bridge = common::bridge();

    assert_eq!(
        config["plugin"],
        serde_json::json!([bridge.plugin.to_str().unwrap()])
    );
    let [bridge_variable, cockpit_variable, port_variable] = PLUGIN_VARIABLES;
    assert_eq!(
        common::variable(&launch, bridge_variable),
        bridge.bridge_program.to_str()
    );
    assert_eq!(
        common::variable(&launch, cockpit_variable),
        bridge.cockpit_program.to_str()
    );
    assert_eq!(
        common::variable(&launch, port_variable),
        Some(port.as_str())
    );
}

#[test]
fn the_owners_inline_config_is_kept_and_the_plugin_joins_its_plugins() {
    let bridge = common::bridge();
    let owner = r#"{"model":"anthropic/claude-sonnet-5-5","plugin":["their-plugin"]}"#;

    let config: serde_json::Value = serde_json::from_str(
        &common::opencode()
            .inline_config(Some(OsStr::new(owner)), None)
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        config,
        serde_json::json!({
            "model": "anthropic/claude-sonnet-5-5",
            "plugin": ["their-plugin", bridge.plugin.to_str().unwrap()],
            "mcp": { "torlek": { "type": "local", "command": ["/opt/cockpit/bin/cockpit", "mcp"] } },
        })
    );
}

/// #419: the agent reaches Cockpit's MCP server without configuring anything; the owner's
/// own servers stay.
#[test]
fn a_launch_with_the_bridge_registers_cockpits_mcp_server_beside_the_owners() {
    let owner = r#"{"mcp":{"theirs":{"type":"remote","url":"https://example.invalid/mcp"}}}"#;

    let config: serde_json::Value = serde_json::from_str(
        &common::opencode()
            .inline_config(Some(OsStr::new(owner)), None)
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        config["mcp"],
        serde_json::json!({
            "theirs": { "type": "remote", "url": "https://example.invalid/mcp" },
            "torlek": { "type": "local", "command": ["/opt/cockpit/bin/cockpit", "mcp"] },
        })
    );
}

/// #419: OpenCode 1.19.5 scans each `skills.paths` folder for `**/SKILL.md`
/// (`packages/opencode/src/skill/index.ts`), so the bundle's `cockpit` skill is listed
/// without the agent asking; the owner's own paths stay.
#[test]
fn a_launch_adds_the_instruction_bundle_to_the_owners_skill_paths() {
    let opencode = common::opencode().with_instruction_skills("/state/agent-context".into());
    let owner = r#"{"skills":{"paths":["~/my-skills"],"urls":["https://example.invalid/s/"]}}"#;

    let config: serde_json::Value = serde_json::from_str(
        &opencode
            .inline_config(Some(OsStr::new(owner)), None)
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        config["skills"],
        serde_json::json!({
            "paths": ["~/my-skills", "/state/agent-context"],
            "urls": ["https://example.invalid/s/"],
        })
    );
}

#[test]
fn a_launch_carries_the_instruction_skills_in_its_inline_config() {
    let launch = common::opencode()
        .with_instruction_skills("/state/agent-context".into())
        .launch(&request(&Conversation::New))
        .unwrap();
    let config: serde_json::Value =
        serde_json::from_str(common::variable(&launch, "OPENCODE_CONFIG_CONTENT").unwrap())
            .unwrap();

    assert_eq!(
        config["skills"],
        serde_json::json!({ "paths": ["/state/agent-context"] })
    );
}

#[test]
fn without_instruction_skills_the_inline_config_names_no_skill_path() {
    let config: serde_json::Value =
        serde_json::from_str(&common::opencode().inline_config(None, None).unwrap()).unwrap();

    assert_eq!(config.get("skills"), None);
}

#[test]
fn an_owners_skills_the_bundle_cannot_join_are_refused() {
    let opencode = common::opencode().with_instruction_skills("/state/agent-context".into());

    for owner in [r#"{"skills":[]}"#, r#"{"skills":{"paths":"one"}}"#] {
        let refusal = opencode
            .inline_config(Some(OsStr::new(owner)), None)
            .unwrap_err();

        assert!(
            refusal.reason.starts_with("OPENCODE_CONFIG_CONTENT "),
            "{owner}: {refusal:?}"
        );
    }
}

#[test]
fn a_launch_denies_the_model_each_hidden_skill() {
    let launch = common::opencode()
        .with_hidden_skills(vec!["wait-what".into(), "other".into()])
        .launch(&request(&Conversation::New))
        .unwrap();
    let config: serde_json::Value =
        serde_json::from_str(common::variable(&launch, "OPENCODE_CONFIG_CONTENT").unwrap())
            .unwrap();

    assert_eq!(
        config["permission"],
        serde_json::json!({ "skill": { "wait-what": "deny", "other": "deny" } })
    );
}

#[test]
fn the_owners_skill_permissions_are_kept_beside_the_denied_skills() {
    let owner = r#"{"permission":{"bash":"ask","skill":{"theirs":"ask"}}}"#;
    let config: serde_json::Value = serde_json::from_str(
        &common::opencode()
            .with_hidden_skills(vec!["wait-what".into()])
            .inline_config(Some(OsStr::new(owner)), None)
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        config["permission"],
        serde_json::json!({
            "bash": "ask",
            "skill": { "theirs": "ask", "wait-what": "deny" },
        })
    );
}

#[test]
fn without_hidden_skills_the_inline_config_denies_nothing() {
    let owner = r#"{"permission":"ask"}"#;
    let config: serde_json::Value = serde_json::from_str(
        &common::opencode()
            .inline_config(Some(OsStr::new(owner)), None)
            .unwrap(),
    )
    .unwrap();

    assert_eq!(config["permission"], "ask");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            &common::opencode().inline_config(None, None).unwrap()
        )
        .unwrap()
        .get("permission"),
        None
    );
}

#[test]
fn an_owners_permissions_the_denied_skills_cannot_join_are_refused() {
    let opencode = common::opencode().with_hidden_skills(vec!["wait-what".into()]);

    for owner in [
        r#"{"permission":"ask"}"#,
        r#"{"permission":{"skill":"ask"}}"#,
    ] {
        let refusal = opencode
            .inline_config(Some(OsStr::new(owner)), None)
            .unwrap_err();

        assert!(
            refusal.reason.starts_with("OPENCODE_CONFIG_CONTENT "),
            "{owner}: {refusal:?}"
        );
    }
}

#[test]
fn an_owners_inline_config_the_plugin_cannot_join_is_refused() {
    for owner in [
        "not json",
        "[]",
        r#"{"plugin":"one"}"#,
        r#"{"mcp":[]}"#,
        r#"{"mcp":{"torlek":{"type":"local","command":["theirs"]}}}"#,
    ] {
        let refusal = common::opencode()
            .inline_config(Some(OsStr::new(owner)), None)
            .unwrap_err();

        assert!(
            refusal.reason.starts_with("OPENCODE_CONFIG_CONTENT "),
            "{owner}: {refusal:?}"
        );
    }
}

#[test]
fn a_launch_without_the_bridge_loads_no_plugin() {
    // Instruction skills alone never touch the owner's inline configuration.
    let opencode = OpenCode::new("/usr/bin/opencode".into(), None, None)
        .with_instruction_skills("/state/agent-context".into());

    let launch = opencode.launch(&request(&Conversation::New)).unwrap();

    assert_eq!(common::variable(&launch, "OPENCODE_CONFIG_CONTENT"), None);
    for name in PLUGIN_VARIABLES {
        assert_eq!(common::variable(&launch, name), None, "{name}");
    }
}

#[test]
fn a_compaction_threshold_is_the_compaction_reserve_of_the_inline_config() {
    for (threshold, reserved) in [
        ("20000", serde_json::json!(20000)),
        ("1", serde_json::json!(1)),
    ] {
        let launch = common::opencode()
            .launch(&LaunchRequest {
                compaction_threshold: Some(threshold),
                ..request(&Conversation::New)
            })
            .unwrap();
        let config: serde_json::Value =
            serde_json::from_str(common::variable(&launch, "OPENCODE_CONFIG_CONTENT").unwrap())
                .unwrap();

        assert_eq!(
            config["compaction"],
            serde_json::json!({ "reserved": reserved })
        );
    }
}

#[test]
fn without_a_compaction_threshold_the_inline_config_names_no_compaction() {
    let config: serde_json::Value =
        serde_json::from_str(&common::opencode().inline_config(None, None).unwrap()).unwrap();

    assert_eq!(config.get("compaction"), None);
}

#[test]
fn the_owners_compaction_settings_are_kept_and_their_reserve_replaced() {
    let owner = r#"{"compaction":{"auto":false,"reserved":5}}"#;

    let config: serde_json::Value = serde_json::from_str(
        &common::opencode()
            .inline_config(Some(OsStr::new(owner)), Some("20000"))
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        config["compaction"],
        serde_json::json!({ "auto": false, "reserved": 20000 })
    );
}

#[test]
fn a_compaction_threshold_without_the_bridge_sets_the_inline_config_without_a_plugin() {
    let opencode = OpenCode::new("/usr/bin/opencode".into(), None, None);

    let launch = opencode
        .launch(&LaunchRequest {
            compaction_threshold: Some("20000"),
            ..request(&Conversation::New)
        })
        .unwrap();

    let config: serde_json::Value =
        serde_json::from_str(common::variable(&launch, "OPENCODE_CONFIG_CONTENT").unwrap())
            .unwrap();
    assert_eq!(
        config,
        serde_json::json!({ "compaction": { "reserved": 20000 } })
    );
    for name in PLUGIN_VARIABLES {
        assert_eq!(common::variable(&launch, name), None, "{name}");
    }
}

#[test]
/// The `"15%"` form is OpenCode's since 2026-10-30 (after 2.0.0), not the tested 1.19.5's, so
/// it is refused rather than passed to a version that would refuse its whole configuration.
fn a_compaction_threshold_that_is_not_a_token_count_is_refused() {
    for threshold in [
        "abc", "", "-5", "+5", "15%", "20 000", "200k", "0", "007", "1.5",
    ] {
        let refusal = common::opencode()
            .launch(&LaunchRequest {
                compaction_threshold: Some(threshold),
                ..request(&Conversation::New)
            })
            .unwrap_err();

        assert_eq!(
            refusal.reason,
            format!(
                "compaction threshold {threshold:?} is not an OpenCode `compaction.reserved`: a \
                 token count above zero"
            )
        );
    }
}

#[test]
fn an_owners_compaction_that_is_not_an_object_is_refused() {
    let refusal = common::opencode()
        .inline_config(Some(OsStr::new(r#"{"compaction":"15%"}"#)), Some("20000"))
        .unwrap_err();

    assert_eq!(
        refusal.reason,
        "OPENCODE_CONFIG_CONTENT has a `compaction` that is not an object, so Torlek's \
         settings cannot join it"
    );
}

#[test]
fn an_effort_or_remote_control_is_refused_naming_its_capability() {
    let opencode = common::opencode();

    let effort = opencode
        .launch(&LaunchRequest {
            effort: Some("high"),
            ..request(&Conversation::New)
        })
        .unwrap_err();
    let remote = opencode
        .launch(&LaunchRequest {
            remote_control: true,
            compaction_threshold: None,
            ..request(&Conversation::New)
        })
        .unwrap_err();

    assert_eq!(effort.reason, "it does not report the `effort` capability");
    assert_eq!(
        remote.reason,
        "it does not report the `remote-control` capability"
    );
}
