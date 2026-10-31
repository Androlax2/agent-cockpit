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
    Capability, Change, ChangeBatch, ChangeEvent, Command, CommitRevision, Cursor, DatabaseEpoch,
    EntityKind, EntityRef, EntityRevision, FeedPosition, Hello, Outcome, Reply, Request, Response,
    SUPPORTED_VERSIONS, ServerFrame, SessionView, negotiate,
};
use cockpit_tui::{App, FeedEvent, follow_feed};
use common::{checkout, screen_text, session};
use serde_json::Value;
use tokio::sync::mpsc;

const GENEROUS: Duration = Duration::from_secs(5);

fn upsert(kind: EntityKind, id: &str, view: &impl serde::Serialize) -> ChangeEvent {
    let Value::Object(state) = serde_json::to_value(view).unwrap() else {
        panic!("a view is a JSON object");
    };
    ChangeEvent {
        entity: EntityRef {
            kind,
            id: id.to_owned(),
        },
        entity_revision: EntityRevision(1),
        change: Change::Upsert { state },
    }
}

fn snapshot_with(session: &SessionView) -> ChangeBatch {
    ChangeBatch {
        follows: FeedPosition::Snapshot,
        cursor: Cursor {
            epoch: DatabaseEpoch("e1".to_owned()),
            revision: CommitRevision(7),
        },
        changes: vec![
            upsert(EntityKind::Checkout, "1", &checkout(1, "web")),
            upsert(EntityKind::Session, &session.id, session),
        ],
    }
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
    app.apply_changes(&ChangeBatch {
        follows: FeedPosition::Snapshot,
        cursor: Cursor {
            epoch: DatabaseEpoch("e1".to_owned()),
            revision: CommitRevision(9),
        },
        changes: vec![upsert(EntityKind::Session, "s-1", &newer)],
    })
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
