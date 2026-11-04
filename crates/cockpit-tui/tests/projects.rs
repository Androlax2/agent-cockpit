//! Issue #727 in the TUI: `P` lists the registered projects; `d` asks to remove the selected
//! one and only `y` sends the removal.

mod common;

use std::time::Instant;

use cockpit_protocol::{Command, Reply};
use cockpit_tui::Key;
use common::{answer, app_with, press, project, screen_text, sent};

fn remove(project: &str) -> Command {
    Command::ProjectRemove {
        project: project.to_owned(),
    }
}

#[test]
fn the_project_list_shows_every_registered_project() {
    let now = Instant::now();
    let mut app = app_with(&["app", "web"], Vec::new(), now);

    app.key(Key::Char('P'), now);
    let shown = screen_text(&app, now);

    assert!(shown.contains("/p/app"), "{shown}");
    assert!(shown.contains("/p/web"), "{shown}");
}

#[test]
fn d_then_y_removes_the_selected_project() {
    let now = Instant::now();
    let mut app = app_with(&["app", "web"], Vec::new(), now);

    let asked = press(
        &mut app,
        &[Key::Char('P'), Key::Char('j'), Key::Char('d')],
        now,
    );
    let shown = screen_text(&app, now);
    let confirmed = app.key(Key::Char('y'), now);

    assert_eq!(sent(&asked), Vec::<Command>::new());
    assert!(
        shown.contains("remove project web? y to confirm"),
        "{shown}"
    );
    assert_eq!(confirmed.send, [remove("web")]);
}

#[test]
fn any_key_but_y_cancels_the_removal() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);

    let steps = press(
        &mut app,
        &[
            Key::Char('P'),
            Key::Char('d'),
            Key::Char('n'),
            Key::Char('y'),
        ],
        now,
    );

    assert_eq!(sent(&steps), Vec::<Command>::new());
}

#[test]
fn a_removed_project_leaves_the_list() {
    let now = Instant::now();
    let mut app = app_with(&["app", "web"], Vec::new(), now);
    app.key(Key::Char('P'), now);

    answer(
        &mut app,
        remove("web"),
        Reply::ProjectRemoved {
            project: project("web"),
            kept_on_disk: Vec::new(),
        },
        now,
    );
    let shown = screen_text(&app, now);

    assert!(!shown.contains("/p/web"), "{shown}");
}

#[test]
fn a_refused_removal_is_shown_with_the_daemons_reason() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    app.key(Key::Char('P'), now);

    app.answer(
        &remove("app"),
        Err("conflict: project app is in use: session s1".to_owned()),
        now,
    );
    let shown = screen_text(&app, now);

    assert!(shown.contains("project app is in use"), "{shown}");
}

#[test]
fn escape_closes_the_project_list() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);

    let steps = press(&mut app, &[Key::Char('P'), Key::Esc], now);
    let shown = screen_text(&app, now);

    assert_eq!(
        steps.iter().map(|step| step.used).collect::<Vec<_>>(),
        [Some("projects"), Some("projects-close")]
    );
    assert!(!shown.contains("/p/app"), "{shown}");
}
