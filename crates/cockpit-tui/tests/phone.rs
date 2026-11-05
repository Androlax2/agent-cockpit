//! The phone layout (#855), driven through the app with an injected clock and drawn on narrow
//! test terminals: the cards first with their command whole, one line per session, the same
//! decision as the desktop, and an open session that reads only until `i`.

mod common;

use std::time::{Duration, Instant};

use cockpit_protocol::{
    AgentState, Command, DecidedThroughView, ManagerPaneView, QuestionSource, Reply,
    RunRequestView, SessionReport,
};
use cockpit_tui::{App, HELP, INPUT_GRACE, Key, PHONE_TYPING_TIMEOUT};
use common::{
    answer, app_with, folded, press, queue, run, screen, sent, session, sized_screen, typing,
};

const SESSION: &str = "s00000000000a";
const OTHER: &str = "s00000000000b";
/// Longer than a phone's row, with a word wider than one.
const LONG_COMMAND: &str = "cargo nextest run --manifest-path Cargo.toml --test phone_layout \
    -- --nocapture && echo all-done-with-a-single-word-wider-than-a-phone-row-0123456789";

fn after_grace(now: Instant) -> Instant {
    now + INPUT_GRACE + Duration::from_millis(1)
}

fn fit(session_open: bool) -> Command {
    Command::ManagerFit { session_open }
}

/// The daemon's answer to `manager-fit`: in the phone layout or not.
fn fitted(app: &mut App, session_open: bool, phone: bool, now: Instant) -> Option<Command> {
    answer(app, fit(session_open), Reply::ManagerFitted { phone }, now)
}

fn reports(app: &mut App, reports: Vec<SessionReport>, now: Instant) {
    answer(
        app,
        Command::AgentReports,
        Reply::AgentReports { reports },
        now,
    );
}

fn report(session: &str, doing: &str, question: Option<&str>) -> SessionReport {
    SessionReport {
        session: session.to_owned(),
        state: Some(AgentState::Working),
        label: None,
        doing: Some(doing.to_owned()),
        needs_input: question.map(str::to_owned),
        outcome: None,
        question_source: question.map(|_| QuestionSource::Agent),
        covered_question: None,
        held_by: None,
        reported_at: 1,
    }
}

/// Two sessions of `app`, on a phone `width`×`height`.
fn phone_app(width: u16, height: u16, now: Instant) -> App {
    let mut app = app_with(
        &["app"],
        vec![session(SESSION, 1, 1), session(OTHER, 1, 2)],
        now,
    );
    app.set_screen(width, height);
    fitted(&mut app, false, true, now);
    app
}

/// `runs` waiting, the column's keys arrived at once.
fn asking(app: &mut App, runs: Vec<RunRequestView>, now: Instant) {
    if let Some(focus) = queue(app, runs, Vec::new(), now) {
        answer(
            app,
            focus,
            Reply::ManagerFocused {
                took_from_viewer: false,
            },
            now,
        );
    }
}

/// The selected session opened on the phone: returns what the TUI sent after the daemon showed
/// it.
fn opened(app: &mut App, now: Instant) -> Option<Command> {
    let shown = Command::SessionShow {
        session: SESSION.to_owned(),
    };
    assert_eq!(
        sent(&press(app, &[Key::Enter], now)),
        std::slice::from_ref(&shown)
    );
    answer(
        app,
        shown,
        Reply::Session {
            session: Box::new(session(SESSION, 1, 1)),
        },
        now,
    )
}

/// The screen's rows with every space gone: a wrapped text reads whole again.
fn squeezed(rows: &[String]) -> String {
    rows.concat().replace(' ', "")
}

#[test]
fn the_runtime_asks_for_the_layout_with_no_session_open() {
    assert_eq!(App::new().fit_command(), fit(false));
}

