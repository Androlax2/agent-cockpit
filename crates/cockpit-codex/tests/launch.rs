//! The command the adapter builds for each launch: the session launcher with the bridge, the
//! TUI alone without it, and the words the launcher gives the app-server and the TUI.

mod common;

use std::path::{Path, PathBuf};

use cockpit_codex::Codex;
use cockpit_provider_api::{Conversation, ConversationId, LaunchRequest, Provider};

const SESSION_DIR: &str = "/run/cockpit/sessions/s0123456789ab";
const SOCKET: &str = "/run/cockpit/sessions/s0123456789ab/codex.sock";

fn request(conversation: &Conversation) -> LaunchRequest<'_> {
    LaunchRequest {
        working_dir: Path::new("/work"),
        session_dir: Path::new(SESSION_DIR),
        conversation,
        first_message: None,
        effort: None,
        model: None,
        remote_control: false,
        compaction_threshold: None,
    }
}

fn thread() -> ConversationId {
    ConversationId::new("01a19800-e8f3-70e3-b788-d9601b03debb").unwrap()
}

fn without_bridge() -> Codex {
    Codex::new(PathBuf::from("/usr/bin/codex"), None, None)
}

/// The values of every `flag` in `words`, in order.
fn values_of(words: &[String], flag: &str) -> Vec<String> {
    words
        .windows(2)
        .filter(|pair| pair[0] == flag)
        .map(|pair| pair[1].clone())
        .collect()
}

fn after_separator(words: &[String]) -> Vec<String> {
    words
        .iter()
        .skip_while(|word| *word != "--")
        .skip(1)
        .cloned()
        .collect()
}

#[test]
fn with_the_bridge_a_launch_runs_the_session_launcher_on_the_sessions_socket() {
    let command = common::codex()
        .launch(&request(&Conversation::New))
        .unwrap();
    let words = common::words(&command.arguments);

    assert_eq!(command.program, common::bridge().session_program);
    assert_eq!(values_of(&words, "--torlek"), ["/opt/cockpit/bin/cockpit"]);
    assert_eq!(values_of(&words, "--codex"), ["/usr/bin/codex"]);
    assert_eq!(values_of(&words, "--socket"), [SOCKET]);
}

#[test]
fn with_the_bridge_a_launch_subscribes_the_hook_bridge() {
    let command = common::codex()
        .launch(&request(&Conversation::New))
        .unwrap();

    let configs = values_of(&common::words(&command.arguments), "--config");
    let subscription = cockpit_codex::hooks::subscription(&common::bridge().hook_command());
    assert_eq!(configs[..subscription.len()], subscription);
}

/// #419: the agent reaches Cockpit's MCP server without configuring anything.
#[test]
fn with_the_bridge_a_launch_registers_cockpits_mcp_server() {
    let command = common::codex()
        .launch(&request(&Conversation::New))
        .unwrap();
    let configs = values_of(&common::words(&command.arguments), "--config");
    let mcp: Vec<&String> = configs
        .iter()
        .filter(|config| config.starts_with("mcp_servers.torlek."))
        .collect();

    assert_eq!(
        mcp[..2],
        [
            format!(
                "mcp_servers.torlek.command=\"{}\"",
                common::bridge().cockpit_program.display()
            ),
            "mcp_servers.torlek.args=[\"mcp\"]".to_owned(),
        ]
        .iter()
        .collect::<Vec<_>>()
    );
}

/// Codex starts a stdio MCP server with only `HOME`, `PATH` and the locale unless the server
/// names more: without the launch token `cockpit mcp` cannot tell which session it serves.
#[test]
fn cockpits_mcp_server_receives_the_launch_token_and_the_agent_socket() {
    let command = common::codex()
        .launch(&request(&Conversation::New))
        .unwrap();
    let configs = values_of(&common::words(&command.arguments), "--config");
    let passed = configs
        .iter()
        .find_map(|config| config.strip_prefix("mcp_servers.torlek.env_vars="))
        .unwrap_or_else(|| panic!("no env_vars for cockpit's MCP server: {configs:?}"));

    for variable in [
        "\"TORLEK_LAUNCH_TOKEN\"",
        "\"TORLEK_AGENT_SOCKET\"",
        "\"TORLEK_PROFILE\"",
        "\"XDG_RUNTIME_DIR\"",
    ] {
        assert!(
            passed.contains(variable),
            "{variable} missing from {passed}"
        );
    }
}

