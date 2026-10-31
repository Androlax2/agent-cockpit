//! Agent sessions outlive the daemon: a restarted daemon's new handle finds them, with the
//! same processes, and observes them again.

mod common;

use std::process::Command;
use std::time::Instant;

use cockpit_tmux::{CaptureFrame, CaptureOptions, CaptureStream};
use common::{GENEROUS, TestServer};

fn parent_pid(pid: u32) -> u32 {
    let output = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("a parent pid")
}

/// The stream's first frame other than output. The ticking pane can write between the
/// observer's attach and tmux running the snapshot command, so output may come first; the
/// snapshot taken after it already shows it.
fn first_frame_after_output(stream: &mut CaptureStream) -> CaptureFrame {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match stream.next_frame(left) {
            Ok(Some(CaptureFrame::Output { .. })) if !left.is_zero() => {}
            Ok(Some(frame)) => return frame,
            other => panic!("expected a frame within {GENEROUS:?}, got {other:?}"),
        }
    }
}

#[test]
fn the_tmux_server_is_not_a_child_of_the_process_that_started_it() {
    let fixture = TestServer::new();
    fixture.start_shell("agent-1", "sleep 30");

    let server_pid = fixture
        .server
        .server_pid()
        .expect("asks")
        .expect("a server runs");

    assert_ne!(parent_pid(server_pid), std::process::id());
}

#[test]
fn a_new_handle_finds_the_sessions_and_observes_them_again() {
    let fixture = TestServer::new();
    let (session, pane) = fixture.start_shell(
        "agent-1",
        "i=0; while true; do i=$((i+1)); echo tick-$i; sleep 0.1; done",
    );
    let before = fixture.server.panes(&session).expect("lists");
    let server_pid = fixture.server.server_pid().expect("asks");
    {
        let mut first_stream = fixture
            .server
            .capture_stream(&session, CaptureOptions::default())
            .expect("opens");
        assert!(matches!(
            first_frame_after_output(&mut first_stream),
            CaptureFrame::Snapshot { .. }
        ));
    }

    let restarted = fixture.reopen();

    assert_eq!(restarted.sessions().expect("lists"), vec![session.clone()]);
    assert_eq!(restarted.panes(&session).expect("lists"), before);
    assert_eq!(restarted.server_pid().expect("asks"), server_pid);
    let mut stream = restarted
        .capture_stream(&session, CaptureOptions::default())
        .expect("reattaches");
    let snapshot = first_frame_after_output(&mut stream);
    assert!(
        matches!(snapshot, CaptureFrame::Snapshot { pane: ref p, .. } if *p == pane),
        "{snapshot:?}"
    );
    assert!(matches!(
        stream.next_frame(GENEROUS),
        Ok(Some(CaptureFrame::Output { .. }))
    ));
}
