//! The command the adapter builds for each launch.

mod common;

use std::ffi::OsString;
use std::path::Path;

use cockpit_provider_api::{Conversation, ConversationId, LaunchRequest, Provider};

fn request(conversation: &Conversation) -> LaunchRequest<'_> {
    LaunchRequest {
        working_dir: Path::new("/work"),
        conversation,
        first_message: None,
        effort: None,
        remote_control: false,
    }
}

fn words(arguments: &[OsString]) -> Vec<String> {
    arguments
        .iter()
        .map(|argument| argument.to_str().unwrap().to_owned())
        .collect()
}

fn value_after<'a>(words: &'a [String], flag: &str) -> Option<&'a str> {
    let index = words.iter().position(|word| word == flag)?;
    words.get(index + 1).map(String::as_str)
}

fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.iter().map(|group| group.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12]
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        })
        && groups[2].starts_with('4')
}

#[test]
fn a_new_conversation_runs_the_program_under_a_session_id_it_names() {
    let command = common::claude()
        .launch(&request(&Conversation::New))
        .unwrap();
    let words = words(&command.arguments);

    assert_eq!(command.program, Path::new("/usr/bin/claude"));
    let id = value_after(&words, "--session-id").unwrap();
    assert!(is_uuid(id), "{id}");
    assert_eq!(command.conversation, Some(ConversationId::new(id).unwrap()));
}

#[test]
fn two_new_conversations_get_different_ids() {
    let claude = common::claude();
    let first = claude.launch(&request(&Conversation::New)).unwrap();
    let second = claude.launch(&request(&Conversation::New)).unwrap();

    assert_ne!(first.conversation, second.conversation);
}

#[test]
fn a_resumed_conversation_keeps_its_id() {
    let id = ConversationId::new("0b7f3c1e-1f6a-4c55-a2a1-9e3c7c0d1a2b").unwrap();
    let conversation = Conversation::Resume(id.clone());
    let command = common::claude().launch(&request(&conversation)).unwrap();
    let words = words(&command.arguments);

    assert_eq!(value_after(&words, "--resume"), Some(id.as_str()));
    assert!(!words.contains(&"--session-id".to_owned()));
    assert_eq!(command.conversation, Some(id));
}

#[test]
fn the_first_message_comes_last_after_the_end_of_options() {
    let launch = LaunchRequest {
        first_message: Some("--looks like a flag"),
        ..request(&Conversation::New)
    };
    let words = words(&common::claude().launch(&launch).unwrap().arguments);

    assert_eq!(words[words.len() - 2..], ["--", "--looks like a flag"]);
}

#[test]
fn every_launch_loads_the_hook_bridge_settings() {
    let words = words(
        &common::claude()
            .launch(&request(&Conversation::New))
            .unwrap()
            .arguments,
    );
    let settings: serde_json::Value =
        serde_json::from_str(value_after(&words, "--settings").unwrap()).unwrap();

    let stop = &settings["hooks"]["Stop"][0]["hooks"][0];
    assert_eq!(stop["type"], "command");
    assert_eq!(
        stop["command"],
        "'/opt/cockpit/bin/cockpit-claude-hook' '/opt/cockpit/bin/cockpit'"
    );
}

#[test]
fn a_bridge_path_with_a_quote_stays_one_shell_word() {
    let mut bridge = common::bridge();
    bridge.hook_program = "/opt/it's/hook".into();
    let claude = cockpit_claude::ClaudeCode::new("/usr/bin/claude".into(), None, Some(bridge));
    let words = words(
        &claude
            .launch(&request(&Conversation::New))
            .unwrap()
            .arguments,
    );
    let settings: serde_json::Value =
        serde_json::from_str(value_after(&words, "--settings").unwrap()).unwrap();

    assert_eq!(
        settings["hooks"]["Stop"][0]["hooks"][0]["command"],
        r"'/opt/it'\''s/hook' '/opt/cockpit/bin/cockpit'"
    );
}

#[test]
fn without_its_hook_bridge_a_launch_subscribes_no_hooks() {
    let claude = cockpit_claude::ClaudeCode::new("/usr/bin/claude".into(), None, None);
    let words = words(
        &claude
            .launch(&request(&Conversation::New))
            .unwrap()
            .arguments,
    );

    assert!(!words.contains(&"--settings".to_owned()), "{words:?}");
}

#[test]
fn an_effort_level_is_passed_to_the_program() {
    let launch = LaunchRequest {
        effort: Some("xhigh"),
        ..request(&Conversation::New)
    };
    let words = words(&common::claude().launch(&launch).unwrap().arguments);

    assert_eq!(value_after(&words, "--effort"), Some("xhigh"));
}

#[test]
fn an_effort_level_claude_code_does_not_have_is_refused() {
    let launch = LaunchRequest {
        effort: Some("extreme"),
        ..request(&Conversation::New)
    };
    let error = common::claude().launch(&launch).unwrap_err();

    assert_eq!(
        error.reason,
        "effort \"extreme\" is not one of low, medium, high, xhigh, max"
    );
}

#[test]
fn remote_control_is_turned_on_at_launch() {
    let launch = LaunchRequest {
        remote_control: true,
        first_message: Some("hello"),
        ..request(&Conversation::New)
    };
    let words = words(&common::claude().launch(&launch).unwrap().arguments);

    // Its optional name would take the next plain word; a flag always follows it.
    let index = words
        .iter()
        .position(|word| word == "--remote-control")
        .unwrap();
    assert!(words[index + 1].starts_with("--"), "{words:?}");
}

/// See `LAUNCH_ENVIRONMENT` (#184).
#[test]
fn every_launch_turns_off_claude_codes_background_daemon() {
    let resumed = ConversationId::new("0b7f3c1e-1f6a-4c55-a2a1-9e3c7c0d1a2b").unwrap();
    for conversation in [Conversation::New, Conversation::Resume(resumed)] {
        let command = common::claude().launch(&request(&conversation)).unwrap();

        assert!(
            command
                .environment
                .contains(&("CLAUDE_CODE_DISABLE_AGENT_VIEW".into(), "1".into())),
            "{conversation:?}: {:?}",
            command.environment
        );
    }
}

/// See `LAUNCH_ENVIRONMENT` (#219): Claude Code kills a `SessionEnd` hook after 1.5 s by
/// default, which would lose the end a `/clear` announces while the daemon is busy.
#[test]
fn every_launch_gives_the_session_end_hook_the_reports_deadline() {
    let command = common::claude()
        .launch(&request(&Conversation::New))
        .unwrap();

    assert!(
        command.environment.contains(&(
            "CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS".into(),
            "10000".into()
        )),
        "{:?}",
        command.environment
    );
}
