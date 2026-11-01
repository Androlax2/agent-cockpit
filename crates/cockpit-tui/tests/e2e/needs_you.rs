//! M1 guide, section 5: an agent's label, doing and question reach its card, `n` opens the
//! session that needs the owner, and the owner's answer clears the mark.

use crate::harness::{Cockpit, FIRST_MESSAGE};

const LABEL_REPORT: &str = r#"!cockpit agent report --label "acceptance" --doing "checking cards""#;
const QUESTION_REPORT: &str = r#"!cockpit agent report --needs-input "Pick A or B?""#;

fn card_shows_needs_you(screen: &str, session: &str) -> bool {
    screen.contains(&format!("! {session}")) && screen.contains("waiting on you")
}

#[test]
fn the_card_shows_the_needs_you_mark_label_and_doing() {
    let cockpit = Cockpit::start();
    let session = cockpit.session_from_cli();
    cockpit.wait_for(&session);
    cockpit.agent_does(&session, LABEL_REPORT);
    cockpit.agent_does(&session, QUESTION_REPORT);
    cockpit.wait_until("the card's mark, label and doing", |screen| {
        card_shows_needs_you(screen, &session)
            && screen.contains("acceptance")
            && screen.contains("checking cards")
    });
}

#[test]
fn n_opens_the_session_that_needs_you() {
    let cockpit = Cockpit::start();
    let quiet = cockpit.session_from_cli();
    cockpit.wait_for(&quiet);
    let asking = cockpit.session_from_cli();
    cockpit.wait_for(&asking);
    cockpit.agent_does(&asking, QUESTION_REPORT);
    cockpit.wait_until("the asking card's mark", |screen| {
        card_shows_needs_you(screen, &asking)
    });
    assert!(
        cockpit.screen().contains(&format!(">  {quiet}")),
        "the first card is selected"
    );
    cockpit.press(&["n"]);
    cockpit.wait_for(&format!(">! {asking}"));
    cockpit.wait_for("Pick A or B?");
}

#[test]
fn answering_the_agent_clears_the_mark() {
    let cockpit = Cockpit::start();
    let session = cockpit.session_from_cli();
    cockpit.wait_for(&session);
    cockpit.press(&["Enter"]);
    cockpit.wait_for(&format!("reply: {FIRST_MESSAGE}"));
    cockpit.tell_agent(QUESTION_REPORT);
    cockpit.wait_until("the card's mark", |screen| {
        card_shows_needs_you(screen, &session)
    });
    cockpit.tell_agent("A");
    let screen = cockpit.wait_for_gone("waiting on you");
    assert!(!screen.contains(&format!("! {session}")), "{screen}");
}
