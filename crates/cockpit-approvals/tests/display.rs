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
    let shown = show("rm -rf /\u{0430} # caf\u{e9}");
    assert_eq!(shown.text, "rm -rf /\u{0430} # caf\u{e9}");
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
