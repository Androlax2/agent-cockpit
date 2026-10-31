//! The `N` form (#35): a fresh install with one known project starts a session from the TUI,
//! and the card appears under that project.

mod common;

use std::time::Instant;

use cockpit_protocol::{Command, ProviderView, Reply};
use cockpit_tui::{App, Key};
use common::{answer, app_with, press, screen_text, sent, session, typing};

fn providers(names: &[&str]) -> Reply {
    Reply::Providers {
        providers: names
            .iter()
            .map(|name| ProviderView {
                name: (*name).to_owned(),
                resume: true,
            })
            .collect(),
    }
}

/// A fresh install: one project, no session, the form open and the agents answered.
fn form_open(agents: &[&str], now: Instant) -> App {
    let mut app = app_with(&["app"], Vec::new(), now);
    let opened = app.key(Key::Char('N'), now);
    assert_eq!(opened.send, [Command::ProviderList]);
    answer(&mut app, Command::ProviderList, providers(agents), now);
    app
}

#[test]
fn a_fresh_install_starts_a_session_from_the_form_and_its_card_appears_under_the_project() {
    let now = Instant::now();
    let mut app = form_open(&["fake"], now);

    press(&mut app, &[Key::Tab, Key::Tab], now);
    typing(&mut app, "fix the build", now);
    let submitted = app.key(Key::Enter, now);
    let command = submitted.send[0].clone();
    assert_eq!(
        command,
        Command::SessionNew {
            dir: "/p/app".to_owned(),
            agent: Some("fake".to_owned()),
            prompt: Some("fix the build".to_owned()),
            clone: None,
        }
    );
    app.answer(
        &command,
        Ok(Reply::Session {
            session: Box::new(session("s00000000000a", 1, 5)),
        }),
        now,
    );

    let shown = screen_text(&app, now);
    assert!(app.form().is_none());
    assert!(
        shown.contains("│app") && shown.contains("│>  s00000000000a  fake  running"),
        "{shown}"
    );
}

#[test]
fn a_new_clone_needs_a_branch_and_sends_it() {
    let now = Instant::now();
    let mut app = form_open(&["fake"], now);
    press(&mut app, &[Key::Tab, Key::Tab, Key::Tab, Key::Right], now);

    let refused = app.key(Key::Enter, now);
    let refused_screen = screen_text(&app, now);
    press(&mut app, &[Key::Tab], now);
    typing(&mut app, "feature/x", now);
    let submitted = app.key(Key::Enter, now);

    assert!(refused.send.is_empty());
    assert!(refused_screen.contains("Cannot start: a new clone needs a branch"));
    assert_eq!(
        submitted.send,
        [Command::SessionNew {
            dir: "/p/app".to_owned(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: Some("feature/x".to_owned()),
        }]
    );
}

#[test]
fn a_typed_project_path_must_be_absolute() {
    let now = Instant::now();
    let mut app = form_open(&["fake"], now);
    let clear: Vec<Key> = std::iter::repeat_n(Key::Backspace, "/p/app".len()).collect();
    press(&mut app, &clear, now);
    typing(&mut app, "relative/dir", now);

    let refused = app.key(Key::Enter, now);

    assert!(refused.send.is_empty());
    assert!(screen_text(&app, now).contains("Cannot start: the project needs an absolute path"));
}

#[test]
fn a_typed_absolute_path_is_sent_as_typed() {
    let now = Instant::now();
    let mut app = form_open(&["fake"], now);
    let clear: Vec<Key> = std::iter::repeat_n(Key::Backspace, "/p/app".len()).collect();
    press(&mut app, &clear, now);
    typing(&mut app, "/p/app/src", now);

    let submitted = app.key(Key::Enter, now);

    assert!(matches!(
        &submitted.send[..],
        [Command::SessionNew { dir, .. }] if dir == "/p/app/src"
    ));
}

#[test]
fn left_and_right_choose_among_the_known_projects_and_the_agents() {
    let now = Instant::now();
    let mut app = app_with(&["api", "web"], Vec::new(), now);
    app.key(Key::Char('N'), now);
    answer(
        &mut app,
        Command::ProviderList,
        providers(&["claude", "codex"]),
        now,
    );

    press(&mut app, &[Key::Right, Key::Tab, Key::Left], now);
    let submitted = app.key(Key::Enter, now);

    assert!(matches!(
        &submitted.send[..],
        [Command::SessionNew { dir, agent: Some(agent), .. }]
            if dir == "/p/web" && agent == "codex"
    ));
}

#[test]
fn without_an_available_agent_the_form_says_so_and_sends_nothing() {
    let now = Instant::now();
    let mut app = form_open(&[], now);

    let refused = app.key(Key::Enter, now);

    assert!(refused.send.is_empty());
    let shown = screen_text(&app, now);
    assert!(shown.contains("Agent: none available"), "{shown}");
    assert!(
        shown.contains("Cannot start: no agent is available"),
        "{shown}"
    );
}

#[test]
fn a_refused_start_keeps_the_form_open_with_the_daemons_reason() {
    let now = Instant::now();
    let mut app = form_open(&["fake"], now);
    let command = app.key(Key::Enter, now).send.remove(0);

    app.answer(
        &command,
        Err("not-found: /p/app is not in a project".to_owned()),
        now,
    );

    assert!(app.form().is_some());
    assert!(screen_text(&app, now).contains("Cannot start: not-found: /p/app is not in a project"));
}

#[test]
fn escape_closes_the_form_without_sending() {
    let now = Instant::now();
    let mut app = form_open(&["fake"], now);

    let steps = press(&mut app, &[Key::Esc], now);

    assert!(app.form().is_none());
    assert_eq!(sent(&steps), Vec::<Command>::new());
}

#[test]
fn a_second_enter_before_the_daemon_answers_sends_no_second_session() {
    let now = Instant::now();
    let mut app = form_open(&["fake"], now);

    let steps = press(&mut app, &[Key::Enter, Key::Enter], now);

    assert_eq!(sent(&steps).len(), 1);
}

#[test]
fn the_answer_to_an_abandoned_form_leaves_the_form_opened_since() {
    let now = Instant::now();
    let mut app = form_open(&["fake"], now);
    let first = app.key(Key::Enter, now).send.remove(0);
    press(&mut app, &[Key::Esc, Key::Char('N')], now);
    answer(&mut app, Command::ProviderList, providers(&["fake"]), now);

    app.answer(
        &first,
        Err("refused: the project is not trusted".to_owned()),
        now,
    );

    let form = app.form().expect("the form opened since stays");
    assert!(
        !screen_text(&app, now).contains("Cannot start"),
        "{}",
        screen_text(&app, now)
    );
    assert_eq!(form.field(), cockpit_tui::Field::Project);
}
