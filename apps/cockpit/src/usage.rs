//! The plan's usage, as claude.ai's usage page shows it, from the cache claude-usage.sh keeps;
//! the full report for the session pane; and the 80 % / 95 % budget banners.

use crate::paths;
use crate::util::{self, now, parse_time, read_json};
use chrono::{Duration, Local, Utc};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::process::{Command, Stdio};

pub const USAGE_REFRESH_S: f64 = 300.0;
pub const USAGE_BAR_MIN: usize = 8;
pub const USAGE_BAR_MAX: usize = 40;
const REPORT_BAR_WIDTH: usize = 40;
const ALERT_THRESHOLDS: [i64; 2] = [80, 95];
/// The flat windows: the older shape of the response, used only without its "limits" list.
const WINDOWS: [(&str, &str); 4] = [("five_hour", "Current session"), ("seven_day", "This week"),
                                    ("seven_day_opus", "Opus this week"), ("seven_day_sonnet", "Sonnet this week")];

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub key: String,
    pub label: String,
    pub percentage: Option<i64>,
    pub resets: String,
    pub resets_at: Option<String>,
    pub note: String,
}

pub fn cache() -> Value {
    match read_json(&paths::usage_cache()) {
        Some(value @ Value::Object(_)) => value,
        _ => json!({}),
    }
}

fn rounded(value: Option<&Value>) -> Option<i64> {
    value.and_then(Value::as_f64).map(|number| number.round() as i64)
}

/// Every limit in the cache, as the usage page lists them.
pub fn windows() -> Vec<Window> {
    windows_of(&cache())
}

