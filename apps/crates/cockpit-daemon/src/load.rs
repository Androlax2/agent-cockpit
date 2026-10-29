//! Where the daemon's main thread spends its time, by request kind and timer: logged once a minute
//! so an idle-CPU regression shows in the log. A sidebar retrying a refused request 25 times a
//! second kept osd at 30-45% CPU for an hour on 2026-10-27, and nothing said so.

use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

thread_local! {
    /// The commands this thread ran and waited for, by program: their CPU is the daemon's children's.
    static SPAWNS: RefCell<LoadMeter> = RefCell::default();
}

pub fn record_spawn(program: &str, took: Duration) {
    let name = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("other");
    SPAWNS.with(|spawns| spawns.borrow_mut().record(name, took));
}

/// The commands run since the last call, as "name Nx Mms" parts, costliest first; then forgotten.
pub fn take_spawns() -> String {
    SPAWNS.with(|spawns| {
        let mut spawns = spawns.borrow_mut();
        let listed = spawns.listed();
        spawns.clear();
        listed
    })
}

/// Busy time above this share of the minute is logged as a warning, not as information.
pub const BUSY_WARN_PERCENT: f64 = 5.0;
/// Above this share Theo sees a banner: about 8-16% is normal with ten busy agents, the sidebar
/// retry storm of 2026-10-27 was far above it.
pub const BUSY_ALERT_PERCENT: f64 = 25.0;
pub const ALERT_EVERY: Duration = Duration::from_secs(3600);

/// Whether a minute this busy deserves a banner, given when the last one was shown.
pub fn should_alert(busy_percent: f64, since_last_alert: Option<Duration>) -> bool {
    busy_percent > BUSY_ALERT_PERCENT
        && since_last_alert.is_none_or(|elapsed| elapsed >= ALERT_EVERY)
}
const KINDS_SHOWN: usize = 6;

#[derive(Default)]
pub struct LoadMeter {
    by_kind: BTreeMap<String, (u64, Duration)>,
}

impl LoadMeter {
    pub fn record(&mut self, kind: &str, took: Duration) {
        let entry = self.by_kind.entry(kind.to_string()).or_default();
        entry.0 += 1;
        entry.1 += took;
    }

    pub fn busy(&self) -> Duration {
        self.by_kind.values().map(|(_, took)| *took).sum()
    }

    /// The share of `elapsed` spent busy, in percent.
    pub fn busy_percent(&self, elapsed: Duration) -> f64 {
        if elapsed.is_zero() {
            return 0.0;
        }
        100.0 * self.busy().as_secs_f64() / elapsed.as_secs_f64()
    }

    /// One line: total busy time, then the costliest kinds with their counts.
    pub fn summary(&self, elapsed: Duration) -> String {
        format!(
            "load over {}s: busy {}ms ({:.1}%); {}",
            elapsed.as_secs(),
            self.busy().as_millis(),
            self.busy_percent(elapsed),
            self.listed()
        )
    }

    fn listed(&self) -> String {
        let mut kinds: Vec<_> = self.by_kind.iter().collect();
        kinds.sort_by(|left, right| right.1.1.cmp(&left.1.1).then(left.0.cmp(right.0)));
        kinds
            .iter()
            .take(KINDS_SHOWN)
            .map(|(kind, (count, took))| format!("{kind} {count}x {}ms", took.as_millis()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn clear(&mut self) {
        self.by_kind.clear();
    }
}

/// The kind of a request: a JSON request's "action" or "request" tag, else a line's first word.
/// Sandboxes write these requests, so anything but a short snake_case word counts as "other".
pub fn kind_of(text: &str) -> String {
    let tagged = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|request| {
            ["action", "request"].iter().find_map(|tag| {
                request
                    .get(*tag)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
        });
    let kind = tagged.unwrap_or_else(|| text.split_whitespace().next().unwrap_or("").to_string());
    let is_word = !kind.is_empty()
        && kind.len() <= 32
        && kind
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_' || byte == b'-');
    if is_word { kind } else { "other".into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_lists_the_costliest_kinds_first_with_their_counts() {
        let mut meter = LoadMeter::default();
        meter.record("team:native_telemetry", Duration::from_millis(4));
        meter.record("team:native_telemetry", Duration::from_millis(4));
        meter.record("tick", Duration::from_millis(30));
        assert_eq!(
            meter.summary(Duration::from_secs(60)),
            "load over 60s: busy 38ms (0.1%); tick 1x 30ms, team:native_telemetry 2x 8ms"
        );
    }

    #[test]
    fn busy_percent_is_the_share_of_the_elapsed_time() {
        let mut meter = LoadMeter::default();
        meter.record("tick", Duration::from_secs(3));
        assert_eq!(meter.busy_percent(Duration::from_secs(60)), 5.0);
    }

    #[test]
    fn a_minute_over_the_alert_share_raises_a_banner_at_most_hourly() {
        assert!(should_alert(30.0, None));
        assert!(!should_alert(20.0, None), "normal load with many agents");
        assert!(
            !should_alert(30.0, Some(Duration::from_secs(600))),
            "shown ten minutes ago"
        );
        assert!(should_alert(30.0, Some(ALERT_EVERY)));
    }

    #[test]
    fn spawns_are_listed_by_program_name_and_forgotten_once_taken() {
        record_spawn("/usr/bin/pkill", Duration::from_millis(6));
        record_spawn("pkill", Duration::from_millis(6));
        record_spawn("hyprctl", Duration::from_millis(3));
        assert_eq!(take_spawns(), "pkill 2x 12ms, hyprctl 1x 3ms");
        assert_eq!(take_spawns(), "");
    }

    #[test]
    fn a_request_kind_comes_from_its_tag_or_first_word() {
        assert_eq!(
            kind_of(r#"{"action":"native_telemetry","sequence":1}"#),
            "native_telemetry"
        );
        assert_eq!(kind_of(r#"{"request":"reserve_task"}"#), "reserve_task");
        assert_eq!(kind_of("session {\"session\":\"x\"}"), "session");
    }

    #[test]
    fn a_request_kind_a_sandbox_made_up_is_other() {
        assert_eq!(kind_of(r#"{"action":"Evil\nlog line"}"#), "other");
        assert_eq!(kind_of(&"a".repeat(33)), "other");
        assert_eq!(kind_of(""), "other");
    }
}
