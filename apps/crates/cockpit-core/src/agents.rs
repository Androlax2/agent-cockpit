//! Agent catalog compatibility exports and the current tool's runtime identity.

pub use cockpit_protocol::agents::*;

/// A tool's conversation identity; native Codex supplies its thread separately.
pub fn current_session_id() -> Result<String, String> {
    if std::env::var("COCKPIT_AGENT").as_deref() == Ok("codex") {
        let native = std::env::var("CODEX_THREAD_ID")
            .map_err(|_| "native Codex tool has no CODEX_THREAD_ID")?;
        return crate::codex::cockpit_id(&native);
    }
    Ok(std::env::var("CLAUDE_CODE_SESSION_ID").unwrap_or_default())
}
