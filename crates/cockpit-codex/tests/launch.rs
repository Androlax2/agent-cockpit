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
        remote_control: false,
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
    assert_eq!(values_of(&words, "--cockpit"), ["/opt/cockpit/bin/cockpit"]);
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

    assert_eq!(
        configs[configs.len() - 2..],
        [
            format!(
                "mcp_servers.cockpit.command=\"{}\"",
                common::bridge().cockpit_program.display()
            ),
            "mcp_servers.cockpit.args=[\"mcp\"]".to_owned(),
        ]
    );
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
        None,
        Some("--help me"),
    ));

    assert_eq!(
        words,
        ["--remote", &format!("unix://{SOCKET}"), "--", "--help me"]
    );
}

#[test]
fn the_tui_of_a_resumed_conversation_resumes_its_thread_after_the_separator() {
    let words = common::words(&cockpit_codex::tui_arguments(
        Path::new(SOCKET),
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
    assert_eq!(common::words(&command.arguments), ["--no-daemon"]);
    assert_eq!(command.conversation, None);
}

#[test]
fn without_the_bridge_a_thread_id_that_looks_like_a_flag_stays_positional() {
    let conversation = Conversation::Resume(ConversationId::new("--last").unwrap());
    let command = without_bridge().launch(&request(&conversation)).unwrap();
    let words = common::words(&command.arguments);

    assert_eq!(words, ["resume", "--no-daemon", "--", "--last"]);
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
fn an_effort_level_is_refused_naming_the_capability() {
    let refused = common::codex()
        .launch(&LaunchRequest {
            effort: Some("high"),
            ..request(&Conversation::New)
        })
        .unwrap_err();

    assert_eq!(refused.reason, "it does not report the `effort` capability");
}

#[test]
fn remote_control_is_refused_naming_the_capability() {
    let refused = common::codex()
        .launch(&LaunchRequest {
            remote_control: true,
            ..request(&Conversation::New)
        })
        .unwrap_err();

    assert_eq!(
        refused.reason,
        "it does not report the `remote-control` capability"
    );
}
