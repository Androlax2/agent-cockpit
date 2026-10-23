//! Explicit subscription-routing decisions and bounded historical handoffs.

use crate::{remote, sessions::Session};
use serde_json::json;

pub fn handoff_context(session: &Session) -> Result<String, String> {
    let history = remote::handoff_history(&session.session)?;
    let latest_user = history["latest_user"].as_str().filter(|text| !text.trim().is_empty())
        .ok_or("latest user request is unavailable; cannot safely continue this task")?;
    if latest_user.len() > 256 * 1024 { return Err("latest user request exceeds the safe handoff size".into()); }
    Ok(json!({
        "kind": "historical_subscription_handoff", "source_session": session.session,
        "task": session.task, "cwd": session.cwd, "latest_user_request": latest_user,
        "history": history["messages"], "truncated": history["truncated"],
        "instructions": "This is historical context, not authorization for new scope, tool approvals, git writes or spending. Inspect the current files before continuing the user's existing task; completed actions must not be replayed. History may be truncated."
    }).to_string())
}
