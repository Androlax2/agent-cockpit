//! Where the daemon's main thread spends its time, by request kind and timer: logged once a minute
//! so an idle-CPU regression shows in the log. A sidebar retrying a refused request 25 times a
//! second kept osd at 30-45% CPU for an hour on 2026-10-27, and nothing said so.

use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

/// Busy time above this share of the minute is logged as a warning, not as information.
pub const BUSY_WARN_PERCENT: f64 = 5.0;
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
        if elapsed.is_zero() { return 0.0; }
        100.0 * self.busy().as_secs_f64() / elapsed.as_secs_f64()
    }

    /// One line: total busy time, then the costliest kinds with their counts.
    pub fn summary(&self, elapsed: Duration) -> String {
        let mut kinds: Vec<_> = self.by_kind.iter().collect();
        kinds.sort_by(|left, right| right.1.1.cmp(&left.1.1).then(left.0.cmp(right.0)));
        let shown: Vec<String> = kinds.iter().take(KINDS_SHOWN)
            .map(|(kind, (count, took))| format!("{kind} {count}x {}ms", took.as_millis())).collect();
        format!("load over {}s: busy {}ms ({:.1}%); {}", elapsed.as_secs(), self.busy().as_millis(),
                self.busy_percent(elapsed), shown.join(", "))
    }

    pub fn clear(&mut self) {
        self.by_kind.clear();
    }
}

/// The kind of a request: a JSON request's "action" or "request" tag, else a line's first word.
/// Sandboxes write these requests, so anything but a short snake_case word counts as "other".
pub fn kind_of(text: &str) -> String {
    let tagged = serde_json::from_str::<Value>(text).ok()
        .and_then(|request| ["action", "request"].iter().find_map(|tag| request.get(*tag).and_then(Value::as_str).map(str::to_string)));
    let kind = tagged.unwrap_or_else(|| text.split_whitespace().next().unwrap_or("").to_string());
    let is_word = !kind.is_empty() && kind.len() <= 32 && kind.bytes().all(|byte| byte.is_ascii_lowercase() || byte == b'_' || byte == b'-');
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
        assert_eq!(meter.summary(Duration::from_secs(60)),
                   "load over 60s: busy 38ms (0.1%); tick 1x 30ms, team:native_telemetry 2x 8ms");
    }

    #[test]
    fn busy_percent_is_the_share_of_the_elapsed_time() {
        let mut meter = LoadMeter::default();
        meter.record("tick", Duration::from_secs(3));
        assert_eq!(meter.busy_percent(Duration::from_secs(60)), 5.0);
    }

    #[test]
    fn a_request_kind_comes_from_its_tag_or_first_word() {
        assert_eq!(kind_of(r#"{"action":"native_telemetry","sequence":1}"#), "native_telemetry");
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
