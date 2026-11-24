//! The keys and text each terminal action becomes, decided from the agent's activity.

mod common;

use cockpit_provider_api::{Action, Activity, Key, Provider, TerminalInput};

fn typed(text: &str, key: Key) -> Vec<TerminalInput> {
    vec![
        TerminalInput::Text(text.to_owned()),
        TerminalInput::Key(key),
    ]
}

#[test]
fn a_message_is_typed_then_entered_while_qwen_waits() {
    let keys = common::qwen()
        .input(&Action::Submit("run the tests"), Some(Activity::Waiting))
        .unwrap();

    assert_eq!(keys, typed("run the tests", Key::Enter));
}

/// Enter during a turn steers it; Ctrl-Q queues the message for the next turn.
#[test]
fn a_message_during_a_turn_is_queued_with_control_q() {
    let keys = common::qwen()
        .input(&Action::Submit("then commit"), Some(Activity::Working))
        .unwrap();

    assert_eq!(keys, typed("then commit", Key::Control('q')));
}

#[test]
fn a_message_is_refused_while_a_dialog_is_open_or_the_activity_is_unknown() {
    for activity in [Some(Activity::NeedsYou), None] {
        let refused = common::qwen().input(&Action::Submit("yes"), activity);

        assert!(refused.is_err(), "{activity:?}");
    }
}

#[test]
fn a_message_that_is_not_one_line_of_text_is_refused() {
    for message in ["", "   ", "first\nsecond", "bell\u{7}"] {
        let refused = common::qwen().input(&Action::Submit(message), Some(Activity::Waiting));

        assert!(refused.is_err(), "{message:?}");
    }
}

#[test]
fn cancel_is_escape() {
    let keys = common::qwen()
        .input(&Action::Cancel, Some(Activity::Working))
        .unwrap();

    assert_eq!(keys, vec![TerminalInput::Key(Key::Escape)]);
}

#[test]
fn compact_types_compress_while_qwen_waits() {
    let keys = common::qwen()
        .input(&Action::Compact, Some(Activity::Waiting))
        .unwrap();

    assert_eq!(keys, typed("/compress", Key::Enter));
}

#[test]
fn compact_is_refused_unless_qwen_waits() {
    for activity in [Some(Activity::Working), Some(Activity::NeedsYou), None] {
        let refused = common::qwen().input(&Action::Compact, activity);

        assert!(refused.is_err(), "{activity:?}");
    }
}