#[test]
fn a_new_conversation_names_no_thread() {
    let command = common::codex()
        .launch(&request(&Conversation::New))
        .unwrap();
    let words = common::words(&command.arguments);

    assert!(values_of(&words, "--resume").is_empty(), "{words:?}");
    assert_eq!(command.conversation, None);
}

#[test]
fn a_resumed_conversation_passes_its_thread_and_keeps_it() {
    let conversation = Conversation::Resume(thread());
    let command = common::codex().launch(&request(&conversation)).unwrap();

    assert_eq!(
        values_of(&common::words(&command.arguments), "--resume"),
        [thread().as_str()]
    );
    assert_eq!(command.conversation, Some(thread()));
}

#[test]
fn the_first_message_is_the_value_of_its_own_option() {
    let command = common::codex()
        .launch(&LaunchRequest {
            first_message: Some("-c fix the build"),
            ..request(&Conversation::New)
        })
        .unwrap();

    assert_eq!(
        values_of(&common::words(&command.arguments), "--message"),
        ["-c fix the build"]
    );
}

#[test]
fn the_app_server_listens_on_the_socket_with_hooks_enabled_and_the_hooks_of_the_launch() {
    let configs = ["hooks.Stop=[]".to_owned()];

    let words = common::words(&cockpit_codex::server_arguments(
        Path::new(SOCKET),
        &configs,
    ));

    assert_eq!(
        words,
        [
            "app-server",
            "--listen",
            &format!("unix://{SOCKET}"),
            "--enable",
            "hooks",
            "--config",
            "hooks.Stop=[]"
        ]
    );
}

#[test]
fn the_tui_of_a_new_conversation_joins_the_app_server_with_its_first_message_last() {
    let words = common::words(&cockpit_codex::tui_arguments(
        Path::new(SOCKET),
        &["model=\"gpt-6-astra\"".to_owned()],
        None,
        Some("--help me"),
    ));

    assert_eq!(
        words,
        [
            "--remote",
            &format!("unix://{SOCKET}"),
            "--config",
            "model=\"gpt-6-astra\"",
            "--",
            "--help me"
        ]
    );
}

#[test]
fn the_tui_of_a_resumed_conversation_resumes_its_thread_after_the_separator() {
    let words = common::words(&cockpit_codex::tui_arguments(
        Path::new(SOCKET),
        &[],
        Some(&thread()),
        None,
    ));

    assert_eq!(
        words,
        [
            "resume",
            "--remote",
            &format!("unix://{SOCKET}"),
            "--",
            thread().as_str()
        ]
    );
}

#[test]
fn without_the_bridge_a_new_conversation_runs_codex_out_of_the_shared_daemon() {
    let command = without_bridge()
        .launch(&request(&Conversation::New))
        .unwrap();

    assert_eq!(command.program, Path::new("/usr/bin/codex"));
    assert_eq!(
        common::words(&command.arguments),
        [
            "--no-daemon",
            "--config",
            "sandbox_mode=\"workspace-write\"",
            "--config",
            "sandbox_workspace_write.network_access=true",
            "--config",
            "check_for_update_on_startup=false"
        ]
    );
    assert_eq!(command.conversation, None);
}

#[test]
fn without_the_bridge_a_thread_id_that_looks_like_a_flag_stays_positional() {
    let conversation = Conversation::Resume(ConversationId::new("--last").unwrap());
    let command = without_bridge().launch(&request(&conversation)).unwrap();
    let words = common::words(&command.arguments);

    assert_eq!(after_separator(&words), ["--last"]);
    assert_eq!(words[..2], ["resume", "--no-daemon"]);
}

#[test]
fn without_the_bridge_the_first_message_comes_last_after_the_separator() {
    let conversation = Conversation::Resume(thread());
    let command = without_bridge()
        .launch(&LaunchRequest {
            first_message: Some("-c fix the build"),
            ..request(&conversation)
        })
        .unwrap();

    assert_eq!(
        after_separator(&common::words(&command.arguments)),
        [thread().as_str(), "-c fix the build"]
    );
}

