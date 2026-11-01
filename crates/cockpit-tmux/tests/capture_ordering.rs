//! A pane's output never reaches a client before that pane's first snapshot, even when the
//! pane writes between the observer's attach and tmux running the snapshot (#267).

mod common;

use cockpit_tmux::{CaptureFrame, CaptureOptions};
use common::{GENEROUS, TestServer};

/// Each attach is one chance for output to land before the snapshot; a busy pane writes
/// within that window most of the time, so this many attaches miss it only by luck.
const ATTACHES: usize = 20;

#[test]
fn a_busy_pane_s_first_frame_is_its_snapshot() {
    let fixture = TestServer::new();
    let (session, pane) = fixture.start_shell(
        "agent-1",
        "i=0; while true; do i=$((i+1)); echo tick-$i; sleep 0.001; done",
    );

    for attach in 0..ATTACHES {
        let mut stream = fixture
            .server
            .capture_stream(&session, CaptureOptions::default())
            .expect("opens");
        let first = stream.next_frame(GENEROUS);
        assert!(
            matches!(first, Ok(Some(CaptureFrame::Snapshot { pane: ref p, .. })) if *p == pane),
            "attach {attach}: {first:?}"
        );
    }
}
