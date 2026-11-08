//! Claude Code's first-run screens, answered in a sandboxed session's own home before its
//! launch (#1178).
//!
//! Measured on 2.1.319, whose interactive start shows these in order: its onboarding (theme,
//! then sign-in) while the global config lacks `hasCompletedOnboarding: true`; the folder
//! trust ([`crate::trust`]); then "Do you want to use this API key?" while the last 20
//! characters of the trimmed `ANTHROPIC_API_KEY` are in neither
//! `customApiKeyResponses.approved` nor `.rejected`. A session's home starts empty, so every
//! sandboxed session stopped on the onboarding, and one with an API key bound then on the key
//! question too: the key it asks about is the placeholder the session proxy completes.
//!
//! The write takes Claude Code's lock as [`crate::trust::record`] does and keeps every other
//! byte of the file; a yes added beside answers about other keys rewrites that one member.
//! An answer already recorded is kept, a "no" to the key included.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde_json::{Value, json};

use crate::trust::{
    CREATED_MODE, ConfigLock, Recorded, WhenMissing, existing, find, members, quoted, replace,
    skip_whitespace,
};

/// The variable whose value Claude Code asks about.
pub const API_KEY_VARIABLE: &str = "ANTHROPIC_API_KEY";

/// The flag Claude Code sets when its onboarding is done.
const ONBOARDED_KEY: &str = "hasCompletedOnboarding";

/// Where Claude Code keeps the answers to its API key question.
const KEY_RESPONSES: &str = "customApiKeyResponses";

/// How many trailing characters of the trimmed key Claude Code records an answer under.
const KEY_TAIL_LENGTH: usize = 20;

/// Records in `config`, created owner-only when missing, that Claude Code's onboarding is
/// done and, given `api_key`, the value the agent holds in `ANTHROPIC_API_KEY`, a yes to
/// using it unless the key was already answered. Refuses, writing nothing, when the file is
/// a link, cannot be parsed, has an unexpected shape or stays locked.
pub fn record(config: &Path, api_key: Option<&str>) -> Result<Recorded, String> {
    // Checked before the lock too, so a file that can never be written takes no lock.
    existing(config, WhenMissing::Create)?;
    let _lock = ConfigLock::acquire(config)?;
    let (text, mode) = match existing(config, WhenMissing::Create)? {
        Some(metadata) => (
            std::fs::read_to_string(config)
                .map_err(|error| format!("read {}: {error}", config.display()))?,
            metadata.mode() & 0o7777,
        ),
        None => ("{}".to_owned(), CREATED_MODE),
    };
    let approved_tail = api_key.map(key_tail);
    let Some(edited) = answered(&text, approved_tail.as_deref())? else {
        return Ok(Recorded::Unchanged);
    };
    replace(config, mode, &edited)?;
    Ok(Recorded::Written)
}

/// What Claude Code records an answer about `key` under: its last characters, trimmed.
pub fn key_tail(key: &str) -> String {
    let trimmed = key.trim();
    let skipped = trimmed.chars().count().saturating_sub(KEY_TAIL_LENGTH);
    trimmed.chars().skip(skipped).collect()
}

/// `text` with the onboarding done and, given `approved_tail`, the key approved, every other
/// byte kept; `None` when both were already recorded. The edit is checked by parsing the
/// result.
pub fn answered(text: &str, approved_tail: Option<&str>) -> Result<Option<String>, String> {
    let unparsable = |error: serde_json::Error| format!("the config is not JSON: {error}");
    let mut expected: Value = serde_json::from_str(text).map_err(unparsable)?;
    let top = expected
        .as_object_mut()
        .ok_or("the config is not a JSON object")?;
    let mut edited = text.to_owned();
    if top.get(ONBOARDED_KEY) != Some(&Value::Bool(true)) {
        edited = with_member(&edited, ONBOARDED_KEY, "true")?;
        top.insert(ONBOARDED_KEY.to_owned(), Value::Bool(true));
    }
    if let Some(tail) = approved_tail
        && let Some(responses) = with_approved(top.get(KEY_RESPONSES), tail)?
    {
        edited = with_member(&edited, KEY_RESPONSES, &responses.to_string())?;
        top.insert(KEY_RESPONSES.to_owned(), responses);
    }
    if edited == text {
        return Ok(None);
    }
    let written: Value = serde_json::from_str(&edited).map_err(unparsable)?;
    if written != expected {
        return Err("the edited config does not read back as the intended change".to_owned());
    }
    Ok(Some(edited))
}

/// The answers `responses` with a yes for `tail` added, every other answer kept; `None` when
/// they already hold a yes or a no for it. Claude Code reads a missing or null value as no
/// answers.
fn with_approved(responses: Option<&Value>, tail: &str) -> Result<Option<Value>, String> {
    let mut responses = match responses {
        None | Some(Value::Null) => serde_json::Map::new(),
        Some(Value::Object(responses)) => responses.clone(),
        Some(_) => return Err(format!("the config's `{KEY_RESPONSES}` is not an object")),
    };
    let mut lists = Vec::new();
    for name in ["approved", "rejected"] {
        let answers = match responses.get(name) {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(answers)) => answers.clone(),
            Some(_) => {
                return Err(format!(
                    "the config's `{KEY_RESPONSES}.{name}` is not a list"
                ));
            }
        };
        if answers.iter().any(|answer| answer.as_str() == Some(tail)) {
            return Ok(None);
        }
        lists.push((name, answers));
    }
    for (name, mut answers) in lists {
        if name == "approved" {
            answers.push(json!(tail));
        }
        responses.insert(name.to_owned(), Value::Array(answers));
    }
    Ok(Some(Value::Object(responses)))
}

/// `text` with the top-level member `name` holding `value`: its value replaced, or the member
/// inserted first.
fn with_member(text: &str, name: &str, value: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let top = skip_whitespace(bytes, 0);
    let top_members = members(bytes, top)?;
    if let Some(member) = find(&top_members, name)? {
        return Ok(format!(
            "{}{value}{}",
            &text[..member.value],
            &text[member.value_end..]
        ));
    }
    let at = top + 1;
    let separator = if top_members.is_empty() { "" } else { "," };
    Ok(format!(
        "{}{}:{value}{separator}{}",
        &text[..at],
        quoted(name)?,
        &text[at..]
    ))
}
