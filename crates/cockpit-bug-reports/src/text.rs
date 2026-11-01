//! The limits on what reporters write. Text is refused, never trimmed to fit, so what the
//! owner reviews is what was sent.

use crate::Refusal;
use crate::error::invalid;

pub const TITLE_MAX_CHARS: usize = 120;
pub const DESCRIPTION_MAX_CHARS: usize = 4000;
pub const STEPS_MAX_CHARS: usize = 4000;
pub const COMPONENT_MAX_CHARS: usize = 80;
/// What a draft's description may grow to as repeats are merged into it; past it, a repeat
/// only counts.
pub(crate) const MERGED_DESCRIPTION_MAX_CHARS: usize = 8000;
/// How much of the description's first line a title made from it keeps.
const DERIVED_TITLE_CHARS: usize = 100;

/// One line, not blank, without control characters.
pub(crate) fn title(text: &str) -> Result<String, Refusal> {
    if text.trim().is_empty() {
        return Err(invalid("title", "is empty"));
    }
    if text.chars().any(char::is_control) {
        return Err(invalid(
            "title",
            "must be one line without control characters",
        ));
    }
    within("title", text, TITLE_MAX_CHARS)?;
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
    Ok(text.trim().to_owned())
}

/// A [`body`] that must say something.
pub(crate) fn required_body(
    field: &'static str,
    text: &str,
    max_chars: usize,
) -> Result<String, Refusal> {
    let text = body(field, text, max_chars)?;
    if text.is_empty() {
        return Err(invalid(field, "is empty"));
    }
    Ok(text)
}

/// A plain name: lowercase ASCII letters, digits and `-_./:`.
pub(crate) fn component(text: &str) -> Result<String, Refusal> {
    if text.is_empty() {
        return Err(invalid("component", "is empty"));
    }
    within("component", text, COMPONENT_MAX_CHARS)?;
    let is_plain = text.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '-' | '_' | '.' | '/' | ':')
    });
    if !is_plain {
        return Err(invalid(
            "component",
            "may hold lowercase letters, digits and - _ . / : only",
        ));
    }
    Ok(text.to_owned())
}

/// The description's first line that says something, cut to a title's length.
pub(crate) fn derived_title(description: &str) -> String {
    let first_line = description
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Cockpit bug");
    let mut title: String = first_line
        .chars()
        .filter(|character| !character.is_control())
        .take(DERIVED_TITLE_CHARS)
        .collect();
    if first_line.chars().count() > DERIVED_TITLE_CHARS {
        title.push('…');
    }
    title
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
