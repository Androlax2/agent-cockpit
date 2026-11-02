//! #44 through a real terminal and daemon in an isolated fixture.
use crate::harness::Cockpit;

#[test]
fn tasks_add_edit_queue_and_explain_through_the_terminal() {
    let cockpit = Cockpit::start();
    cockpit.press(&["T"]);
    cockpit.wait_for("No open tasks");
    cockpit.press(&["a"]);
    cockpit.wait_for("Add task");
    cockpit.type_text("Terminal task");
    cockpit.press(&["Tab"]);
    cockpit.wait_for("> Notes:");
    cockpit.type_text("Keep this note");
    cockpit.press(&["Enter"]);
    cockpit.wait_for("Terminal task");
    cockpit.wait_for_gone("Add task");
    let listed = cockpit.cockpit(&["todo", "list"]);
    assert!(listed.contains("Terminal task"), "{listed}");
    let id = listed.split_whitespace().next().unwrap();
    cockpit.press(&["e"]);
    cockpit.wait_for("Edit task");
    cockpit.type_text(" edited");
    cockpit.press(&["Enter"]);
    cockpit.wait_for_gone("Edit task");
    let shown = cockpit.cockpit(&["todo", "show", id]);
    assert!(
        shown.contains("Terminal task edited") && shown.contains("Keep this note"),
        "{shown}"
    );
    cockpit.press(&["Q"]);
    cockpit.wait_for("[Q]");
    cockpit.cockpit(&["queue", "dispatch"]);
    cockpit.press(&["?"]);
    cockpit.wait_for("Queue explanation");
    cockpit.wait_for("no session works");
    cockpit.press(&["Escape"]);
    cockpit.wait_for("[Q]");
    cockpit.press(&["Q"]);
    cockpit.wait_for("[ ]");
}