#[test]
fn at_40_and_60_columns_the_card_shows_the_whole_command_wrapped_never_folded() {
    for width in [40, 60] {
        let now = Instant::now();
        let mut app = phone_app(width, 40, now);
        let folded_run = RunRequestView {
            folded_command: folded("cargo nextest run", 1, 80),
            ..run(3, SESSION, LONG_COMMAND)
        };

        asking(&mut app, vec![folded_run], now);
        let rows = sized_screen(&app, now, width, 40);

        assert!(
            squeezed(&rows).contains(&LONG_COMMAND.replace(' ', "")),
            "{width} columns:\n{}",
            rows.join("\n")
        );
        assert!(
            !rows.iter().any(|row| row.contains("… more")),
            "{}",
            rows.join("\n")
        );
    }
}

#[test]
fn at_40_and_60_columns_the_list_has_one_line_per_session_with_project_state_and_doing() {
    for width in [40, 60] {
        let now = Instant::now();
        let mut app = phone_app(width, 20, now);
        reports(
            &mut app,
            vec![report(SESSION, "Reading the code", None)],
            now,
        );

        let rows = sized_screen(&app, now, width, 20);

        assert_eq!(rows[0], ">  app  running  Reading the code", "{rows:#?}");
        assert_eq!(rows[1], "   app  running", "{rows:#?}");
    }
}

#[test]
fn a_session_that_asks_is_marked_and_its_question_shown_first_on_a_card() {
    let now = Instant::now();
    let mut app = phone_app(40, 20, now);
    reports(
        &mut app,
        vec![report(OTHER, "Planning", Some("Merge the two branches?"))],
        now,
    );

    let rows = sized_screen(&app, now, 40, 20);

    assert!(
        rows[0].starts_with(&format!("? {OTHER} asks: Merge")),
        "{rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.starts_with(" ? app  running")),
        "{rows:#?}"
    );
}

#[test]
fn a_question_a_coordinator_holds_gets_no_card_and_no_mark() {
    let now = Instant::now();
    let mut app = phone_app(40, 20, now);
    let held = SessionReport {
        held_by: Some(SESSION.to_owned()),
        ..report(OTHER, "Planning", Some("Merge the two branches?"))
    };
    reports(&mut app, vec![held], now);

    let rows = sized_screen(&app, now, 40, 20);

    assert!(!rows.iter().any(|row| row.contains("Merge")), "{rows:#?}");
    assert!(
        rows.iter().any(|row| row == "   app  running  Planning"),
        "{rows:#?}"
    );
}

#[test]
fn the_key_hint_line_fits_40_columns_uncut() {
    let now = Instant::now();
    let mut app = phone_app(40, 20, now);
    asking(&mut app, vec![run(3, SESSION, "make test")], now);

    let rows = sized_screen(&app, after_grace(now), 40, 20);

    let hint = rows.last().unwrap();
    assert_eq!(hint, "y approve   x decline   m more");
    assert!(!hint.ends_with('…'));
}

#[test]
fn an_approval_through_the_phone_layout_sends_what_the_desktop_sends() {
    let now = Instant::now();
    let mut desktop = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    let mut phone = phone_app(40, 40, now);
    asking(&mut desktop, vec![run(3, SESSION, LONG_COMMAND)], now);
    asking(&mut phone, vec![run(3, SESSION, LONG_COMMAND)], now);

    let from_desktop = sent(&press(&mut desktop, &[Key::Char('y')], after_grace(now)));
    let from_phone = sent(&press(&mut phone, &[Key::Char('y')], after_grace(now)));

    assert_eq!(
        from_phone,
        [Command::ApprovalApprove {
            request: 3,
            revision: 1,
            digest: "digest-3-1".to_owned(),
            through: Some(DecidedThroughView::Tui),
        }]
    );
    assert_eq!(from_phone, from_desktop);
}

#[test]
fn a_decline_through_the_phone_layout_sends_what_the_desktop_sends() {
    let now = Instant::now();
    let mut desktop = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    let mut phone = phone_app(40, 40, now);
    asking(&mut desktop, vec![run(3, SESSION, "make test")], now);
    asking(&mut phone, vec![run(3, SESSION, "make test")], now);

    let from_desktop = sent(&press(&mut desktop, &[Key::Char('x')], after_grace(now)));
    let from_phone = sent(&press(&mut phone, &[Key::Char('x')], after_grace(now)));

    assert_eq!(from_phone, from_desktop);
    assert_eq!(from_phone.len(), 1);
}

