//! The capture stream clients render from, on a real private tmux server.

mod common;

use std::time::{Duration, Instant};

use cockpit_tmux::{CaptureFrame, CaptureOptions, CaptureStream, ObserveOptions};
use common::{GENEROUS, TestServer, contains, wait_for};

fn next(stream: &mut CaptureStream) -> CaptureFrame {
    stream
        .next_frame(GENEROUS)
        .expect("streaming")
        .expect("a frame in time")
}

#[test]
fn the_stream_starts_with_the_screen_then_follows_new_output() {
    let fixture = TestServer::new();
    let script = format!(
        "echo before-attach; {}; echo after-attach; sleep 30",
        wait_for("go")
    );
    let (session, pane) = fixture.start_shell("agent-1", &script);
    common::eventually("the first line is on screen", || {
        contains(
            &fixture.server.capture(&pane, 0).expect("captures"),
            b"before-attach",
        )
    });

    let mut stream = fixture
        .server
        .capture_stream(&session, CaptureOptions::default())
        .expect("opens");
    let CaptureFrame::Snapshot {
        pane: snapshot_pane,
        screen,
    } = next(&mut stream)
    else {
        panic!("the first frame is a snapshot");
    };
    assert_eq!(snapshot_pane, pane);
    assert_eq!(screen, fixture.server.capture(&pane, 0).expect("captures"));

    fixture.release("go");

    let mut output = Vec::new();
    while !contains(&output, b"after-attach") {
        match next(&mut stream) {
            CaptureFrame::Output { pane: from, bytes } => {
                assert_eq!(from, pane);
                output.extend(bytes);
            }
            other => panic!("expected output, got {other:?}"),
        }
    }
}

#[test]
fn a_client_that_falls_behind_gets_a_fresh_snapshot_instead_of_the_backlog() {
    let fixture = TestServer::new();
    let script = format!("{}; while true; do seq 1 3000; done", wait_for("go"));
    let (session, pane) = fixture.start_shell("agent-1", &script);
    let options = CaptureOptions {
        observe: ObserveOptions {
            pause_after_seconds: 1,
            capacity: 4,
        },
        history_lines: 0,
    };
    let mut stream = fixture
        .server
        .capture_stream(&session, options)
        .expect("opens");
    assert!(matches!(next(&mut stream), CaptureFrame::Snapshot { .. }));

    fixture.release("go");
    std::thread::sleep(Duration::from_secs(3));

    let deadline = Instant::now() + GENEROUS;
    loop {
        assert!(
            Instant::now() < deadline,
            "no re-snapshot after falling behind"
        );
        if let CaptureFrame::Snapshot {
            pane: resynced,
            screen,
        } = next(&mut stream)
        {
            assert_eq!(resynced, pane);
            assert!(!screen.is_empty());
            break;
        }
    }
    assert!(matches!(
        next(&mut stream),
        CaptureFrame::Output { .. } | CaptureFrame::Snapshot { .. }
    ));
}

#[test]
fn the_stream_ends_when_the_session_closes() {
    let fixture = TestServer::new();
    let (session, _) = fixture.start_shell("agent-1", "sleep 30");
    let mut stream = fixture
        .server
        .capture_stream(&session, CaptureOptions::default())
        .expect("opens");
    assert!(matches!(next(&mut stream), CaptureFrame::Snapshot { .. }));

    fixture.server.close_session(&session).expect("closes");

    loop {
        if let CaptureFrame::Ended { .. } = next(&mut stream) {
            break;
        }
    }
}
