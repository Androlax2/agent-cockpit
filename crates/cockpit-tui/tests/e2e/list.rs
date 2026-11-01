//! The session list on a terminal too short for every card (#291): the selected card stays on
//! screen, and a card's wrapped details line stays under its indent.

use crate::harness::Cockpit;

/// A label and a doing that together are longer than the list is wide, so the card's
/// details line wraps.
const LABEL_AND_DOING: &str = r#"!cockpit agent report --label "plan review" --doing "reading every report before it changes the card layout""#;

/// Rows the owner's terminal keeps: the list's border and the status line leave five rows of
/// cards, fewer than the project's name and five one-row cards need.
const SHORT_ROWS: u16 = 8;

#[test]
fn the_list_scrolls_to_keep_the_selected_card_on_screen() {
    let cockpit = Cockpit::start();
    let sessions: Vec<String> = (0..5).map(|_| cockpit.session_from_cli()).collect();
    cockpit.wait_until("every session listed", |screen| {
        sessions
            .iter()
            .all(|session| screen.contains(session.as_str()))
    });
    cockpit.resize_rows(SHORT_ROWS);

    // To the last card, whichever it is: sessions started in the same second list by id.
    cockpit.press(&["j", "j", "j", "j"]);

    cockpit.wait_until("the selected last card on screen", |screen| {
        let selected = screen.lines().find(|line| line.starts_with("│>  s"));
        selected.is_some_and(|line| {
            sessions
                .iter()
                .any(|session| line.contains(session.as_str()))
        }) && !screen.contains("│demo")
    });
}

#[test]
fn a_wrapped_details_line_stays_under_the_card_s_indent() {
    let cockpit = Cockpit::start();
    let session = cockpit.session_from_cli();
    cockpit.wait_for(&session);

    cockpit.agent_does(&session, LABEL_AND_DOING);

    let screen = cockpit.wait_for("layout");
    let details: Vec<&str> = screen
        .lines()
        .skip_while(|line| !line.contains("plan review"))
        .take_while(|line| !line.starts_with('└'))
        .collect();
    assert!(details.len() > 1, "the details wrap:\n{screen}");
    for line in details {
        assert!(
            line.starts_with("│   "),
            "{line:?} is under the indent:\n{screen}"
        );
    }
}
