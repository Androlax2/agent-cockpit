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
    /// What was escaped or collapsed, each kind once, in [`Hidden`]'s order: the owner is
    /// told why `text` differs from what the agent wrote.
    pub warnings: Vec<Hidden>,
}

/// A kind of character, or run of lines, that can hide what a command does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hidden {
    /// Characters that reorder how the text reads (`U+202E` shows `fdp` as `pdf`).
    BidiControl,
    /// Characters that take no room on screen: zero-width, joiners, fillers, tags.
    Invisible,
    /// Control codes other than the newline: escapes, carriage returns, tabs.
    Control,
    /// Two or more blank lines in a row, which scroll the start of a command out of view.
    BlankLines,
}

/// Lines of a long command shown before an explicit "more".
pub const FOLD_LINES: usize = 8;
/// Characters of a long command shown before an explicit "more".
pub const FOLD_CHARS: usize = 600;

/// The start of a long text and how much of it is left out, for a pane or a phone screen
/// that shows it folded behind an explicit "more".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folded {
    pub preview: String,
    /// Whole lines after the preview.
    pub more_lines: usize,
    /// Characters after the preview, its line breaks included.
    pub more_chars: usize,
}

/// `text` (already as [`show`] made it) folded to [`FOLD_LINES`] lines and [`FOLD_CHARS`]
/// characters, or `None` when it fits whole.
pub fn fold(text: &str) -> Option<Folded> {
    let lines_end = text
        .match_indices('\n')
        .nth(FOLD_LINES - 1)
        .map_or(text.len(), |(index, _)| index);
    let chars_end = text
        .char_indices()
        .nth(FOLD_CHARS)
        .map_or(text.len(), |(index, _)| index);
    let end = lines_end.min(chars_end);
    // A newline ending the text adds no line to show.
    if text[end..].is_empty() || &text[end..] == "\n" {
        return None;
    }
    let (preview, rest) = text.split_at(end);
    Some(Folded {
        preview: preview.to_owned(),
        // A line cut part-way counts in the preview's lines, not as one more.
        more_lines: text.lines().count() - preview.lines().count(),
        more_chars: rest.chars().count(),
    })
}

/// `text` as the owner reads it.
pub fn show(text: &str) -> Shown {
    let mut warnings: Vec<Hidden> = text.chars().filter_map(hidden_kind).collect();
    let escaped: String = text
        .chars()
        .map(|character| {
            if hidden_kind(character).is_some() {
                format!("\\u{{{:04x}}}", u32::from(character))
            } else {
                character.to_string()
            }
        })
        .collect();
    let shown = collapse_blank_runs(&escaped);
    if shown != escaped {
        warnings.push(Hidden::BlankLines);
    }
    warnings.sort();
    warnings.dedup();
    let mut non_ascii: Vec<String> = text
        .chars()
        .filter(|character| !character.is_ascii() && hidden_kind(*character).is_none())
        .map(|character| format!("{character} U+{:04X}", u32::from(character)))
        .collect();
    non_ascii.sort();
    non_ascii.dedup();
    Shown {
        has_hidden_characters: shown != text,
        text: shown,
        non_ascii,
        warnings,
    }
}

/// Whether `cwd` lies outside `project_root`; with no known project it always does. Both are
/// canonical paths, so a component-wise prefix test is exact.
pub fn is_outside_project(cwd: &Path, project_root: Option<&Path>) -> bool {
    project_root.is_none_or(|root| !cwd.starts_with(root))
}

/// Which kind of hiding character `character` is, if any: control codes other than the
/// newline (escape sequences rewrite lines, a carriage return overwrites one, a tab passes for
/// spaces), and invisible or direction-changing Unicode. The set is the one the current
/// approval terminal escapes (`apps/crates/cockpit-core/src/approval.rs`).
fn hidden_kind(character: char) -> Option<Hidden> {
    match u32::from(character) {
        0x0a => None,
        0x00..=0x1f | 0x7f..=0x9f => Some(Hidden::Control),
        0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069 => Some(Hidden::BidiControl),
        0xad
        | 0x34f
        | 0x115f
        | 0x1160
        | 0x180e
        | 0x200b..=0x200d
        | 0x2028..=0x2029
        | 0x2060..=0x2065
        | 0x2800
        | 0x3164
        | 0xfe00..=0xfe0f
        | 0xfeff
        | 0xffa0
        | 0xe0000..=0xe007f => Some(Hidden::Invisible),
        _ => None,
    }
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
