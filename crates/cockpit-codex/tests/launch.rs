//! The command the adapter builds for each launch.

mod common;

use std::path::{Path, PathBuf};

use cockpit_codex::Codex;
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

fn thread() -> ConversationId {
    ConversationId::new("01a19800-e8f3-70e3-b788-d9601b03debb").unwrap()
}

/// The words before `--`, without the hook subscription's `--enable`/`--config` pairs.
fn flags_without_hooks(words: &[String]) -> Vec<String> {
    let mut flags = Vec::new();
    let mut words = words.iter().take_while(|word| *word != "--");
    while let Some(word) = words.next() {
        if word == "--enable" || word == "--config" {
            words.next();
        } else {
            flags.push(word.clone());
        }
    }
    flags
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
fn a_new_conversation_runs_codex_out_of_the_shared_daemon_and_names_no_thread() {
    let command = common::codex()
        .launch(&request(&Conversation::New))
        .unwrap();
    let words = common::words(&command.arguments);

    assert_eq!(command.program, Path::new("/usr/bin/codex"));
    assert_eq!(flags_without_hooks(&words), ["--no-daemon"]);
    assert!(!words.contains(&"--".to_owned()), "{words:?}");
    assert_eq!(command.conversation, None);
}

#[test]
fn a_resumed_conversation_runs_resume_with_its_thread_after_the_separator() {
    let conversation = Conversation::Resume(thread());
    let command = common::codex().launch(&request(&conversation)).unwrap();
    let words = common::words(&command.arguments);

    assert_eq!(flags_without_hooks(&words), ["resume", "--no-daemon"]);
    assert_eq!(after_separator(&words), [thread().as_str()]);
    assert_eq!(command.conversation, Some(thread()));
}

#[test]
fn a_thread_id_that_looks_like_a_flag_stays_positional() {
    let conversation = Conversation::Resume(ConversationId::new("--last").unwrap());
    let command = common::codex().launch(&request(&conversation)).unwrap();
    let words = common::words(&command.arguments);

    assert_eq!(after_separator(&words), ["--last"]);
}

#[test]
fn the_first_message_comes_last_after_the_separator() {
    let conversation = Conversation::Resume(thread());
    let command = common::codex()
        .launch(&LaunchRequest {
            first_message: Some("-c fix the build"),
            ..request(&conversation)
        })
        .unwrap();
    let words = common::words(&command.arguments);

    assert_eq!(
        after_separator(&words),
        [thread().as_str(), "-c fix the build"]
    );
}

#[test]
fn a_new_conversation_with_a_first_message_passes_it_alone_after_the_separator() {
    let command = common::codex()
        .launch(&LaunchRequest {
            first_message: Some("hello"),
            ..request(&Conversation::New)
        })
        .unwrap();

    assert_eq!(
        after_separator(&common::words(&command.arguments)),
        ["hello"]
    );
}

#[test]
fn with_the_bridge_a_launch_enables_hooks_and_subscribes_the_bridge() {
    let command = common::codex()
        .launch(&request(&Conversation::New))
        .unwrap();
    let words = common::words(&command.arguments);
    let enabled: Vec<&str> = words
        .windows(2)
        .filter(|pair| pair[0] == "--enable")
        .map(|pair| pair[1].as_str())
        .collect();
    let configured: Vec<String> = words
        .windows(2)
        .filter(|pair| pair[0] == "--config")
        .map(|pair| pair[1].clone())
        .collect();

    assert_eq!(enabled, ["hooks"]);
    assert_eq!(
        configured,
        cockpit_codex::hooks::subscription(&common::bridge().hook_command())
    );
}

#[test]
fn without_the_bridge_a_launch_subscribes_no_hooks() {
    let codex = Codex::new(PathBuf::from("/usr/bin/codex"), None, None);
    let command = codex.launch(&request(&Conversation::New)).unwrap();
    let words = common::words(&command.arguments);

    assert_eq!(words, ["--no-daemon"]);
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

#[test]
fn the_hook_command_quotes_both_programs_for_the_shell() {
    let bridge = cockpit_codex::Bridge {
        hook_program: PathBuf::from("/opt/it's/cockpit-codex-hook"),
        cockpit_program: PathBuf::from("/opt/cockpit/bin/cockpit"),
    };

    assert_eq!(
        bridge.hook_command(),
        r"'/opt/it'\''s/cockpit-codex-hook' '/opt/cockpit/bin/cockpit'"
    );
}
