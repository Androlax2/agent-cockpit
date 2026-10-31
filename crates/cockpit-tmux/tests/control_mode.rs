//! Control-mode observation on a real private tmux server.

mod common;

use std::time::{Duration, Instant};

use cockpit_tmux::{ControlEvent, Error, ObserveOptions, Observer, PaneId, SessionName};
use common::{GENEROUS, TestServer, contains, wait_for};

/// The first event `matches` accepts, failing after [`GENEROUS`] with `what`.
fn first_event(
    observer: &Observer,
    what: &str,
    matches: impl Fn(&ControlEvent) -> bool,
) -> ControlEvent {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for {what}");
        if let Some(event) = observer.next_event(remaining).expect("observing")
            && matches(&event)
        {
            return event;
        }
    }
}

/// Bytes `pane` wrote until `until` holds for them, failing after [`GENEROUS`].
fn output_until(observer: &Observer, pane: &PaneId, until: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let deadline = Instant::now() + GENEROUS;
    let mut output = Vec::new();
    while !until(&output) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out; got {:?}",
            String::from_utf8_lossy(&output)
        );
        if let Some(ControlEvent::Output { pane: from, bytes }) =
            observer.next_event(remaining).expect("observing")
        {
            assert_eq!(&from, pane);
            output.extend(bytes);
        }
    }
    output
}

#[test]
fn pane_output_arrives_as_the_exact_bytes_written() {
    let fixture = TestServer::new();
    let script = format!(
        "{}; printf 'start\\001\\033[1m\\\\end\\n'; sleep 30",
        wait_for("go")
    );
    let (session, pane) = fixture.start_shell("agent-1", &script);
    let observer = fixture
        .server
        .observe(&session, ObserveOptions::default())
        .expect("attaches");

    fixture.release("go");

    let output = output_until(&observer, &pane, |bytes| contains(bytes, b"end"));
    assert!(contains(&output, b"start\x01\x1b[1m\\end"), "{output:?}");
}

#[test]
fn the_observer_neither_resizes_nor_types_into_the_session() {
    let fixture = TestServer::new();
    let (session, _) = fixture.start_shell("agent-1", "sleep 30");

    let _observer = fixture
        .server
        .observe(&session, ObserveOptions::default())
        .expect("attaches");

    common::eventually("the control client is attached", || {
        !fixture
            .tmux(&["list-clients", "-F", "#{client_flags}"])
            .is_empty()
    });
    let flags = fixture.tmux(&["list-clients", "-F", "#{client_flags}"]);
    assert!(
        flags.contains("read-only") && flags.contains("ignore-size"),
        "{flags}"
    );
    let size = fixture.tmux(&[
        "display-message",
        "-p",
        "-t",
        "=agent-1:",
        "#{window_width}x#{window_height}",
    ]);
    assert_eq!(size.trim(), "100x30");
}

#[test]
fn a_consumer_that_falls_behind_loses_output_and_is_told_so() {
    let fixture = TestServer::new();
    let script = format!("{}; seq 1 100000; sleep 30", wait_for("go"));
    let (session, pane) = fixture.start_shell("agent-1", &script);
    let observer = fixture
        .server
        .observe(
            &session,
            ObserveOptions {
                pause_after_seconds: 1,
                capacity: 4,
            },
        )
        .expect("attaches");

    fixture.release("go");
    std::thread::sleep(Duration::from_secs(1));

    let lost = first_event(&observer, "the loss report", |event| {
        matches!(event, ControlEvent::OutputLost { .. })
    });
    assert!(matches!(lost, ControlEvent::OutputLost { pane: from, .. } if from == pane));
}

#[test]
fn a_consumer_that_never_reads_does_not_slow_the_agent() {
    let fixture = TestServer::new();
    let script = format!("{}; seq 1 300000; touch finished; sleep 30", wait_for("go"));
    let (session, _) = fixture.start_shell("agent-1", &script);
    let _observer = fixture
        .server
        .observe(
            &session,
            ObserveOptions {
                pause_after_seconds: 1,
                capacity: 4,
            },
        )
        .expect("attaches");

    fixture.release("go");

    let finished = fixture.home.path().join("finished");
    common::eventually("the agent finished writing", || finished.exists());
}

#[test]
fn closing_the_session_ends_the_observer() {
    let fixture = TestServer::new();
    let (session, _) = fixture.start_shell("agent-1", "sleep 30");
    let observer = fixture
        .server
        .observe(&session, ObserveOptions::default())
        .expect("attaches");

    fixture.server.close_session(&session).expect("closes");

    first_event(&observer, "the exit event", |event| {
        matches!(event, ControlEvent::Exit { .. })
    });
    assert!(matches!(
        observer.next_event(GENEROUS),
        Err(Error::ObserverClosed)
    ));
}

#[test]
fn observing_a_session_that_does_not_exist_fails_at_once() {
    let fixture = TestServer::new();
    fixture.start_shell("agent-12", "sleep 30");

    let error = fixture
        .server
        .observe(
            &SessionName::new("agent-1").expect("valid"),
            ObserveOptions::default(),
        )
        .err()
        .expect("agent-1 does not exist");

    assert!(matches!(error, Error::CommandFailed { .. }), "{error:?}");
}

#[test]
fn dropping_the_observer_leaves_the_session_running() {
    let fixture = TestServer::new();
    let (session, _) = fixture.start_shell("agent-1", "sleep 30");
    let observer = fixture
        .server
        .observe(&session, ObserveOptions::default())
        .expect("attaches");

    drop(observer);

    assert_eq!(fixture.tmux(&["list-clients"]), "");
    assert_eq!(fixture.server.sessions().expect("lists"), vec![session]);
}

#[test]
fn waiting_without_a_limit_returns_when_the_next_event_comes() {
    let fixture = TestServer::new();
    let (session, _) = fixture.start_shell("agent-1", "sleep 30");
    let observer = fixture
        .server
        .observe(&session, ObserveOptions::default())
        .expect("attaches");
    fixture.server.close_session(&session).expect("closes");

    loop {
        match observer.next_event(Duration::MAX).expect("observing") {
            Some(ControlEvent::Exit { .. }) => break,
            Some(_) => {}
            None => panic!("a wait without limit returned nothing"),
        }
    }
}
