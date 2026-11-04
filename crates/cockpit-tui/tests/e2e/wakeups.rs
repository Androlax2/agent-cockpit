//! Issue #706's measurement: what an idle daemon costs with no TUI, with the TUI open, and the
//! change the issue asks for. Ignored by default: run it before and after a change and
//! compare, with `cargo nextest run --locked --release -p cockpit-tui --test e2e
//! --run-ignored only --no-capture wakeups`.
//!
//! The counts are the daemon's own threads' scheduler slices and voluntary context switches,
//! summed over the threads alive at both ends of the window (a blocking-pool thread that exits
//! in between takes its count with it, so the numbers are a floor).

use std::collections::BTreeMap;
use std::time::Duration;

use crate::harness::{Cockpit, DaemonKind};

const SETTLE: Duration = Duration::from_secs(10);
const WINDOW: Duration = Duration::from_secs(60);

/// `(timeslices, voluntary context switches)` per thread id.
fn sample(pid: u32) -> BTreeMap<String, (u64, u64)> {
    let mut threads = BTreeMap::new();
    for entry in std::fs::read_dir(format!("/proc/{pid}/task")).expect("the daemon runs") {
        let thread = entry.expect("a thread entry").file_name();
        let thread = thread.to_string_lossy().into_owned();
        let base = format!("/proc/{pid}/task/{thread}");
        let (Ok(schedstat), Ok(status)) = (
            std::fs::read_to_string(format!("{base}/schedstat")),
            std::fs::read_to_string(format!("{base}/status")),
        ) else {
            continue;
        };
        let slices = schedstat
            .split_whitespace()
            .nth(2)
            .and_then(|field| field.parse().ok())
            .expect("schedstat has timeslices");
        let switches = status
            .lines()
            .find_map(|line| line.strip_prefix("voluntary_ctxt_switches:"))
            .and_then(|field| field.trim().parse().ok())
            .expect("status has voluntary context switches");
        threads.insert(thread, (slices, switches));
    }
    threads
}

fn per_second(cockpit: &Cockpit) -> (f64, f64) {
    std::thread::sleep(SETTLE);
    let before = sample(cockpit.daemon_pid());
    std::thread::sleep(WINDOW);
    let after = sample(cockpit.daemon_pid());
    let (mut slices, mut switches) = (0, 0);
    for (thread, (later_slices, later_switches)) in &after {
        if let Some((earlier_slices, earlier_switches)) = before.get(thread) {
            slices += later_slices - earlier_slices;
            switches += later_switches - earlier_switches;
        }
    }
    let seconds = WINDOW.as_secs_f64();
    (slices as f64 / seconds, switches as f64 / seconds)
}

#[test]
#[ignore = "a measurement, not a check: run it before and after a change and compare"]
fn measure_idle_daemon_wakeups() {
    let alone = per_second(&Cockpit::start_without_tui(DaemonKind::Product));
    let with_tui = per_second(&Cockpit::start());
    println!(
        "daemon alone:   {:.1} timeslices/s, {:.1} voluntary switches/s",
        alone.0, alone.1
    );
    println!(
        "daemon + TUI:   {:.1} timeslices/s, {:.1} voluntary switches/s",
        with_tui.0, with_tui.1
    );
}
