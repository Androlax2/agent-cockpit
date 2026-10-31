//! Issue #199: the TUI follows the daemon's change feed instead of polling the session and
//! checkout lists. A scripted daemon on a real Unix socket records every request the TUI's
//! feed task sends.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use cockpit_protocol::{
    Capability, ChangeBatch, Command, EntityKind, FeedPosition, Hello, Outcome, Reply, Request,
    Response, SUPPORTED_VERSIONS, ServerFrame, SessionView, negotiate,
};
use cockpit_tui::{App, FeedEvent, follow_feed};
use common::{after_snapshot, checkout, screen_text, session, snapshot, tombstone, upsert};
use tokio::sync::mpsc;

const GENEROUS: Duration = Duration::from_secs(5);

fn snapshot_with(session: &SessionView) -> ChangeBatch {
    snapshot(vec![
        upsert(EntityKind::Checkout, "1", &checkout(1, "web")),
        upsert(EntityKind::Session, &session.id, session),
    ])
}

/// A daemon that offers the change feed, answers `subscribe` with `first`, then keeps the
/// connection open. Each command it is sent is reported on `requests` before it is answered.
fn scripted_daemon(socket: PathBuf, first: ChangeBatch, requests: std_mpsc::Sender<Command>) {
    let listener = UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut lines = BufReader::new(stream).lines();
        let hello: Hello = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
        send_frame(
            &mut writer,
            &negotiate(&hello, &SUPPORTED_VERSIONS, &[Capability::ChangeFeed]),
        );
        for line in lines {
            let Ok(line) = line else { return };
            let request: Request = serde_json::from_str(&line).unwrap();
            let is_subscribe = matches!(request.command, Command::Subscribe { .. });
            if requests.send(request.command).is_err() {
                return;
            }
            if is_subscribe {
                send_frame(
                    &mut writer,
                    &ServerFrame::Response(Response {
                        id: request.id,
                        outcome: Outcome::Ok(Reply::Subscribed {
                            cursor: first.cursor.clone(),
                        }),
                    }),
                );
                send_frame(&mut writer, &ServerFrame::Changes(first.clone()));
            }
        }
    });
}

fn send_frame(writer: &mut impl Write, frame: &impl serde::Serialize) {
    let mut line = serde_json::to_vec(frame).unwrap();
    line.push(b'\n');
    writer.write_all(&line).unwrap();
}

#[tokio::test]
async fn a_session_appears_in_the_list_from_the_feed_alone() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("daemon.sock");
    let started = session("s-from-feed", 1, 100);
    let (requests, sent) = std_mpsc::channel();
    scripted_daemon(socket.clone(), snapshot_with(&started), requests);
    let (events, mut received) = mpsc::channel(1);
    tokio::spawn(follow_feed(socket, events));
    let mut app = App::new();

    let event = tokio::time::timeout(GENEROUS, received.recv())
        .await
        .expect("the feed delivers a batch")
        .expect("the feed task runs");
    let FeedEvent::Batch { batch, applied } = event else {
        panic!("expected a batch, got {event:?}");
    };
    let outcome = app.apply_changes(&batch);
    applied.send(outcome.clone()).unwrap();

    assert_eq!(outcome, Ok(()));
    assert!(
        screen_text(&app, Instant::now()).contains("s-from-feed"),
        "{}",
        screen_text(&app, Instant::now())
    );
    let commands: Vec<Command> = sent.try_iter().collect();
    assert!(
        matches!(
            commands.as_slice(),
            [Command::Subscribe {
                from: FeedPosition::Snapshot
            }]
        ),
        "the TUI sent {commands:?}"
    );
}

#[test]
fn the_refresh_no_longer_lists_sessions_or_checkouts() {
    let polled = App::refresh_commands();

    assert!(
        !polled.iter().any(|command| matches!(
            command,
            Command::SessionList { .. } | Command::CheckoutList { .. }
        )),
        "{polled:?}"
    );
}

#[test]
fn a_late_answer_does_not_take_a_session_back_to_an_older_revision() {
    let mut app = App::new();
    let mut newer = session("s-1", 1, 100);
    newer.revision = 5;
    newer.state = cockpit_protocol::SessionStateView::Asleep;
    let mut older = newer.clone();
    older.revision = 4;
    older.state = cockpit_protocol::SessionStateView::Running;
    app.apply_changes(&snapshot(vec![upsert(EntityKind::Session, "s-1", &newer)]))
        .unwrap();

    common::answer(
        &mut app,
        Command::SessionWake {
            session: "s-1".to_owned(),
        },
        Reply::Session {
            session: Box::new(older),
        },
        Instant::now(),
    );

    assert_eq!(
        app.board().listed_sessions()[0].state,
        cockpit_protocol::SessionStateView::Asleep
    );
}

#[test]
fn a_late_answer_does_not_bring_back_a_session_the_feed_took_off() {
    let mut app = App::new();
    let shown = session("s-1", 1, 100);
    app.apply_changes(&snapshot(vec![upsert(EntityKind::Session, "s-1", &shown)]))
        .unwrap();
    app.apply_changes(&after_snapshot(vec![tombstone(EntityKind::Session, "s-1")]))
        .unwrap();

    common::answer(
        &mut app,
        Command::SessionShow {
            session: "s-1".to_owned(),
        },
        Reply::Session {
            session: Box::new(shown),
        },
        Instant::now(),
    );

    assert!(app.board().listed_sessions().is_empty());
}

#[test]
fn a_lost_feed_stays_on_the_status_line_after_a_command_goes_through() {
    let started = session("s-1", 1, 100);
    let mut app = common::app_with(&["web"], vec![started.clone()], Instant::now());
    app.feed_lost("the daemon closed the connection".to_owned());

    common::answer(
        &mut app,
        Command::SessionRestart {
            session: "s-1".to_owned(),
        },
        Reply::Session {
            session: Box::new(started),
        },
        Instant::now(),
    );

    assert!(
        screen_text(&app, Instant::now()).contains("the daemon closed the connection"),
        "{}",
        screen_text(&app, Instant::now())
    );
}
