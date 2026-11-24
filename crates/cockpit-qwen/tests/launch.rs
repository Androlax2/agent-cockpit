//! The command the adapter builds for each launch.

mod common;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use cockpit_provider_api::{Conversation, ConversationId, LaunchRequest, Provider};
use cockpit_qwen::QwenCode;

fn request(conversation: &Conversation) -> LaunchRequest<'_> {
    LaunchRequest {
        working_dir: Path::new("/work"),
        session_dir: Path::new("/run/torlek/sessions/s-1"),
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
        .map(|argument| argument.to_str().unwrap().to_owned())
        .collect()
}

fn value_after<'a>(words: &'a [String], flag: &str) -> Option<&'a str> {
    let index = words.iter().position(|word| word == flag)?;
    words.get(index + 1).map(String::as_str)
}

fn is_uuid_v4(text: &str) -> bool {
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
    let command = common::qwen().launch(&request(&Conversation::New)).unwrap();
    let words = words(&command.arguments);

    assert_eq!(command.program, Path::new("/usr/bin/qwen"));
    let id = value_after(&words, "--session-id").unwrap();
    assert!(is_uuid_v4(id), "{id}");
    assert_eq!(command.conversation, Some(ConversationId::new(id).unwrap()));
}

#[test]
fn two_new_conversations_get_different_ids() {
    let qwen = common::qwen();
    let first = qwen.launch(&request(&Conversation::New)).unwrap();
    let second = qwen.launch(&request(&Conversation::New)).unwrap();

    assert_ne!(first.conversation, second.conversation);
}

/// Qwen Code refuses `--session-id` together with `--resume`.
#[test]
fn a_resumed_conversation_keeps_its_id_without_a_session_id() {
    let id = ConversationId::new(common::FIXTURE_SESSION).unwrap();
    let conversation = Conversation::Resume(id.clone());
    let command = common::qwen().launch(&request(&conversation)).unwrap();
    let words = words(&command.arguments);

    assert_eq!(value_after(&words, "--resume"), Some(id.as_str()));
    assert!(!words.contains(&"--session-id".to_owned()));
    assert_eq!(command.conversation, Some(id));
}

#[test]
fn a_chosen_model_is_passed_with_its_flag() {
    let conversation = Conversation::New;
    let command = common::qwen()
        .launch(&LaunchRequest {
            model: Some("qwen3-coder-plus"),
            ..request(&conversation)
        })
        .unwrap();

    assert_eq!(
        value_after(&words(&command.arguments), "--model"),
        Some("qwen3-coder-plus")
    );
}

#[test]
fn a_model_that_could_read_as_a_flag_is_refused() {
    let conversation = Conversation::New;
    for model in ["--yolo", "qwen coder", "", "qwen\n"] {
        let refused = common::qwen().launch(&LaunchRequest {
            model: Some(model),
            ..request(&conversation)
        });

        assert!(refused.is_err(), "{model:?} was accepted");
    }
}

/// `qwen <query>` is one-shot; `--prompt-interactive` runs the message and stays interactive.
#[test]
fn the_first_message_is_one_word_with_the_interactive_prompt_flag() {
    let conversation = Conversation::New;
    let command = common::qwen()
        .launch(&LaunchRequest {
            first_message: Some("-v explain this flag"),
            ..request(&conversation)
        })
        .unwrap();
    let words = words(&command.arguments);

    assert_eq!(
        words.last().map(String::as_str),
        Some("--prompt-interactive=-v explain this flag")
    );
    assert!(!words.contains(&"-v explain this flag".to_owned()));
}

#[test]
fn without_a_first_message_no_prompt_is_passed() {
    let command = common::qwen().launch(&request(&Conversation::New)).unwrap();

    assert!(
        !words(&command.arguments)
            .iter()
            .any(|word| word.starts_with("--prompt"))
    );
}

#[test]
fn the_launch_points_qwen_at_the_bridge_defaults_file() {
    let command = common::qwen().launch(&request(&Conversation::New)).unwrap();

    assert_eq!(
        command.environment,
        vec![(
            OsString::from("QWEN_CODE_SYSTEM_DEFAULTS_PATH"),
            OsString::from("/var/lib/torlek/qwen/system-defaults.json"),
        )]
    );
}

#[test]
fn without_its_bridge_a_launch_sets_no_environment() {
    let qwen = QwenCode::new(PathBuf::from("/usr/bin/qwen"), Some("0.28.4".into()), None);
    let command = qwen.launch(&request(&Conversation::New)).unwrap();

    assert!(command.environment.is_empty());
}

/// The daemon's trial build passes `/` for every path (#701): a launch only builds words.
#[test]
fn a_launch_writes_nothing_in_the_session_folder() {
    let session_dir = tempfile::tempdir().unwrap();
    let conversation = Conversation::New;
    common::qwen()
        .launch(&LaunchRequest {
            session_dir: session_dir.path(),
            ..request(&conversation)
        })
        .unwrap();

    assert_eq!(std::fs::read_dir(session_dir.path()).unwrap().count(), 0);
}

#[test]
fn a_launch_asking_for_what_qwen_lacks_is_refused() {
    let conversation = Conversation::New;
    let base = request(&conversation);
    for asked in [
        LaunchRequest {
            effort: Some("high"),
            ..base.clone()
        },
        LaunchRequest {
            remote_control: true,
            ..base.clone()
        },
        LaunchRequest {
            compaction_threshold: Some("70"),
            ..base.clone()
        },
    ] {
        assert!(common::qwen().launch(&asked).is_err(), "{asked:?}");
    }
}
