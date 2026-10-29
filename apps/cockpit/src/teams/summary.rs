//! Shared plain project labels.

use crate::approval::escape_hiding;
use serde_json::Value;

pub(crate) fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => escape_hiding(text, "", ""),
        Value::Null => String::new(),
        other => escape_hiding(&other.to_string(), "", ""),
    }
}

/// "remote:git@github.com:example/project-a.git" → "project-a".
pub(crate) fn short_project(canonical: &str) -> String {
    let tail = canonical.rsplit(['/', ':']).next().unwrap_or(canonical);
    shown(&Value::String(tail.trim_end_matches(".git").to_string()))
}
