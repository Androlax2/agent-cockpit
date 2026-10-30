//! Protected Chief contract. Wire types do not confer authorization or readiness.

pub mod bundle;
pub mod store;
pub mod summary;
pub use store::reconcile_todos;

use crate::teams::{ErrorCode, TeamError};
pub use cockpit_protocol::chief::*;
use serde_json::Value;

/// A complete host review, never a mixture of pagination generations.
pub fn host_snapshot() -> Result<Value, TeamError> {
    use crate::teams::{HostRequest, client::host_request};
    for _ in 0..3 {
        let read_at_ms = chrono::Utc::now().timestamp_millis();
        let mut records = Vec::new();
        let (mut after, mut generation, mut envelope) = (0, None, None);
        let mut session_names = serde_json::Map::new();
        for _ in 0..512 {
            let page = host_request(&HostRequest::ChiefSnapshot {
                after,
                limit: 50,
                generation: generation.clone(),
            })?
            .result?;
            if page["invalidated"] == true {
                break;
            }
            if page["projection"] != "host_review" || page["cursor_kind"] != "snapshot_offset" {
                return Err(TeamError::new(
                    ErrorCode::Unavailable,
                    "Chief host review projection is missing",
                ));
            }
            let current = page["generation"]
                .as_str()
                .ok_or_else(|| {
                    TeamError::new(
                        ErrorCode::Unavailable,
                        "Chief snapshot generation is missing",
                    )
                })?
                .to_string();
            if generation
                .as_ref()
                .is_some_and(|generation| generation != &current)
            {
                break;
            }
            generation = Some(current);
            let batch = page["records"].as_array().ok_or_else(|| {
                TeamError::new(ErrorCode::Unavailable, "Chief snapshot records are missing")
            })?;
            records.extend(batch.iter().cloned());
            if envelope.is_none() {
                envelope = Some(page.clone());
            }
            if let Some(names) = page["session_names"].as_object() {
                session_names.extend(names.clone());
            }
            if page["has_more"] == false {
                let mut complete = envelope.expect("first page is present");
                complete["records"] = serde_json::json!(records);
                complete["has_more"] = serde_json::json!(false);
                complete["session_names"] = serde_json::json!(session_names);
                complete["read_at_ms"] = serde_json::json!(read_at_ms);
                return Ok(complete);
            }
            after = page["next"]
                .as_i64()
                .filter(|next| *next > after)
                .ok_or_else(|| {
                    TeamError::new(
                        ErrorCode::Unavailable,
                        "Chief snapshot cursor did not advance",
                    )
                })?;
        }
    }
    Err(TeamError::new(
        ErrorCode::Unavailable,
        "Chief host snapshot changed repeatedly or exceeded its bounded read; try opening again",
    ))
}

/// When Theo last saw his habits, on the Habits page or in a digest (epoch ms); a missing or unreadable note, or one from the future, counts as never.
pub fn habits_seen_ms() -> Option<i64> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    std::fs::read_to_string(crate::paths::chief_habits_seen_at())
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .filter(|seen_ms| (1..=now_ms).contains(seen_ms))
}

/// Saves when Theo saw his habits, never earlier than the time already saved: the page and the digest each read them at their own time.
pub fn save_habits_seen(seen_ms: i64) -> std::io::Result<()> {
    if habits_seen_ms().is_some_and(|saved_ms| saved_ms >= seen_ms) {
        return Ok(());
    }
    let note = crate::paths::chief_habits_seen_at();
    if let Some(folder) = note.parent() {
        std::fs::create_dir_all(folder)?;
    }
    std::fs::write(note, seen_ms.to_string())
}
