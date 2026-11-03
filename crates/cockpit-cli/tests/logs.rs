mod common;

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use common::TestHome;

const SECONDS_PER_DAY: i64 = 86_400;
/// POSIX `TZ` reads offsets west of UTC as positive: this is UTC+05:30.
const INDIA: &str = "<+0530>-5:30";
const INDIA_OFFSET_SECONDS: i64 = 19_800;

#[test]
fn owner_can_set_inspect_and_revert_the_running_daemons_level() {
    let mut home = TestHome::new();
    home.daemon_env
        .push(("TORLEK_LOG_LEVEL".into(), "warn".into()));
    let _daemon = home.start();
    let baseline = home.cockpit_ok(&["logs", "level"]);
    assert!(
        baseline.contains("warn") && baseline.contains("no active override"),
        "{baseline}"
    );
    let active = home.cockpit_ok(&["logs", "level", "debug"]);
    assert!(
        active.contains("level: debug until") && active.contains("then back to warn"),
        "{active}"
    );
    assert!(home.cockpit_ok(&["logs", "level"]).contains("debug"));
    for _ in 0..2 {
        let reverted = home.cockpit_ok(&["logs", "level", "--revert"]);
        assert!(
            reverted.contains("warn") && reverted.contains("no active override"),
            "{reverted}"
        );
    }
    assert!(
        home.cockpit_ok(&["logs", "level", "tmux=trace", "--for", "15m"])
            .contains("tmux=trace")
    );
}

#[test]
fn invalid_override_leaves_the_previous_override_in_effect() {
    let home = TestHome::new();
    let _daemon = home.start();
    home.cockpit_ok(&["logs", "level", "debug"]);
    for (level, duration) in [
        ("loud", "1s"),
        ("trace", "0s"),
        ("trace", "16m"),
        ("trace", "18446744073709551615m"),
    ] {
        let failed = home.cockpit(&["logs", "level", level, "--for", duration]);
        assert_eq!(failed.status.code(), Some(1), "{failed:?}");
        assert!(
            String::from_utf8_lossy(&failed.stderr).contains("invalid-argument"),
            "{failed:?}"
        );
        assert!(home.cockpit_ok(&["logs", "level"]).contains("debug"));
    }
}

#[test]
fn malformed_log_arguments_are_usage_errors_without_a_daemon() {
    let home = TestHome::new();
    for arguments in [
        vec!["logs"],
        vec!["logs", "level", "--for"],
        vec!["logs", "level", "--revert", "--for", "1s"],
        vec!["logs", "level", "debug", "--for"],
        vec!["logs", "level", "debug", "--for", "--revert"],
        vec!["logs", "level", "debug", "--for", "--unknown"],
        vec!["logs", "level", "debug", "--for", "1s", "--for", "2s"],
        vec!["logs", "level", "--unknown"],
    ] {
        let output = home.cockpit(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}: {output:?}");
    }
}

#[test]
fn an_override_reads_as_a_local_clock_time_with_what_follows_it() {
    let mut home = TestHome::new();
    home.daemon_env
        .push(("TORLEK_LOG_LEVEL".into(), "info".into()));
    let _daemon = home.start();
    let expected_expiry = utc_seconds_of_day() + 60;
    let utc = level_in_zone(&home, "UTC0", &["logs", "level", "debug", "--for", "1m"]);
    let utc_clock = expiry_clock(&utc, "(in 1m), then back to info");
    assert!(circular_distance(utc_clock, expected_expiry) <= 2, "{utc}");
    let local = level_in_zone(&home, INDIA, &["logs", "level"]);
    let local_clock = expiry_clock(&local, "), then back to info");
    assert_eq!(
        circular_distance(local_clock, utc_clock + INDIA_OFFSET_SECONDS),
        0,
        "{utc}{local}"
    );
}

fn level_in_zone(home: &TestHome, zone: &str, arguments: &[&str]) -> String {
    let output = home
        .base_command(Path::new(env!("CARGO_BIN_EXE_torlek")))
        .env("TZ", zone)
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// The seconds of the day in `level: debug until HH:MM:SS (in …`, which ends with `suffix`.
fn expiry_clock(line: &str, suffix: &str) -> i64 {
    let clock = line
        .trim_end()
        .strip_prefix("level: debug until ")
        .filter(|rest| rest.ends_with(suffix))
        .and_then(|rest| rest.get(..8))
        .unwrap_or_else(|| panic!("not an override line: {line:?}"));
    clock
        .split(':')
        .map(|part| part.parse::<i64>().unwrap())
        .fold(0, |seconds, part| seconds * 60 + part)
}

fn utc_seconds_of_day() -> i64 {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    i64::try_from(since_epoch.as_secs()).unwrap() % SECONDS_PER_DAY
}

fn circular_distance(first: i64, second: i64) -> i64 {
    let difference = (first - second).rem_euclid(SECONDS_PER_DAY);
    difference.min(SECONDS_PER_DAY - difference)
}
