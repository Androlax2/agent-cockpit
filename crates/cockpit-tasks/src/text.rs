//! The limits on what filers write. Text is refused, never trimmed to fit, so what the owner
//! reads is what was sent.

use crate::Refusal;
use crate::error::invalid;

pub const TITLE_MAX_CHARS: usize = 200;
pub const NOTES_MAX_CHARS: usize = 4000;
pub const WHY_MAX_CHARS: usize = 600;
pub const SUMMARY_MAX_CHARS: usize = 1000;

/// One line, not blank, without control characters.
pub(crate) fn title(field: &'static str, text: &str) -> Result<String, Refusal> {
    if text.trim().is_empty() {
        return Err(invalid(field, "is empty"));
    }
    if text.chars().any(char::is_control) {
        return Err(invalid(
            field,
            "must be one line without control characters",
        ));
    }
    within(field, text, TITLE_MAX_CHARS)?;
    Ok(text.trim().to_owned())
}

/// Lines and tabs allowed, no other control character. May be empty.
pub(crate) fn body(field: &'static str, text: &str, max_chars: usize) -> Result<String, Refusal> {
    if text
        .chars()
        .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(invalid(
            field,
            "may hold line breaks and tabs, no other control characters",
        ));
    }
    within(field, text, max_chars)?;
    Ok(text.trim_end().to_owned())
}

/// A [`body`] that must say something.
pub(crate) fn required_body(
    field: &'static str,
    text: &str,
    max_chars: usize,
) -> Result<String, Refusal> {
    let text = body(field, text, max_chars)?;
    if text.trim().is_empty() {
        return Err(invalid(field, "is empty"));
    }
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

/// What decline memory compares: case and runs of white space do not make a new title.
pub(crate) fn title_key(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
