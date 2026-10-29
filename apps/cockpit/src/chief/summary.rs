//! Shared escaped digest labels and snapshot-only Chief status.

use crate::approval::escape_hiding;
use serde_json::Value;

/// Status for a fresh host snapshot, without constructing a terminal view.
pub fn status(snapshot: &Value) -> String {
    status_with_startup_phase(snapshot, startup_phase(snapshot))
}

pub(super) fn startup_phase(snapshot: &Value) -> &str {
    match snapshot["startup"]["state"].as_str() {
        None if snapshot["startup"].is_null() => "off",
        Some("ended") => "off",
        Some("starting") => "starting",
        Some("failed") if snapshot["startup"]["can_retry"] == true => "failed",
        _ => "checking",
    }
}

pub(super) fn unavailable_reason(snapshot: &Value, chief: &Value) -> Option<String> {
    snapshot["records"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|record| record["kind"] == "worker")
        .find(|worker| worker["session"] == chief["session"])
        .and_then(|worker| worker["blocked_by"].as_array())
        .and_then(|blockers| {
            blockers.iter().find(|blocker| {
                matches!(
                    blocker.as_str(),
                    Some(
                        "native_admission_unverified_for_adapter"
                            | "live_queued_admission_verification_required"
                            | "fresh_native_availability_required"
                            | "runtime_native_sample_unavailable"
                            | "unavailable"
                    )
                )
            })
        })
        .map(text)
}

pub(super) fn status_with_startup_phase(snapshot: &Value, phase: &str) -> String {
    let chiefs: Vec<_> = snapshot["records"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|record| record["kind"] == "chief" && record["state"] != "disabled")
        .collect();
    match chiefs.as_slice() {
        [] if snapshot.get("startup").is_some() => match phase {
            "off" => "Off",
            "starting" => "Starting",
            "failed" => "Could not start",
            _ => "Checking",
        }
        .into(),
        [] => "Off".into(),
        [chief] if !host_flag(&chief["live_verified"]) => {
            format!("Unavailable · retained {} grant", text(&chief["state"]))
        }
        [chief] if unavailable_reason(snapshot, chief).is_some() => format!(
            "Unavailable · retained {} grant · {}",
            text(&chief["state"]),
            unavailable_reason(snapshot, chief).expect("reason checked")
        ),
        [chief] if chief["state"] == "paused" => "Paused · workers continue".into(),
        [chief] if chief["state"] == "active" => "Active".into(),
        _ => "Unknown chief state — host review required".into(),
    }
}

/// A host-supplied string cut to one escaped line: agent-chosen text can hold newlines, terminal codes and any length.
pub(crate) fn one_line(value: &str, limit: usize) -> String {
    let escaped = escape_hiding(
        &value.split_whitespace().collect::<Vec<_>>().join(" "),
        "",
        "",
    );
    if escaped.chars().count() <= limit {
        return escaped;
    }
    let mut kept: String = escaped.chars().take(limit.saturating_sub(1)).collect();
    kept.push('…');
    kept
}

/// `ses_…` and the last 6 characters of a session id, for a session without a name.
pub fn fallback_session_name(session_id: &str) -> String {
    let tail: String = session_id
        .chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("ses_…{}", one_line(&tail, 6))
}

/// `<session name> · <question> → <answer>` on one escaped line of at most `width` characters, the question and the name cut first so the answer stays readable.
pub fn answer_summary_line(
    session_name: &str,
    question: &str,
    answer: &str,
    width: usize,
) -> String {
    one_line(
        &format!(
            "{} · {} → {}",
            one_line(session_name, NAME_LIMIT.min(30)),
            one_line(question, 40),
            one_line(answer, LINE_LIMIT)
        ),
        width,
    )
}

/// `<habit> · <project name | everywhere>` on one escaped line of at most `width` characters, the habit cut so its scope stays readable.
pub fn habit_summary_line(text: &str, project: Option<&str>, width: usize) -> String {
    let scope = one_line(
        &scope_name(&project.map_or(Value::Null, |project| Value::String(project.into()))),
        30,
    );
    one_line(
        &format!(
            "{} · {scope}",
            one_line(text, width.saturating_sub(scope.chars().count() + 3))
        ),
        width,
    )
}

/// When a "since" time started: `HH:MM` on `today`, `yesterday HH:MM` the day before, otherwise `Wed 14 Oct 09:05`.
pub fn since_label(since_ms: i64, today: chrono::NaiveDate) -> String {
    let Some(moment) = chrono::DateTime::from_timestamp_millis(since_ms)
        .map(|moment| moment.with_timezone(&chrono::Local))
    else {
        return "an unknown time".into();
    };
    let day = moment.date_naive();
    if day == today {
        moment.format("%H:%M").to_string()
    } else if today.pred_opt() == Some(day) {
        moment.format("yesterday %H:%M").to_string()
    } else {
        moment.format("%a %d %b %H:%M").to_string()
    }
}

/// A project key as Theo reads it (its last path part), or `everywhere` for none.
pub(super) fn scope_name(project: &Value) -> String {
    match project.as_str() {
        Some(project) => crate::teams::summary::short_project(project),
        None => "everywhere".into(),
    }
}

pub(super) fn host_flag(value: &Value) -> bool {
    value == true || value == 1
}

pub(super) fn text(value: &Value) -> String {
    value
        .as_str()
        .map(|value| escape_hiding(value, "", ""))
        .unwrap_or_else(|| shown(value))
}

pub fn shown(value: &Value) -> String {
    escape_hiding(
        &serde_json::to_string_pretty(value).expect("JSON value serializes"),
        "",
        "",
    )
}

pub const DEFAULT_LOOKBACK_MS: i64 = 24 * 3_600_000;

pub const DIGEST_LINES: usize = crate::chief::store::DIGEST_ANSWER_LINES;

pub const DIGEST_LINE_WIDTH: usize = 100;

pub(super) const LINE_LIMIT: usize = 300;

pub(super) const NAME_LIMIT: usize = 80;