#[test]
fn a_launch_adds_no_environment() {
    let command = common::codex()
        .launch(&request(&Conversation::New))
        .unwrap();

    assert!(command.environment.is_empty());
}

#[test]
fn with_the_bridge_a_compaction_threshold_is_one_more_config_value_for_the_app_server() {
    let command = common::codex()
        .launch(&LaunchRequest {
            compaction_threshold: Some("75"),
            ..request(&Conversation::New)
        })
        .unwrap();

    let configs = values_of(&common::words(&command.arguments), "--config");
    assert!(
        configs.contains(&"model_post_turn_compact_threshold_percent=75".to_owned()),
        "{configs:?}"
    );
}

#[test]
fn without_the_bridge_a_compaction_threshold_is_a_config_value_before_the_separator() {
    let conversation = Conversation::Resume(thread());
    let command = without_bridge()
        .launch(&LaunchRequest {
            compaction_threshold: Some("0"),
            first_message: Some("go"),
            ..request(&conversation)
        })
        .unwrap();

    assert_eq!(
        common::words(&command.arguments),
        [
            "resume",
            "--no-daemon",
            "--config",
            "model_post_turn_compact_threshold_percent=0",
            "--config",
            "sandbox_mode=\"workspace-write\"",
            "--config",
            "sandbox_workspace_write.network_access=true",
            "--config",
            "check_for_update_on_startup=false",
            "--",
            thread().as_str(),
            "go"
        ]
    );
}

#[test]
fn without_a_compaction_threshold_no_launch_names_the_key() {
    for codex in [common::codex(), without_bridge()] {
        let command = codex.launch(&request(&Conversation::New)).unwrap();

        let words = common::words(&command.arguments);
        assert!(
            !words
                .iter()
                .any(|word| word.contains("model_post_turn_compact_threshold_percent")),
            "{words:?}"
        );
    }
}

#[test]
fn a_compaction_threshold_that_is_not_a_whole_percent_is_refused_before_the_launch() {
    for threshold in ["101", "-5", "+5", "50%", "abc", "", "0100", "05", "007"] {
        let refused = common::codex()
            .launch(&LaunchRequest {
                compaction_threshold: Some(threshold),
                ..request(&Conversation::New)
            })
            .unwrap_err();

        assert_eq!(
            refused.reason,
            format!(
                "compaction threshold {threshold:?} is not a Codex \
                 `model_post_turn_compact_threshold_percent`: a whole percent from 0 to 100, written plainly"
            )
        );
    }
}

/// #1281: a coordinator picks the model and effort per session, without the owner's
/// `~/.codex/config.toml`; the TUI gets them too, so a thread it starts asks for the same.
#[test]
fn with_the_bridge_a_model_and_effort_reach_both_the_app_server_and_the_tui() {
    let command = common::codex()
        .launch(&LaunchRequest {
            model: Some("gpt-6.1-terra"),
            effort: Some("xhigh"),
            ..request(&Conversation::New)
        })
        .unwrap();
    let words = common::words(&command.arguments);
    let chosen = [
        "model=\"gpt-6.1-terra\"".to_owned(),
        "model_reasoning_effort=\"xhigh\"".to_owned(),
    ];

    let server = values_of(&words, "--config");
    assert_eq!(server[server.len() - 2..], chosen);
    let tui = values_of(&words, "--tui-config");
    assert_eq!(tui[tui.len() - 3..tui.len() - 1], chosen);
}

#[test]
fn without_the_bridge_a_model_and_effort_are_config_values_before_the_separator() {
    let command = without_bridge()
        .launch(&LaunchRequest {
            model: Some("gpt-6-astra"),
            effort: Some("max"),
            first_message: Some("go"),
            ..request(&Conversation::New)
        })
        .unwrap();

    assert_eq!(
        common::words(&command.arguments),
        [
            "--no-daemon",
            "--config",
            "sandbox_mode=\"workspace-write\"",
            "--config",
            "sandbox_workspace_write.network_access=true",
            "--config",
            "model=\"gpt-6-astra\"",
            "--config",
            "model_reasoning_effort=\"max\"",
            "--config",
            "check_for_update_on_startup=false",
            "--",
            "go"
        ]
    );
}

