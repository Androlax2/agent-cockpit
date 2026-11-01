//! The limits on what sessions write into a workflow. Text is refused, never trimmed to fit,
//! so what the next session reads is what was sent.

use crate::error::invalid;
use crate::{Handoff, Refusal};

/// Report 06: a handoff targets about 2 KiB plus referenced evidence. The prose fields
/// together may not exceed it; evidence is referenced, one line per reference, beside it.
pub const HANDOFF_MAX_BYTES: usize = 2048;
pub const EVIDENCE_MAX: usize = 20;
pub const EVIDENCE_MAX_CHARS: usize = 300;
pub const CHECKPOINT_MAX_CHARS: usize = 1000;
pub const BLOCKED_DETAIL_MAX_CHARS: usize = 600;

/// What a handoff field may not be on its own: report 06 calls a handoff that only says
/// "done" incomplete.
const EMPTY_CLAIMS: &[&str] = &["done", "finished", "complete", "completed", "ok", "fixed"];

/// The handoff as it will be stored, or the field at fault.
pub(crate) fn handoff(handoff: Handoff) -> Result<Handoff, Refusal> {
    let changed = claim("changed", &handoff.changed)?;
    let checked = claim("checked", &handoff.checked)?;
    let outcome = claim("outcome", &handoff.outcome)?;
    let needs_human = body("needs_human", &handoff.needs_human)?;
    let bytes = changed.len() + checked.len() + outcome.len() + needs_human.len();
    if bytes > HANDOFF_MAX_BYTES {
        return Err(invalid(
            "handoff",
            format!(
                "has {bytes} bytes of prose; the limit is {HANDOFF_MAX_BYTES}: reference the \
                 evidence instead of pasting it"
            ),
        ));
    }
    if handoff.evidence.len() > EVIDENCE_MAX {
        return Err(invalid(
            "evidence",
            format!(
                "has {} references; the limit is {EVIDENCE_MAX}",
                handoff.evidence.len()
            ),
        ));
    }
    let evidence = handoff
        .evidence
        .iter()
        .map(|reference| line("evidence", reference, EVIDENCE_MAX_CHARS))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Handoff {
        changed,
        checked,
        outcome,
        needs_human,
        evidence,
    })
}

/// A required field that says more than an empty claim of success.
fn claim(field: &'static str, text: &str) -> Result<String, Refusal> {
    let text = body(field, text)?;
    if text.trim().is_empty() {
        return Err(invalid(field, "is empty"));
    }
    let words: String = text
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    if EMPTY_CLAIMS.contains(&words.as_str()) {
        return Err(invalid(
            field,
            format!("{:?} says nothing the next session can check", text.trim()),
        ));
    }
    Ok(text)
}

/// Lines and tabs allowed, no other control character. May be empty.
fn body(field: &'static str, text: &str) -> Result<String, Refusal> {
    if text
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(invalid(
            field,
            "may hold line breaks and tabs, no other control characters",
        ));
    }
    Ok(text.trim_end().to_owned())
}

/// One line, not blank, without control characters.
pub(crate) fn line(field: &'static str, text: &str, max_chars: usize) -> Result<String, Refusal> {
    if text.trim().is_empty() {
        return Err(invalid(field, "is empty"));
    }
    if text.chars().any(char::is_control) {
        return Err(invalid(
            field,
            "must be one line without control characters",
        ));
    }
    within(field, text, max_chars)?;
    Ok(text.trim().to_owned())
}

/// A [`body`] that must say something, within `max_chars`.
pub(crate) fn note(field: &'static str, text: &str, max_chars: usize) -> Result<String, Refusal> {
    let text = body(field, text)?;
    if text.trim().is_empty() {
        return Err(invalid(field, "is empty"));
    }
    within(field, &text, max_chars)?;
    Ok(text)
}

fn within(field: &'static str, text: &str, max_chars: usize) -> Result<(), Refusal> {
    let chars = text.chars().count();
    if chars > max_chars {
        return Err(invalid(
            field,
            format!("has {chars} characters; the limit is {max_chars}"),
        ));
    }
    Ok(())
}
