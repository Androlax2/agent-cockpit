//! A session card names the coordinator grants its session holds and the latest message a
//! coordinator queued to it, with the sender (#410).

mod common;

use std::time::Instant;

use cockpit_protocol::InboundMessageView;
use cockpit_tui::{App, Key};
use common::{app_with, press, screen, screen_text, session};

fn message(delivery: &str, text: &str) -> Option<InboundMessageView> {
    Some(InboundMessageView {
        from: "s00000000000a".to_owned(),
        text: text.to_owned(),
        delivery: delivery.to_owned(),
        sent_at: 3,
        note: None,
    })
}

/// The coordinator's card comes first and is selected; the worker's card is not.
fn coordinator_and_worker(worker_message: Option<InboundMessageView>, now: Instant) -> App {
    let mut coordinator = session("s00000000000a", 1, 1);
    coordinator.coordinator_grants =
        vec!["observe-sessions".to_owned(), "message-sessions".to_owned()];
    let mut worker = session("s00000000000b", 1, 2);
    worker.inbound_message = worker_message;
    app_with(&["app"], vec![coordinator, worker], now)
}

#[test]
fn a_card_shows_its_coordinator_grants() {
    let now = Instant::now();
    let app = coordinator_and_worker(None, now);

    let screen = screen_text(&app, now);

    assert!(screen.contains("grants observe,message"), "{screen}");
}

/// #728: once the agent has the message, nothing waits on the owner.
#[test]
fn an_unselected_card_hides_a_message_its_agent_received() {
    let now = Instant::now();
    let app = coordinator_and_worker(message("received", "check PR 12"), now);

    let screen = screen_text(&app, now);

    assert!(!screen.contains('✉'), "{screen}");
}

/// #728: the full text stays reachable on the selected card.
#[test]
fn the_selected_card_shows_the_whole_message_its_agent_received() {
    let now = Instant::now();
    let text = "check PR 12, then rebase the branch on main and rerun the focused tests";
    let mut app = coordinator_and_worker(message("received", text), now);

    press(&mut app, &[Key::Char('j')], now);

    let rows = screen(&app, now, 60);
    let unwrapped = rows
        .iter()
        .map(|row| row.trim_matches('│').trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        unwrapped.contains(&format!("✉ s00000000000a (received): \"{text}\"")),
        "{rows:#?}"
    );
}

/// #728: a message still waiting to be typed stays, on one row, cut where the row ends.
#[test]
fn an_unselected_card_shows_an_undelivered_message_on_one_truncated_row() {
    let now = Instant::now();
    let text = "check PR 12, then rebase the branch on main and rerun the focused tests";
    for delivery in ["queued", "held", "failed"] {
        let app = coordinator_and_worker(message(delivery, text), now);

        let rows = screen(&app, now, 60);

        let message_rows: Vec<&String> = rows.iter().filter(|row| row.contains('✉')).collect();
        assert_eq!(message_rows.len(), 1, "{delivery}: {rows:#?}");
        let message_row = message_rows[0];
        assert!(
            message_row.contains(&format!("✉ s00000000000a ({delivery}): \"check PR 12")),
            "{delivery}: {rows:#?}"
        );
        assert!(
            message_row.trim_end_matches('│').trim_end().ends_with('…'),
            "{delivery}: {rows:#?}"
        );
        assert!(
            !rows.join("\n").contains("focused tests"),
            "{delivery}: {rows:#?}"
        );
    }
}

/// #656: a message that cannot be typed yet says why on its target's card.
#[test]
fn a_card_says_why_the_message_queued_to_it_waits() {
    let now = Instant::now();
    let mut worker = session("s00000000000b", 1, 2);
    worker.inbound_message = Some(InboundMessageView {
        from: "s00000000000a".to_owned(),
        text: "hello".to_owned(),
        delivery: "queued".to_owned(),
        sent_at: 3,
        note: Some("its conversation is not named yet".to_owned()),
    });
    let app = app_with(&["app"], vec![worker], now);

    let screen = screen_text(&app, now);

    assert!(
        screen.contains("✉ s00000000000a (queued: its conversation is not named yet)"),
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
