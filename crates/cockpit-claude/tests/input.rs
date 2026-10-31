//! The keystrokes each terminal action becomes, and the activities that refuse it.

mod common;

use cockpit_provider_api::{Action, Activity, Key, Provider, TerminalInput};

fn text(text: &str) -> TerminalInput {
    TerminalInput::Text(text.to_owned())
}

#[test]
fn a_message_at_the_prompt_is_typed_then_entered() {
    let input = common::claude()
        .input(&Action::Submit("run the tests"), Some(Activity::Waiting))
        .unwrap();

    assert_eq!(
        input,
        [text("run the tests"), TerminalInput::Key(Key::Enter)]
    );
}

#[test]
fn a_message_during_a_turn_is_queued_with_ctrl_x_enter() {
    let input = common::claude()
        .input(&Action::Submit("also lint"), Some(Activity::Working))
        .unwrap();

    assert_eq!(
        input,
        [
            text("also lint"),
            TerminalInput::Key(Key::Control('x')),
            TerminalInput::Key(Key::Enter)
        ]
    );
}

#[test]
fn a_message_is_refused_while_a_dialog_is_open() {
    let error = common::claude()
        .input(&Action::Submit("yes"), Some(Activity::NeedsYou))
        .unwrap_err();

    assert_eq!(
        error.reason,
        "a dialog is open in its terminal; answer it first"
    );
}

#[test]
fn a_message_is_refused_when_the_activity_is_unknown() {
    assert!(
        common::claude()
            .input(&Action::Submit("hello"), None)
            .is_err()
    );
}

#[test]
fn an_empty_or_multi_line_message_is_refused() {
    for message in ["", "   ", "first\nsecond", "tab\there"] {
        assert!(
            common::claude()
                .input(&Action::Submit(message), Some(Activity::Waiting))
                .is_err(),
            "{message:?}"
        );
    }
}

#[test]
fn cancel_presses_escape_whatever_the_activity() {
    for activity in [None, Some(Activity::Working), Some(Activity::NeedsYou)] {
        assert_eq!(
            common::claude().input(&Action::Cancel, activity).unwrap(),
            [TerminalInput::Key(Key::Escape)]
        );
    }
}

#[test]
fn compact_runs_the_slash_command_at_the_prompt() {
    assert_eq!(
        common::claude()
            .input(&Action::Compact, Some(Activity::Waiting))
            .unwrap(),
        [text("/compact"), TerminalInput::Key(Key::Enter)]
    );
}

#[test]
fn compact_is_refused_during_a_turn() {
    assert!(
        common::claude()
            .input(&Action::Compact, Some(Activity::Working))
            .is_err()
    );
}
