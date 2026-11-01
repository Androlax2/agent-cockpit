//! The keys a running OpenCode TUI receives for each action, decided by its activity.

mod common;

use cockpit_provider_api::{Action, Activity, Key, Provider, TerminalInput};

fn typed(text: &str) -> Vec<TerminalInput> {
    vec![
        TerminalInput::Text(text.to_owned()),
        TerminalInput::Key(Key::Enter),
    ]
}

#[test]
fn a_message_is_typed_then_entered_while_it_waits_or_works() {
    for activity in [Activity::Waiting, Activity::Working] {
        assert_eq!(
            common::opencode()
                .input(&Action::Submit("run the tests"), Some(activity))
                .unwrap(),
            typed("run the tests"),
            "{activity:?}"
        );
    }
}

#[test]
fn a_message_is_refused_while_a_dialog_is_open_or_the_activity_is_unknown() {
    for activity in [Some(Activity::NeedsYou), None] {
        assert!(
            common::opencode()
                .input(&Action::Submit("yes"), activity)
                .is_err(),
            "{activity:?}"
        );
    }
}

#[test]
fn a_message_the_prompt_would_not_submit_as_written_is_refused() {
    for message in [
        "/compact",
        "!rm -rf target",
        "look at @src/main.rs",
        "exit",
        " quit ",
        ":q",
        "",
        "two\nlines",
    ] {
        assert!(
            common::opencode()
                .input(&Action::Submit(message), Some(Activity::Waiting))
                .is_err(),
            "{message:?} was typed"
        );
    }
}

#[test]
fn cancel_presses_escape_twice_during_a_turn() {
    assert_eq!(
        common::opencode()
            .input(&Action::Cancel, Some(Activity::Working))
            .unwrap(),
        [
            TerminalInput::Key(Key::Escape),
            TerminalInput::Key(Key::Escape)
        ]
    );
}

#[test]
fn cancel_is_refused_outside_a_turn() {
    for activity in [Some(Activity::Waiting), Some(Activity::NeedsYou), None] {
        assert!(
            common::opencode().input(&Action::Cancel, activity).is_err(),
            "{activity:?}"
        );
    }
}

#[test]
fn compact_is_refused_naming_its_capability() {
    let error = common::opencode()
        .input(&Action::Compact, Some(Activity::Waiting))
        .unwrap_err();

    assert_eq!(error.reason, "it does not report the `compact` capability");
}
