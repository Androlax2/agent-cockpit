//! What the owner reads before deciding (report 17, display rules): the exact text with every
//! character that could hide what it does shown escaped, runs of blank lines collapsed into
//! one marked line, characters outside ASCII named, and a working directory outside the
//! asking session's project flagged.

use std::path::Path;

/// Text as the owner is shown it, and what in it deserves a second look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The text with hiding characters as `\u{…}` and blank-line runs as `[N blank lines]`.
    pub text: String,
    /// Whether `text` differs from the original: something was escaped or collapsed.
    pub has_hidden_characters: bool,
    /// Each distinct visible non-ASCII character as `<char> U+XXXX`, sorted. Named, not
    /// escaped: lookalike letters (a Cyrillic `а` for an `a`) make a command read as another,
    /// but accented text in a command is legitimate.
    pub non_ascii: Vec<String>,
}

/// `text` as the owner reads it.
pub fn show(text: &str) -> Shown {
    let escaped: String = text
        .chars()
        .map(|character| {
            if is_hiding(character) {
                format!("\\u{{{:04x}}}", u32::from(character))
            } else {
                character.to_string()
            }
        })
        .collect();
    let shown = collapse_blank_runs(&escaped);
    let mut non_ascii: Vec<String> = text
        .chars()
        .filter(|character| !character.is_ascii() && !is_hiding(*character))
        .map(|character| format!("{character} U+{:04X}", u32::from(character)))
        .collect();
    non_ascii.sort();
    non_ascii.dedup();
    Shown {
        has_hidden_characters: shown != text,
        text: shown,
        non_ascii,
    }
}

/// Whether `cwd` lies outside `project_root`; with no known project it always does. Both are
/// canonical paths, so a component-wise prefix test is exact.
pub fn is_outside_project(cwd: &Path, project_root: Option<&Path>) -> bool {
    project_root.is_none_or(|root| !cwd.starts_with(root))
}

/// Characters that can hide what a command does on screen: control codes other than the
/// newline (escape sequences rewrite lines, a carriage return overwrites one, a tab passes for
/// spaces), and invisible or direction-changing Unicode. The set is the one the current
/// approval terminal escapes (`apps/crates/cockpit-core/src/approval.rs`).
fn is_hiding(character: char) -> bool {
    matches!(
        u32::from(character),
        0x00..=0x09
            | 0x0b..=0x1f
            | 0x7f..=0x9f
            | 0xad
            | 0x34f
            | 0x115f
            | 0x1160
            | 0x180e
            | 0x200b..=0x200f
            | 0x2028..=0x202e
            | 0x2060..=0x2069
            | 0x2800
            | 0x3164
            | 0xfe00..=0xfe0f
            | 0xfeff
            | 0xffa0
            | 0xe0000..=0xe007f
    )
}

/// Blank lines in a row are where a payload hides: enough of them scroll the start of a
/// command out of view. A run of two or more shows as one marker line.
fn collapse_blank_runs(text: &str) -> String {
    let mut shown: Vec<String> = Vec::new();
    let mut blank_run: Vec<&str> = Vec::new();
    // `None` ends the text, so a trailing run is flushed like any other.
    for line in text.split('\n').map(Some).chain([None]) {
        if let Some(line) = line
            && line.trim().is_empty()
        {
            blank_run.push(line);
            continue;
        }
        if blank_run.len() > 1 {
            shown.push(format!("[{} blank lines]", blank_run.len()));
        } else {
            shown.extend(blank_run.iter().map(|blank| (*blank).to_owned()));
        }
        blank_run.clear();
        shown.extend(line.map(str::to_owned));
    }
    shown.join("\n")
}