/// #1281: Codex's "Update available" screen held a restarted session until the owner answered.
#[test]
fn with_the_bridge_the_tui_skips_the_update_screen() {
    let command = common::codex()
        .launch(&request(&Conversation::New))
        .unwrap();
    let words = common::words(&command.arguments);

    assert_eq!(
        values_of(&words, "--tui-config").last().map(String::as_str),
        Some("check_for_update_on_startup=false")
    );
    assert!(
        !values_of(&words, "--config").contains(&"check_for_update_on_startup=false".to_owned()),
        "the app-server shows no screen: {words:?}"
    );
}

/// #1281: Codex's default sandbox for an untrusted clone is read-only, and without the network
/// it refuses every `connect`, Torlek's agent socket included. The owner chose that every
/// session Torlek starts writes its workspace and has the network, whatever
/// `~/.codex/config.toml` says.
#[test]
fn every_launch_runs_its_commands_in_a_writable_workspace_with_the_network() {
    let sandbox = [
        "sandbox_mode=\"workspace-write\"".to_owned(),
        "sandbox_workspace_write.network_access=true".to_owned(),
    ];
    let bridged = common::words(
        &common::codex()
            .launch(&request(&Conversation::New))
            .unwrap()
            .arguments,
    );
    let direct = common::words(
        &without_bridge()
            .launch(&request(&Conversation::New))
            .unwrap()
            .arguments,
    );

    for (words, flag) in [
        (&bridged, "--config"),
        (&bridged, "--tui-config"),
        (&direct, "--config"),
    ] {
        let values = values_of(words, flag);
        assert!(
            sandbox.iter().all(|config| values.contains(config)),
            "{flag}: {values:?}"
        );
    }
}

/// The owner's `writable_roots` (an sccache folder, say) and approval policy stay theirs: a
/// `--config` array replaces the owner's list instead of adding to it.
#[test]
fn no_launch_sets_the_owners_writable_roots_or_approval_policy() {
    for codex in [common::codex(), without_bridge()] {
        let command = codex.launch(&request(&Conversation::New)).unwrap();

        let words = common::words(&command.arguments);
        assert!(
            !words
                .iter()
                .any(|word| word.contains("writable_roots") || word.starts_with("approval_policy")),
            "{words:?}"
        );
    }
}

#[test]
fn without_a_model_or_effort_no_launch_names_their_keys() {
    for codex in [common::codex(), without_bridge()] {
        let command = codex.launch(&request(&Conversation::New)).unwrap();

        let words = common::words(&command.arguments);
        assert!(
            !words
                .iter()
                .any(|word| word.starts_with("model=") || word.starts_with("model_reasoning")),
            "{words:?}"
        );
    }
}

#[test]
fn a_model_that_could_read_as_a_flag_or_breaks_its_shape_is_refused_before_the_launch() {
    for model in ["-m", "", "gpt 6", "gpt\"6", "gpt=6", &"g".repeat(201)] {
        let refused = common::codex()
            .launch(&LaunchRequest {
                model: Some(model),
                ..request(&Conversation::New)
            })
            .unwrap_err();

        assert!(
            refused
                .reason
                .starts_with(&format!("model {model:?} is not a Codex model name")),
            "{}",
            refused.reason
        );
    }
}

#[test]
fn an_effort_that_is_not_one_plain_word_is_refused_before_the_launch() {
    for effort in [
        "--high",
        "",
        "very high",
        "high\"",
        "x.high",
        &"h".repeat(65),
    ] {
        let refused = without_bridge()
            .launch(&LaunchRequest {
                effort: Some(effort),
                ..request(&Conversation::New)
            })
            .unwrap_err();

        assert!(
            refused.reason.starts_with(&format!(
                "effort {effort:?} is not a Codex reasoning effort"
            )),
            "{}",
            refused.reason
        );
    }
}

#[test]
fn remote_control_is_refused_naming_the_capability() {
    let refused = common::codex()
        .launch(&LaunchRequest {
            remote_control: true,
            compaction_threshold: None,
            ..request(&Conversation::New)
        })
        .unwrap_err();

    assert_eq!(
        refused.reason,
        "it does not report the `remote-control` capability"
    );
}
