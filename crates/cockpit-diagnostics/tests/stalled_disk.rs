//! A FIFO nobody reads stands in for a stalled disk: once its pipe buffer fills, every write
//! blocks forever, exactly like a hung filesystem.

mod support;

use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use support::start;

fn stalled_log(log_dir: &Path) -> File {
    let fifo = log_dir.join("cockpit.log");
    let status = Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo runs");
    assert!(status.success(), "mkfifo failed");
    // Opening read-write keeps the FIFO open without reading, so the writer's open succeeds.
    File::options()
        .read(true)
        .write(true)
        .open(&fifo)
        .expect("the FIFO opens")
}

#[test]
fn a_stalled_disk_does_not_block_the_logging_thread() {
    let log_dir = tempfile::tempdir().unwrap();
    let _held_open = stalled_log(log_dir.path());
    let diagnostics = start(log_dir.path(), "info");

    // A separate thread, so a blocking writer fails this test instead of hanging it.
    let (finished, finished_signal) = mpsc::channel();
    let dispatch = diagnostics.dispatch().clone();
    thread::spawn(move || {
        tracing::dispatcher::with_default(&dispatch, || {
            for index in 0..20_000 {
                tracing::info!(index, padding = "x".repeat(200), "control path keeps going");
            }
        });
        finished.send(()).unwrap();
    });

    assert!(
        finished_signal.recv_timeout(Duration::from_secs(5)).is_ok(),
        "logging blocked on the stalled disk"
    );
    assert!(
        diagnostics.dropped_records() > 0,
        "the drop counter shows the lost records"
    );
    let shutdown_started = Instant::now();
    assert!(
        !diagnostics.shutdown(Duration::from_millis(200)),
        "a stalled writer cannot drain"
    );
    assert!(
        shutdown_started.elapsed() < Duration::from_secs(2),
        "shutdown is bounded"
    );
}

#[test]
fn dropped_records_are_reported_in_the_log_once_writing_resumes() {
    let log_dir = tempfile::tempdir().unwrap();
    let mut options =
        cockpit_diagnostics::Options::new(log_dir.path().to_path_buf(), "info".parse().unwrap());
    options.queue_records = 1;
    let diagnostics = cockpit_diagnostics::start(options).unwrap();
    tracing::dispatcher::with_default(diagnostics.dispatch(), || {
        for index in 0..2_000 {
            tracing::info!(index, "burst");
        }
    });
    assert!(diagnostics.shutdown(support::SHUTDOWN_TIMEOUT));
    let dropped = diagnostics.dropped_records();
    assert!(dropped > 0, "a one-record queue drops part of a burst");
    let reported: u64 = support::records(log_dir.path())
        .iter()
        .filter(|record| record["fields"]["message"] == "dropped log records")
        .map(|record| record["fields"]["dropped"].as_u64().unwrap())
        .sum();
    assert_eq!(reported, dropped);
}