#[test]
fn y_waits_until_the_end_of_a_card_taller_than_the_screen_is_on_screen() {
    let now = Instant::now();
    let mut app = phone_app(40, 8, now);
    asking(&mut app, vec![run(3, SESSION, LONG_COMMAND)], now);
    let later = after_grace(now);

    let unseen = sent(&press(&mut app, &[Key::Char('y'), Key::Char('x')], later));
    let hint = sized_screen(&app, later, 40, 8).last().unwrap().clone();
    let scrolled = sent(&press(&mut app, &[Key::Char('j'); 20], later));
    let approved = sent(&press(&mut app, &[Key::Char('y')], later));

    assert_eq!(unseen, []);
    assert!(hint.starts_with("j scrolls"), "{hint}");
    assert_eq!(scrolled, []);
    assert!(
        matches!(
            approved.as_slice(),
            [Command::ApprovalApprove { request: 3, .. }]
        ),
        "{approved:?}"
    );
}

#[test]
fn scrolling_shows_the_end_of_a_tall_card() {
    let now = Instant::now();
    let mut app = phone_app(40, 8, now);
    asking(&mut app, vec![run(3, SESSION, LONG_COMMAND)], now);

    press(&mut app, &[Key::Char('j'); 20], after_grace(now));
    let rows = sized_screen(&app, after_grace(now), 40, 8);

    assert!(
        squeezed(&rows).contains("0123456789"),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn enter_opens_the_session_and_asks_for_the_session_layout() {
    let now = Instant::now();
    let mut app = phone_app(40, 3, now);

    let next = opened(&mut app, now);
    let rows = sized_screen(&app, now, 40, 3);

    assert_eq!(next, Some(fit(true)));
    assert_eq!(rows[0], format!("{SESSION} · app · read-only"));
    assert_eq!(rows[1], "i type   Esc back   ? help");
}

#[test]
fn keys_in_the_read_only_session_reach_no_pane() {
    let now = Instant::now();
    let mut app = phone_app(40, 3, now);
    opened(&mut app, now);

    let mut steps = typing(&mut app, "make test now yxrn", now);
    steps.extend(press(
        &mut app,
        &[Key::Enter, Key::Tab, Key::Backspace, Key::Up],
        now,
    ));

    assert_eq!(sent(&steps), []);
}

#[test]
fn after_i_the_typed_line_is_sent_to_the_open_session_on_enter() {
    let now = Instant::now();
    let mut app = phone_app(40, 3, now);
    opened(&mut app, now);

    let mut steps = press(&mut app, &[Key::Char('i')], now);
    steps.extend(typing(&mut app, "yes, merge", now));
    let rows = sized_screen(&app, now, 40, 3);
    steps.extend(press(&mut app, &[Key::Enter], now));

    assert_eq!(rows[0], format!("{SESSION} · app · TYPING"));
    assert_eq!(rows[1], "> yes, merge_");
    assert_eq!(
        sent(&steps),
        [Command::SessionType {
            session: SESSION.to_owned(),
            text: "yes, merge".to_owned(),
        }]
    );
}

#[test]
fn typing_reads_only_again_after_a_minute_without_a_key() {
    let now = Instant::now();
    let mut app = phone_app(40, 3, now);
    opened(&mut app, now);
    press(&mut app, &[Key::Char('i')], now);
    let later = now + PHONE_TYPING_TIMEOUT;

    let rows = sized_screen(&app, later, 40, 3);
    let mut steps = typing(&mut app, "late", later);
    steps.extend(press(&mut app, &[Key::Enter], later));

    assert_eq!(rows[0], format!("{SESSION} · app · read-only"));
    assert_eq!(sent(&steps), []);
}

#[test]
fn esc_stops_typing_and_esc_again_goes_back_to_the_list_layout() {
    let now = Instant::now();
    let mut app = phone_app(40, 20, now);
    opened(&mut app, now);
    press(&mut app, &[Key::Char('i')], now);

    let stopped = sent(&press(&mut app, &[Key::Esc], now));
    let after_stop = sent(&typing(&mut app, "x", now));
    let back = sent(&press(&mut app, &[Key::Esc], now));
    let rows = sized_screen(&app, now, 40, 20);

    assert_eq!(stopped, []);
    assert_eq!(after_stop, []);
    assert_eq!(back, [fit(false)]);
    assert!(rows[0].starts_with(">  app"), "{rows:#?}");
}

#[test]
fn leaving_the_session_reads_only_when_it_is_opened_again() {
    let now = Instant::now();
    let mut app = phone_app(40, 3, now);
    opened(&mut app, now);
    press(&mut app, &[Key::Char('i'), Key::Esc, Key::Esc], now);

    opened(&mut app, now);
    let mut steps = typing(&mut app, "abc", now);
    steps.extend(press(&mut app, &[Key::Enter], now));

    assert_eq!(sent(&steps), []);
}

#[test]
fn a_request_arriving_while_a_session_is_open_is_never_decided_from_it() {
    let now = Instant::now();
    let mut app = phone_app(40, 3, now);
    opened(&mut app, now);
    asking(&mut app, vec![run(3, SESSION, "make test")], now);

    let steps = press(
        &mut app,
        &[Key::Char('y'), Key::Char('x')],
        after_grace(now),
    );
    let rows = sized_screen(&app, after_grace(now), 40, 3);

    assert_eq!(sent(&steps), []);
    assert_eq!(rows[2], "! 1 waiting on you: Esc shows them");
}

#[test]
fn back_from_the_session_the_card_waits_out_its_grace_again() {
    let now = Instant::now();
    let mut app = phone_app(40, 20, now);
    opened(&mut app, now);
    asking(&mut app, vec![run(3, SESSION, "make test")], now);
    let back_at = after_grace(now);
    press(&mut app, &[Key::Esc], back_at);

    let too_soon = sent(&press(&mut app, &[Key::Char('y')], back_at));
    let read = sent(&press(
        &mut app,
        &[Key::Char('y')],
        after_grace(after_grace(back_at)),
    ));

    assert_eq!(too_soon, []);
    assert!(
        matches!(
            read.as_slice(),
            [Command::ApprovalApprove { request: 3, .. }]
        ),
        "{read:?}"
    );
}

#[test]
fn a_desktop_wide_fit_closes_the_open_session_and_draws_the_desktop_layout() {
    let now = Instant::now();
    let mut desktop = app_with(
        &["app"],
        vec![session(SESSION, 1, 1), session(OTHER, 1, 2)],
        now,
    );
    let mut app = phone_app(100, 40, now);
    opened(&mut app, now);

    let next = fitted(&mut app, true, false, now);
    fitted(&mut desktop, false, false, now);

    assert_eq!(next, None);
    assert_eq!(screen(&app, now, 100), screen(&desktop, now, 100));
}

#[test]
fn a_fit_for_a_layout_the_owner_left_since_is_asked_again() {
    let now = Instant::now();
    let mut app = phone_app(40, 20, now);
    opened(&mut app, now);
    press(&mut app, &[Key::Esc], now);

    let next = fitted(&mut app, true, true, now);

    assert_eq!(next, Some(fit(false)));
}

#[test]
fn on_the_phone_keys_the_column_took_from_the_viewer_stay_in_the_column() {
    let now = Instant::now();
    let mut app = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    let focus = queue(
        &mut app,
        vec![run(3, SESSION, "make test")],
        Vec::new(),
        now,
    )
    .unwrap();
    answer(
        &mut app,
        focus,
        Reply::ManagerFocused {
            took_from_viewer: true,
        },
        now,
    );
    fitted(&mut app, false, true, now);

    let emptied = queue(&mut app, Vec::new(), Vec::new(), now);

    assert_ne!(
        emptied,
        Some(Command::ManagerFocus {
            pane: ManagerPaneView::Viewer
        })
    );
    assert_eq!(emptied, None);
}

#[test]
fn the_help_lists_the_phone_keys() {
    assert!(HELP.contains("Phone layout"));
    assert!(HELP.contains("full screen, read-only"));
    assert!(HELP.contains("i                  type into it"));
}
