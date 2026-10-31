//! The screen-reading fallback: Claude Code's terminal tail read as an activity with its
//! confidence. Never the source of a task's completion.

mod common;

use cockpit_provider_api::{Activity, Confidence, Observation, ObservedBy, Provider};

fn read(screen: &str) -> Option<Observation> {
    common::claude().read_screen(screen)
}

fn seen(activity: Activity, confidence: Confidence) -> Option<Observation> {
    Some(Observation {
        activity,
        observed_by: ObservedBy::Screen(confidence),
    })
}

const PERMISSION_DIALOG: &str = "╭──────────────────────────────╮
│ Bash command                 │
│   cargo nextest run          │
│ Do you want to proceed?      │
│ ❯ 1. Yes                     │
│   2. No, and tell Claude     │
╰──────────────────────────────╯";

#[test]
fn a_permission_dialog_needs_you() {
    assert_eq!(
        read(PERMISSION_DIALOG),
        seen(Activity::NeedsYou, Confidence::High)
    );
}

#[test]
fn a_choice_menu_with_its_cursor_needs_you() {
    let screen = " Which branch?\n ❯ 1. main\n   2. greenfield/33\n";
    assert_eq!(read(screen), seen(Activity::NeedsYou, Confidence::Low));
}

#[test]
fn a_running_spinner_is_working() {
    let screen = "✻ Compiling… (12s · ↓ 1.2k tokens)\n\n❯\u{a0}\n";
    assert_eq!(read(screen), seen(Activity::Working, Confidence::High));
}

#[test]
fn the_interrupt_hint_is_working() {
    let screen = "some output\n  esc to interrupt · ctrl+t to show todos\n";
    assert_eq!(read(screen), seen(Activity::Working, Confidence::High));
}

#[test]
fn the_bare_prompt_is_waiting_with_low_confidence() {
    let screen = "● Done.\n\n────────\n❯\u{a0}\n────────\n";
    assert_eq!(read(screen), seen(Activity::Waiting, Confidence::Low));
}

#[test]
fn a_screen_without_any_pattern_reads_nothing() {
    assert_eq!(read("$ ls\nCargo.toml\n"), None);
    assert_eq!(read(""), None);
}

#[test]
fn a_symbol_glued_to_the_spinner_verb_is_working() {
    assert_eq!(
        read("✻✻Thinking… (3s)\n"),
        seen(Activity::Working, Confidence::High)
    );
}

#[test]
fn a_word_before_an_ellipsis_verb_is_not_a_spinner() {
    assert_eq!(read("Note: Running… (2m ago)\n"), None);
}

#[test]
fn a_prompt_sign_without_its_padding_is_not_the_prompt() {
    assert_eq!(read("❯\n"), None);
    assert_eq!(read("│ ❯ │\n"), None);
}
