//! The recoverable draft's existing plain-text representation.

const EDITOR_HELP_MARK: &str = "#── everything from this line on is left out ──";

/// A line typed in the plain terminal, outside the column's screen.
/// The editor's file: the title on the first line, the notes below, then help (Claude's reason
/// for a proposal included) that is left out when read back.
pub fn editor_text(title: &str, notes: &str, why: &str) -> String {
    let mut text = format!(
        "{title}\n\n{notes}\n\n{EDITOR_HELP_MARK}\n# First line: the title. Below it: the notes, as long as needed.\n# Save and quit to keep it; an empty title cancels.\n"
    );
    if !why.is_empty() {
        text.push_str(&format!(
            "# Why Claude proposed it:\n# {}\n",
            why.replace('\n', "\n# ")
        ));
    }
    text
}

/// Title and notes back from the editor's file; None for an empty title.
pub fn parse_editor_text(text: &str) -> Option<(String, String)> {
    let body = text.split(EDITOR_HELP_MARK).next().unwrap_or("");
    let mut lines = body.lines();
    let title = lines.next()?.trim().to_string();
    if title.is_empty() {
        return None;
    }
    let notes = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    Some((title, notes))
}
