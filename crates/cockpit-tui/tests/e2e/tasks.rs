//! #44 through a real terminal and daemon in an isolated fixture.
use crate::harness::Cockpit;

/// Waits for the list view with the task's row: its help line and the `[ ]` of a row, neither
/// of which the form draws. The daemon has answered by then. Waiting for the form to vanish
/// instead matches a capture taken mid-redraw, with the pane blank.
fn wait_for_task_row(cockpit: &Cockpit) {
    cockpit.wait_until("the task list shows the row", |screen| {
        screen.contains("a add; e edit") && screen.contains("[ ] Terminal task")
    });
}

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
    wait_for_task_row(&cockpit);
    let listed = cockpit.cockpit(&["todo", "list"]);
    assert!(listed.contains("Terminal task"), "{listed}");
    let id = listed.split_whitespace().next().unwrap();
    cockpit.press(&["e"]);
    cockpit.wait_for("Edit task");
    // Rows 0 to 2 are the form's: a capture of the form being drawn over the list can still
    // show the list's help line and row.
    cockpit.wait_for("> Title: Terminal task");
    cockpit.type_text(" edited");
    cockpit.press(&["Enter"]);
    wait_for_task_row(&cockpit);
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
