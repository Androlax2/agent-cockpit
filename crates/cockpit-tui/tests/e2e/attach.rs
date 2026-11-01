//! M1 guide, sections 1 and 2: `cockpit tui` attaches with the list ready for the first key.

use crate::harness::{COLUMNS, Cockpit, ManagerPane};

/// The help overlay's title, as its border shows it.
const HELP_TITLE: &str = "┌Help";

#[test]
fn attach_shows_the_project_and_the_empty_viewer() {
    let cockpit = Cockpit::start();
    cockpit.snapshot("attach");
}

#[test]
fn the_list_has_the_keys_on_attach() {
    let cockpit = Cockpit::start();
    assert_eq!(cockpit.focused(), ManagerPane::List);
    cockpit.press(&["N"]);
    cockpit.wait_for("New session");
}

/// Attaching resizes the TUI, often as the owner's first key arrives; such a key used to wait
/// for the next one (#356). Each round resizes and presses at once; a stuck key fails it.
/// Every round narrows the terminal: a width the TUI drew before could leave it a stale
/// screen, which is not what this test is about.
#[test]
fn a_key_pressed_while_the_terminal_resizes_reaches_the_list() {
    let cockpit = Cockpit::start();
    for narrower in 1..=20 {
        cockpit.resize_and_press(COLUMNS - narrower, "?");
        cockpit.wait_for(HELP_TITLE);
        cockpit.press(&["Escape"]);
        cockpit.wait_for_gone(HELP_TITLE);
    }
}
