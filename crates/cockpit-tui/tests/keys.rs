//! The basic TUI's list keys (decision 17): open, next needing you, restart and exit with a
//! confirmation, help and the log view. Each key counts as one named command.

mod common;

use std::time::Instant;

use cockpit_protocol::{AgentState, Command, Reply, SessionReport};
use cockpit_tui::{HELP, Key, Overlay};
use common::{answer, app_with, press, screen_text, sent, session};

fn three_sessions(now: Instant) -> cockpit_tui::App {
    app_with(
        &["app"],
        vec![
            session("s00000000000a", 1, 1),
            session("s00000000000b", 1, 2),
            session("s00000000000c", 1, 3),
        ],
        now,
    )
}

#[test]
fn enter_opens_the_selected_session_in_the_viewer() {
    let now = Instant::now();
    let mut app = three_sessions(now);

    let steps = press(&mut app, &[Key::Char('j'), Key::Enter], now);

    assert_eq!(
        sent(&steps),
        [Command::SessionShow {
            session: "s00000000000b".to_owned()
        }]
    );
}

#[test]
fn n_opens_the_next_session_that_needs_you_wrapping_around() {
    let now = Instant::now();
    let mut app = three_sessions(now);
    answer(
        &mut app,
        Command::AgentReports,
        Reply::AgentReports {
            reports: vec![SessionReport {
                session: "s00000000000a".to_owned(),
                state: Some(AgentState::Waiting),
                label: None,
                doing: None,
                needs_input: Some("Which branch?".to_owned()),
                outcome: None,
                question_source: Some(cockpit_protocol::QuestionSource::Agent),
                reported_at: 1,
            }],
        },
        now,
    );
    press(&mut app, &[Key::Char('j')], now);

    let step = app.key(Key::Char('n'), now);

    assert_eq!(
        step.send,
        [Command::SessionShow {
            session: "s00000000000a".to_owned()
        }]
    );
    assert_eq!(app.selected(), Some("s00000000000a"));
}

#[test]
fn restart_and_exit_are_sent_only_after_y() {
    let now = Instant::now();
    let mut app = three_sessions(now);

    let cancelled = press(&mut app, &[Key::Char('r'), Key::Char('n')], now);
    let asked = app.key(Key::Char('x'), now);
    let shown = screen_text(&app, now);
    let confirmed = app.key(Key::Char('y'), now);

    assert_eq!(sent(&cancelled), Vec::<Command>::new());
    assert_eq!(asked.used, Some("exit-ask"));
    assert!(
        shown.contains("exit s00000000000a? y to confirm"),
        "{shown}"
    );
    assert_eq!(
        confirmed.send,
        [Command::SessionEnd {
            session: "s00000000000a".to_owned()
        }]
    );
}

#[test]
fn help_lists_every_key_and_the_log_shows_what_the_tui_did() {
    let now = Instant::now();
    let mut app = three_sessions(now);
    let open = app.key(Key::Enter, now).send.remove(0);
    app.answer(
        &open,
        Err("not-found: no manager session is open".to_owned()),
        now,
    );

    app.key(Key::Char('?'), now);
    let help = screen_text(&app, now);
    app.key(Key::Char('L'), now);
    let log = screen_text(&app, now);
    app.key(Key::Esc, now);

    assert!(help.contains("next session that needs you"), "{help}");
    assert!(HELP.contains("Detach from the manager"));
    assert!(
        log.contains("session-show: not-found: no manager session is open"),
        "{log}"
    );
    assert_eq!(app.overlay(), None::<Overlay>);
}

/// Decision 17: every key the TUI receives counts as a named command, the unbound ones too.
#[test]
fn every_key_counts_as_a_named_command() {
    let now = Instant::now();
    let mut app = three_sessions(now);
    let keys = [
        Key::Char('j'),
        Key::Char('k'),
        Key::Enter,
        Key::Char('n'),
        Key::Char('r'),
        Key::Esc,
        Key::Char('Z'),
        Key::Char('?'),
        Key::Esc,
        Key::Char('L'),
        Key::Esc,
        Key::Char('N'),
        Key::Char('a'),
        Key::Esc,
        Key::Tab,
    ];

    let used: Vec<Option<&str>> = keys.iter().map(|key| app.key(*key, now).used).collect();

    assert_eq!(
        used,
        [
            Some("move"),
            Some("move"),
            Some("open"),
            Some("next-needing-you"),
            Some("restart-ask"),
            Some("confirm-cancel"),
            Some("unbound"),
            Some("help"),
            Some("close-overlay"),
            Some("log-view"),
            Some("close-overlay"),
            Some("new-session-form"),
            Some("form-edit"),
            Some("new-session-cancel"),
            Some("focus-list"),
        ]
    );
}

#[test]
fn a_daemon_outage_is_logged_once_and_its_status_clears_when_the_daemon_answers() {
    let now = Instant::now();
    let mut app = three_sessions(now);
    let problem = "cannot connect: connection refused".to_owned();

    for _ in 0..3 {
        app.feed_lost(problem.clone());
        for command in cockpit_tui::App::refresh_commands() {
            app.answer(&command, Err(problem.clone()), now);
        }
    }
    let outage = app.log().filter(|entry| entry.contains("refused")).count();
    for command in cockpit_tui::App::refresh_commands() {
        let reply = match command {
            Command::ProjectList => Reply::Projects {
                projects: Vec::new(),
            },
            Command::AgentReports => Reply::AgentReports {
                reports: Vec::new(),
            },
            Command::ApprovalQueue => Reply::ApprovalQueue {
                runs: Vec::new(),
                guides: Vec::new(),
            },
            other => panic!("not a refresh command: {other:?}"),
        };
        app.answer(&command, Ok(reply), now);
    }
    app.apply_changes(&common::snapshot(Vec::new())).unwrap();

    assert_eq!(outage, 1);
    assert!(
        !screen_text(&app, now).contains("connection refused"),
        "{}",
        screen_text(&app, now)
    );
}