pub fn windows_of(usage: &Value) -> Vec<Window> {
    if let Some(limits) = usage.get("limits").and_then(Value::as_array) {
        return limits.iter().filter(|limit| limit.is_object()).map(limit_window).collect();
    }
    let mut rows: Vec<(String, String, &Value)> = Vec::new();
    for (key, label) in WINDOWS {
        if let Some(window) = usage.get(key) {
            rows.push((key.into(), label.into(), window));
        }
    }
    if let Some(object) = usage.as_object() {
        let mut others: Vec<_> = object
            .iter()
            .filter(|(key, window)| !WINDOWS.iter().any(|(known, _)| known == key) && window.get("utilization").is_some())
            .collect();
        others.sort_by(|a, b| a.0.cmp(b.0));
        for (key, window) in others {
            rows.push((key.clone(), label_of(key), window));
        }
    }
    if let Some(scoped) = usage.get("model_scoped").and_then(Value::as_array) {
        for (index, window) in scoped.iter().enumerate() {
            let name = window.get("display_name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or("Model");
            rows.push((format!("model-{index}"), format!("{name} this week"), window));
        }
    }
    rows.into_iter()
        .filter(|(_, _, window)| window.is_object())
        .map(|(key, label, window)| {
            let resets_at = window.get("resets_at").and_then(Value::as_str).map(str::to_string);
            Window { key, label, percentage: rounded(window.get("utilization")), resets: reset_time(resets_at.as_deref()),
                     resets_at, note: String::new() }
        })
        .collect()
}

/// One entry of the response's limits list. A scoped weekly limit names its model or surface.
fn limit_window(limit: &Value) -> Window {
    let kind = limit.get("kind").and_then(Value::as_str).unwrap_or("");
    let scope = limit.get("scope");
    let scoped_name = ["model", "surface"]
        .iter()
        .filter_map(|part| scope.and_then(|scope| scope.get(part)).and_then(|part| part.get("display_name")).and_then(Value::as_str))
        .find(|name| !name.is_empty())
        .unwrap_or("");
    let (label, note) = match kind {
        "session" => ("Current session".to_string(), String::new()),
        "weekly_all" => ("This week".to_string(), String::new()),
        _ if !scoped_name.is_empty() => (format!("{scoped_name} this week"), format!("separate weekly limit for {scoped_name}")),
        _ => {
            let words = kind.replace('_', " ");
            (if words.is_empty() { "Limit".into() } else { capitalize(&words) }, String::new())
        }
    };
    let resets_at = limit.get("resets_at").and_then(Value::as_str).map(str::to_string);
    Window {
        key: if scoped_name.is_empty() { kind.to_string() } else { format!("{kind}:{scoped_name}") },
        label, percentage: rounded(limit.get("percent")), resets: reset_time(resets_at.as_deref()), resets_at, note,
    }
}

fn capitalize(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut characters = lower.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// "seven_day_fable" -> "Fable this week"; anything else keeps its words.
fn label_of(key: &str) -> String {
    match key.strip_prefix("seven_day_") {
        Some(rest) => capitalize(&rest.replace('_', " ")) + " this week",
        None => capitalize(&key.replace('_', " ")),
    }
}

/// "15:00" when the window resets within a day, else "Mon 09:00"; empty when unknown.
pub fn reset_time(stamp: Option<&str>) -> String {
    let Some(moment) = stamp.and_then(parse_time) else { return String::new() };
    let local = moment.with_timezone(&Local);
    if moment - Utc::now() < Duration::days(1) { local.format("%H:%M").to_string() } else { local.format("%a %H:%M").to_string() }
}

/// "2d 3h", "2h 13m" or "7m" until the window resets; "now" when past; empty when unknown.
pub fn time_left(window: &Window) -> String {
    let Some(moment) = window.resets_at.as_deref().and_then(parse_time) else { return String::new() };
    let remaining = (moment - Utc::now()).num_seconds();
    if remaining <= 0 {
        return "now".into();
    }
    let (hours, minutes) = (remaining / 60 / 60, remaining / 60 % 60);
    if hours >= 48 {
        format!("{}d {}h", hours / 24, hours % 24)
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

/// The Status panel's right column: "in 2h 13m", or the clock time when only that is known.
pub fn resets_short(window: &Window) -> String {
    match time_left(window).as_str() {
        "" if !window.resets.is_empty() => format!("resets {}", window.resets),
        "" => String::new(),
        "now" => "resets now".into(),
        left => format!("in {left}"),
    }
}

/// The report's form: "resets 15:39  (in 2h 13m)".
pub fn resets_in(window: &Window) -> String {
    if window.resets.is_empty() {
        return String::new();
    }
    match time_left(window).as_str() {
        "now" => "resetting".into(),
        "" => format!("resets {}", window.resets),
        left => format!("resets {}  (in {left})", window.resets),
    }
}

pub fn plan() -> String {
    read_json(&paths::credentials())
        .and_then(|credentials| credentials.get("claudeAiOauth")?.get("subscriptionType")?.as_str().map(capitalize))
        .unwrap_or_default()
}

pub fn bar(percentage: Option<i64>, width: usize) -> String {
    match percentage {
        None => "·".repeat(width),
        Some(percentage) => {
            let filled = ((percentage as f64 / 100.0 * width as f64).round() as usize).min(width);
            "█".repeat(filled) + &"░".repeat(width - filled)
        }
    }
}

/// Where this week's usage went (Claude Code, Chats, Cowork …), non-zero rows only.
fn breakdown(usage: &Value) -> Vec<(String, i64)> {
    usage.pointer("/seven_day_breakdown/rows").and_then(Value::as_array).into_iter().flatten()
        .filter_map(|row| {
            let percent = row.get("percent")?.as_f64()?;
            let name = row.get("display_name").or_else(|| row.get("key")).and_then(Value::as_str).unwrap_or("?");
            (percent > 0.0).then(|| (name.to_string(), percent.round() as i64))
        })
        .collect()
}

/// "€0 of €170 usage credits" from the spend block, empty when the response has none.
fn credits(usage: &Value) -> String {
    let spend = usage.get("spend").cloned().unwrap_or(json!({}));
    let money = |amount: &Value| -> Option<String> {
        let minor = amount.get("amount_minor")?.as_f64()?;
        let exponent = amount.get("exponent").and_then(Value::as_i64).unwrap_or(2);
        let value = minor / 10f64.powi(exponent as i32);
        let currency = amount.get("currency").and_then(Value::as_str).unwrap_or("");
        let symbol = match currency { "EUR" => "€".to_string(), "USD" => "$".into(), "GBP" => "£".into(), other => format!("{other} ") };
        Some(if value.fract() == 0.0 { format!("{symbol}{value:.0}") } else { format!("{symbol}{value:.2}") })
    };
    let Some(used) = spend.get("used").and_then(|used| money(used)) else { return String::new() };
    let mut text = match spend.get("limit").and_then(|limit| money(limit)) {
        Some(limit) => format!("{used} of {limit} usage credits"),
        None => format!("{used} usage credits"),
    };
    if !spend.get("enabled").and_then(Value::as_bool).unwrap_or(false) {
        text += "  (off)";
    }
    text
}

/// The usage page as text with ANSI colours, for the session pane (claude-sessions usage).
pub fn report_lines(columns: usize) -> Vec<String> {
    let (bold, dim, reset) = ("\x1b[1m", "\x1b[2m", "\x1b[0m");
    let colour = |percentage: Option<i64>| match percentage {
        None => "\x1b[2m",
        Some(p) if p >= 90 => "\x1b[31m",
        Some(p) if p >= 70 => "\x1b[33m",
        Some(_) => "\x1b[35m",
    };
    let width = columns.saturating_sub(30).clamp(10, REPORT_BAR_WIDTH);
    let plan = plan();
    let mut lines = vec![String::new(), format!("  {bold}\x1b[34mClaude usage{reset}  {dim}{}{reset}", if plan.is_empty() { "plan unknown" } else { &plan }), String::new()];
    let usage = cache();
    let windows = windows_of(&usage);
    if windows.is_empty() {
        lines.push(format!("  {dim}No usage fetched yet. r in the Status panel fetches it.{reset}"));
    }
    for window in &windows {
        let tint = colour(window.percentage);
        let filled = window.percentage.map(|p| ((p as f64 / 100.0 * width as f64).round() as usize).min(width)).unwrap_or(0);
        let bar = format!("{tint}{}{dim}{}{reset}", "█".repeat(filled), "░".repeat(width - filled));
        let value = window.percentage.map(|p| format!("{p:3}%")).unwrap_or_else(|| "   –".into());
        let note = if window.note.is_empty() { String::new() } else { format!("  {dim}{}{reset}", window.note) };
        lines.push(format!("  {bold}{}{reset}{note}", window.label));
        lines.push(format!("  {bar}  {tint}{bold}{value}{reset}  {dim}{}{reset}", resets_in(window)));
        lines.push(String::new());
    }
    let parts = breakdown(&usage);
    if !parts.is_empty() {
        lines.push(format!("  {bold}This week by product{reset}"));
        lines.push("  ".to_string() + &parts.iter().map(|(name, percent)| format!("{name} {dim}{percent}%{reset}")).collect::<Vec<_>>().join("   "));
        lines.push(String::new());
    }
    let credits = credits(&usage);
    if !credits.is_empty() {
        lines.push(format!("  {bold}Usage credits{reset}  {dim}{credits}{reset}"));
        lines.push(String::new());
    }
    if let Ok(modified) = fs::metadata(paths::usage_cache()).and_then(|m| m.modified()) {
        let fetched = modified.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        lines.push(format!("  {dim}Fetched {} ago, refreshed every {} min. Pick a session to replace this.{reset}",
                           util::age(fetched), USAGE_REFRESH_S as i64 / 60));
    }
    lines
}

/// claude-sessions usage: redraws the report until the pane is replaced.
pub fn report_forever() -> ! {
    loop {
        let columns = crossterm::terminal::size().map(|(columns, _)| columns as usize).unwrap_or(80);
        print!("\x1b[2J\x1b[H{}\n", report_lines(columns).join("\n"));
        use std::io::Write;
        let _ = std::io::stdout().flush();
        std::thread::sleep(std::time::Duration::from_secs(30));
    }
}

/// Runs claude-usage.sh every 5 minutes so the cache stays fresh.
pub struct Refresher {
    last_run: f64,
}

impl Refresher {
    pub fn new() -> Self {
        Self { last_run: 0.0 }
    }

    pub fn tick(&mut self, force: bool) {
        if !force && now() - self.last_run < USAGE_REFRESH_S {
            return;
        }
        self.last_run = now();
        if force {
            let _ = fs::remove_file(paths::usage_cache()); // the script trusts a cache younger than 4 min
        }
        let script = paths::usage_script();
        if script.exists() {
            let _ = Command::new(script).arg("five_hour").stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        }
    }
}

/// A banner once per limit, threshold and window when a usage limit passes 80 % and 95 %,
/// remembered in alerts.json so a restarted column does not repeat itself.
pub struct BudgetAlerts {
    fired: BTreeSet<String>,
    send: fn(&Value) -> bool,
}

impl BudgetAlerts {
    pub fn new(send: fn(&Value) -> bool) -> Self {
        let fired = read_json(&paths::alerts())
            .and_then(|saved| saved.as_array().map(|keys| keys.iter().filter_map(|key| key.as_str().map(str::to_string)).collect()))
            .unwrap_or_default();
        Self { fired, send }
    }

    pub fn check(&mut self, windows: &[Window]) {
        let mut live = BTreeSet::new();
        let mut changed = false;
        for window in windows {
            for threshold in ALERT_THRESHOLDS {
                let key = format!("{}:{threshold}:{}", window.key, window.resets_at.as_deref().unwrap_or(""));
                live.insert(key.clone());
                let Some(percentage) = window.percentage else { continue };
                if percentage < threshold || self.fired.contains(&key) {
                    continue;
                }
                let resets = resets_in(window);
                let message = if resets.is_empty() { "the plan's limit is close".to_string() } else { capitalize_first(&resets) };
                let notice = json!({"id": format!("claude-usage-{threshold}"), "title": format!("Claude · {} at {percentage}%", window.label), "message": message});
                if (self.send)(&notice) {
                    util::log_event("INFO", &format!("budget alert: {} at {percentage}% (threshold {threshold}%)", window.label));
                    self.fired.insert(key);
                    changed = true;
                }
            }
        }
        if !windows.is_empty() {
            let stale: Vec<_> = self.fired.difference(&live).cloned().collect();
            if !stale.is_empty() {
                for key in stale {
                    self.fired.remove(&key);
                }
                changed = true;
            }
        }
        if changed {
            let _ = util::write_json(&paths::alerts(), &json!(self.fired.iter().collect::<Vec<_>>()));
        }
    }
}

fn capitalize_first(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_windows_fall_back_without_a_limits_list() {
        let usage = json!({"five_hour": {"utilization": 37.4, "resets_at": "2026-10-20T09:00:00Z"}, "seven_day": {"utilization": 72},
                           "seven_day_opus": null, "seven_day_fable": {"utilization": 12}, "model_scoped": [{"display_name": "Sonnet", "utilization": 5}]});
        let labels: Vec<_> = windows_of(&usage).into_iter().map(|w| (w.label, w.percentage)).collect();
        assert_eq!(labels, vec![("Current session".into(), Some(37)), ("This week".into(), Some(72)),
                                ("Fable this week".into(), Some(12)), ("Sonnet this week".into(), Some(5))]);
    }

    #[test]
    fn scoped_limits_name_their_model() {
        let usage = json!({"limits": [{"kind": "session", "percent": 15}, {"kind": "weekly_scoped", "percent": 65, "scope": {"model": {"display_name": "Fable"}}}]});
        let windows = windows_of(&usage);
        assert_eq!(windows[0].label, "Current session");
        assert_eq!((windows[1].label.as_str(), windows[1].key.as_str()), ("Fable this week", "weekly_scoped:Fable"));
    }

    #[test]
    fn the_bar_fills_by_percentage() {
        assert_eq!(bar(Some(50), 12), "██████░░░░░░");
        assert_eq!(bar(None, 12), "············");
    }
}
