//! A session card names the coordinator grants its session holds and the latest message a
//! coordinator queued to it, with the sender (#410).

mod common;

use std::time::Instant;

use cockpit_protocol::InboundMessageView;
use common::{app_with, screen_text, session};

#[test]
fn a_card_shows_its_coordinator_grants_and_the_message_queued_to_it() {
    let now = Instant::now();
    let mut coordinator = session("s00000000000a", 1, 1);
    coordinator.coordinator_grants =
        vec!["observe-sessions".to_owned(), "message-sessions".to_owned()];
    let mut worker = session("s00000000000b", 1, 2);
    worker.inbound_message = Some(InboundMessageView {
        from: "s00000000000a".to_owned(),
        text: "check PR 12".to_owned(),
        delivery: "received".to_owned(),
        sent_at: 3,
    });
    let app = app_with(&["app"], vec![coordinator, worker], now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("grants observe,message"), "{screen}");
    assert!(
        screen.contains("✉ s00000000000a (received): \"check PR 12\""),
        "{screen}"
    );
}

#[test]
fn a_card_without_grants_or_messages_has_no_coordination_line() {
    let now = Instant::now();
    let app = app_with(&["app"], vec![session("s00000000000a", 1, 1)], now);

    let screen = screen_text(&app, now);

    assert!(!screen.contains("grants "), "{screen}");
    assert!(!screen.contains('✉'), "{screen}");
}
