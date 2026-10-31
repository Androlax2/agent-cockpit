//! Bounded draining: a viewer that falls behind the window reads a gap marker naming exactly
//! the bytes it missed, and the daemon never holds more than the window (report 03).

mod common;

use cockpit_processes::{OutputKind, OutputLimits, OutputRead};
use common::*;

const WINDOW: usize = 1024;
const WRITTEN: u64 = 10_000;

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_viewer_reads_a_gap_marker_then_the_bytes_still_held() {
    let home = Home::new();
    let database = home.open_database().await;
    let mut supervisor = home.process_group_supervisor();
    supervisor.output_limits = OutputLimits {
        window_bytes: WINDOW,
        chunk_bytes: 256,
        ..OutputLimits::default()
    };
    let execution = supervisor
        .launch(
            &database,
            &shell(&home, &format!("head -c {WRITTEN} /dev/zero")),
        )
        .await
        .unwrap();
    let stream = supervisor
        .follow_output(&database, &execution, OutputKind::Stdout)
        .unwrap();
    let held_start = WRITTEN - WINDOW as u64;
    // The viewer reads nothing until everything was drained.
    assert_eq!(stream.read(WRITTEN, usize::MAX).await, OutputRead::Finished);

    assert_eq!(
        stream.read(0, usize::MAX).await,
        OutputRead::Gap {
            from: 0,
            to: held_start
        }
    );
    let OutputRead::Bytes { offset, bytes } = stream.read(held_start, usize::MAX).await else {
        panic!("the window holds the last bytes");
    };
    assert_eq!((offset, bytes.len()), (held_start, WINDOW));
    assert_eq!(stream.held_bytes(), WINDOW);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_viewer_reads_at_most_the_bytes_it_asked_for() {
    let home = Home::new();
    let database = home.open_database().await;
    let supervisor = home.process_group_supervisor();
    let execution = supervisor
        .launch(&database, &shell(&home, "printf abcdef"))
        .await
        .unwrap();
    let stream = supervisor
        .follow_output(&database, &execution, OutputKind::Stdout)
        .unwrap();

    let mut read = Vec::new();
    let mut cursor = 0;
    loop {
        match stream.read(cursor, 4).await {
            OutputRead::Bytes { offset, bytes } => {
                assert!(bytes.len() <= 4);
                assert_eq!(offset, cursor);
                cursor += bytes.len() as u64;
                read.extend(bytes);
            }
            OutputRead::Finished => break,
            other => panic!("unexpected {other:?}"),
        }
    }

    assert_eq!(read, b"abcdef");
}
