//! M1 guide, sections 1 and 2: `cockpit tui` attaches with the list ready for the first key.

use crate::harness::{Cockpit, ManagerPane};

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
