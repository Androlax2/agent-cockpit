//! Network requests in the approval pane (#674): a host a sandboxed session was refused waits
//! on the owner, who allows it once (`y`), always (`a`) or denies it (`x`), after the grace.

mod common;

use std::time::{Duration, Instant};

use cockpit_protocol::{
    Command, DecidedThroughView, NetworkRequestStateView, NetworkRequestView, NetworkSubjectView,
    NetworkTargetView, Reply,
};
use cockpit_tui::{App, INPUT_GRACE, Key};
use common::{answer, app_with, screen_text, session};

const SESSION: &str = "s00000000000a";

fn asking(id: i64) -> NetworkRequestView {
    NetworkRequestView {
        id,
        subject: NetworkSubjectView::Session {
            id: SESSION.to_owned(),
        },
        project: "app".to_owned(),
        asks: NetworkTargetView::Host {
            host: "unknown.example.com".to_owned(),
            port: 443,
        },
        target: "unknown.example.com:443".to_owned(),
        state: NetworkRequestStateView::Asking,
        asked_at: 1_000,
        decision: None,
        grant: None,
    }
}

/// Request `id` for the session's declared `figcraft` service (#880).
fn asking_service(id: i64) -> NetworkRequestView {
    NetworkRequestView {
        asks: NetworkTargetView::Service {
            service: "figcraft".to_owned(),
        },
        target: "service figcraft listen 127.0.0.1:3845 target 127.0.0.1:3845".to_owned(),
        ..asking(id)
    }
}

/// A network request shown, and tmux's keys for the column there at once.
fn app_asked(now: Instant) -> App {
    app_asked_for(asking(7), now)
}

fn app_asked_for(request: NetworkRequestView, now: Instant) -> App {
    let mut app = app_with(&["app"], vec![session(SESSION, 1, 1)], now);
    let listed = answer(
        &mut app,
        Command::NetworkList,
        Reply::NetworkRequests {
            requests: vec![request],
        },
        now,
    );
    if let Some(asked_for_keys) = listed {
        answer(
            &mut app,
            asked_for_keys,
            Reply::ManagerFocused {
                took_from_viewer: false,
            },
            now,
        );
    }
    app
}

fn after_grace(now: Instant) -> Instant {
    now + INPUT_GRACE + Duration::from_millis(1)
}

#[test]
fn y_after_the_grace_allows_the_host_for_the_rest_of_the_session() {
    let now = Instant::now();
    let mut app = app_asked(now);

    let allowed = app.key(Key::Char('y'), after_grace(now));

    assert_eq!(
        allowed.send,
        [Command::NetworkAllow {
            request: 7,
            always: false,
            through: Some(DecidedThroughView::Tui),
        }]
    );
}

#[test]
fn a_after_the_grace_allows_the_host_always() {
    let now = Instant::now();
    let mut app = app_asked(now);

    let allowed = app.key(Key::Char('a'), after_grace(now));

    assert_eq!(
        allowed.send,
        [Command::NetworkAllow {
            request: 7,
            always: true,
            through: Some(DecidedThroughView::Tui),
        }]
    );
}

#[test]
fn x_after_the_grace_denies_the_host() {
    let now = Instant::now();
    let mut app = app_asked(now);

    let denied = app.key(Key::Char('x'), after_grace(now));

    assert_eq!(
        denied.send,
        [Command::NetworkDeny {
            request: 7,
            through: Some(DecidedThroughView::Tui),
        }]
    );
}

#[test]
fn an_allow_key_within_the_grace_decides_nothing() {
    let now = Instant::now();
    let mut app = app_asked(now);

    let early = app.key(Key::Char('a'), now + INPUT_GRACE - Duration::from_millis(1));

    assert_eq!(
        (early.used, early.send),
        (Some("approval-ignored-in-grace"), Vec::new())
    );
}

#[test]
fn the_pane_names_the_host_the_session_and_what_each_key_does() {
    let now = Instant::now();
    let app = app_asked(now);

    let screen = screen_text(&app, after_grace(now));

    for expected in [
        "Network request 7 from s00000000000a (project app)",
        "Host: unknown.example.com:443",
        "y allow once   a allow always   x deny",
    ] {
        assert!(screen.contains(expected), "{expected:?} not in:\n{screen}");
    }
}

#[test]
fn a_session_with_an_asking_network_request_needs_the_owner() {
    let now = Instant::now();
    let app = app_asked(now);

    assert!(app.board().needs_you(SESSION));
}

#[test]
fn the_pane_names_a_closed_service_by_its_record() {
    let now = Instant::now();
    let app = app_asked_for(asking_service(8), now);

    let screen = screen_text(&app, after_grace(now));

    for expected in [
        "Network request 8 from s00000000000a (project app)",
        "Service: service figcraft listen 127.0.0.1:3845",
        "y allow once   a allow always   x deny",
    ] {
        assert!(screen.contains(expected), "{expected:?} not in:\n{screen}");
    }
    assert!(
        !screen.contains("Host: "),
        "a service is shown as a host:\n{screen}"
    );
}

#[test]
fn a_after_the_grace_allows_the_service_always() {
    let now = Instant::now();
    let mut app = app_asked_for(asking_service(8), now);

    let allowed = app.key(Key::Char('a'), after_grace(now));

    assert_eq!(
        allowed.send,
        [Command::NetworkAllow {
            request: 8,
            always: true,
            through: Some(DecidedThroughView::Tui),
        }]
    );
}
