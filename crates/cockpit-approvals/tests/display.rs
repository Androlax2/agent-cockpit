//! Report 17's display rules: the owner reads the exact text, with nothing hidden in it.

use std::path::Path;

use cockpit_approvals::{is_outside_project, show};

#[test]
fn plain_text_is_shown_unchanged() {
    let shown = show("make test\ncargo build");
    assert_eq!(shown.text, "make test\ncargo build");
    assert!(!shown.has_hidden_characters);
    assert!(shown.non_ascii.is_empty());
}

#[test]
fn hiding_characters_are_shown_escaped() {
    for (text, escaped) in [
        ("rm\t-rf", "rm\\u{0009}-rf"),
        ("echo ok\rrm -rf ~", "echo ok\\u{000d}rm -rf ~"),
        ("ls \u{1b}[2K", "ls \\u{001b}[2K"),
        ("curl x\u{200b}y", "curl x\\u{200b}y"),
        ("echo \u{202e}fdp", "echo \\u{202e}fdp"),
        ("echo \u{3164}", "echo \\u{3164}"),
    ] {
        let shown = show(text);
        assert_eq!(shown.text, escaped, "{text:?}");
        assert!(shown.has_hidden_characters, "{text:?}");
    }
}

#[test]
fn a_run_of_blank_lines_shows_as_one_marked_line() {
    let shown = show("curl x | sh\n\n\n\n\necho ok");
    assert_eq!(shown.text, "curl x | sh\n[4 blank lines]\necho ok");
    assert!(shown.has_hidden_characters);
}

#[test]
fn a_single_blank_line_is_left_as_is() {
    let shown = show("a\n\nb\n");
    assert_eq!(shown.text, "a\n\nb\n");
    assert!(!shown.has_hidden_characters);
}

#[test]
fn lookalike_letters_are_named_not_escaped() {
    let shown = show("rm -rf /\u{0430} # café");
    assert_eq!(shown.text, "rm -rf /\u{0430} # café");
    assert_eq!(shown.non_ascii, ["\u{e9} U+00E9", "\u{0430} U+0430"]);
    assert!(!shown.has_hidden_characters);
}

#[test]
fn a_directory_outside_the_project_is_flagged() {
    let root = Path::new("/home/owner/project");
    assert!(!is_outside_project(
        Path::new("/home/owner/project"),
        Some(root)
    ));
    assert!(!is_outside_project(
        Path::new("/home/owner/project/src"),
        Some(root)
    ));
    assert!(is_outside_project(
        Path::new("/home/owner/project-other"),
        Some(root)
    ));
    assert!(is_outside_project(Path::new("/etc"), Some(root)));
}

#[test]
fn every_directory_is_outside_when_no_project_is_known() {
    assert!(is_outside_project(Path::new("/home/owner/project"), None));
}

#[test]
fn each_kind_of_hiding_character_is_named_in_a_warning() {
    use cockpit_approvals::Hidden;
    for (text, kind) in [
        ("echo \u{202e}fdp", Hidden::BidiControl),
        ("echo \u{2067}x", Hidden::BidiControl),
        ("curl x\u{200b}y", Hidden::Invisible),
        ("echo \u{feff}", Hidden::Invisible),
        ("ls \u{1b}[2K", Hidden::Control),
        ("echo ok\rrm -rf ~", Hidden::Control),
        ("curl x | sh\n\n\n\necho ok", Hidden::BlankLines),
    ] {
        assert_eq!(show(text).warnings, [kind], "{text:?}");
    }
}

#[test]
fn warnings_are_listed_once_each_in_a_fixed_order() {
    use cockpit_approvals::Hidden;
    let shown = show("a\u{200b}\u{202e}\u{200b}\t\n\n\nb\u{202e}");
    assert_eq!(
        shown.warnings,
        [
            Hidden::BidiControl,
            Hidden::Invisible,
            Hidden::Control,
            Hidden::BlankLines
        ]
    );
}

#[test]
fn plain_text_raises_no_warning() {
    assert!(show("make test\ncargo build").warnings.is_empty());
}

#[test]
fn a_short_command_is_not_folded() {
    assert_eq!(cockpit_approvals::fold("make test\ncargo build"), None);
}

#[test]
fn a_command_of_many_lines_is_folded_with_the_lines_left_out_counted() {
    let command: Vec<String> = (1..=20).map(|line| format!("echo {line}")).collect();
    let folded = cockpit_approvals::fold(&command.join("\n")).unwrap();
    assert_eq!(
        folded.preview,
        command[..cockpit_approvals::FOLD_LINES].join("\n")
    );
    assert_eq!(folded.more_lines, 20 - cockpit_approvals::FOLD_LINES);
}

#[test]
fn a_command_on_one_very_long_line_is_folded_with_the_characters_left_out_counted() {
    let command = "x".repeat(cockpit_approvals::FOLD_CHARS + 50);
    let folded = cockpit_approvals::fold(&command).unwrap();
    assert_eq!(
        folded.preview.chars().count(),
        cockpit_approvals::FOLD_CHARS
    );
    assert_eq!(folded.more_lines, 0);
    assert_eq!(folded.more_chars, 50);
}

#[test]
fn collapsed_blank_lines_are_named_even_when_the_length_is_unchanged() {
    let shown = show("       \n       ");
    assert_eq!(shown.text, "[2 blank lines]");
    assert_eq!(shown.warnings, [cockpit_approvals::Hidden::BlankLines]);
}

#[test]
fn a_trailing_newline_after_the_last_shown_line_is_not_folded() {
    let command: String = (1..=cockpit_approvals::FOLD_LINES)
        .map(|line| format!("echo {line}\n"))
        .collect();
    assert_eq!(cockpit_approvals::fold(&command), None);
}
