//! How an agent that exited by itself ended, read from its terminal before the session closes
//! it (#629): its exit status and the last lines it printed, which hold the reason an agent
//! or its sandbox wrapper refused to start.

use cockpit_tmux::{PaneState, Server, SessionName};

use crate::{AgentExit, Error, ExitStatus};

/// Lines above the visible screen read from the pane: the screen alone may be blank below
/// a short error.
const HISTORY_LINES: u32 = 200;
/// Lines kept: enough for an error and its context, little enough to show in a reply.
const KEPT_LINES: usize = 20;
/// Characters kept of one line; a longer one is cut.
const LINE_CHARACTERS: usize = 400;
/// What tmux writes into a pane whose process ended under `remain-on-exit`.
const DEAD_PANE_NOTICE: &str = "Pane is dead (";

/// The agent's exit as its terminal shows it, or `None` when the terminal is gone (#844):
/// nothing can be read from it. A pane tmux still shows running (a wrapper outliving the
/// agent, or tmux not yet marking it dead) reads as an unknown status with what it printed.
pub(crate) fn read(tmux: &Server, name: &SessionName) -> Result<Option<AgentExit>, Error> {
    if !tmux.sessions()?.contains(name) {
        return Ok(None);
    }
    let pane = crate::lifecycle::agent_pane_info(tmux, name)?;
    let status = match pane.state {
        PaneState::Running => ExitStatus::Unknown,
        PaneState::Exited { status: Some(code) } => ExitStatus::Code(code),
        PaneState::Exited { status: None } => ExitStatus::Signal,
    };
    let printed = tmux.capture_text(&pane.id, HISTORY_LINES)?;
    Ok(Some(AgentExit {
        status,
        output: last_lines(&printed),
    }))
}

pub(crate) fn unknown() -> AgentExit {
    AgentExit {
        status: ExitStatus::Unknown,
        output: Vec::new(),
    }
}

/// The session's reason for an agent that exited: its status only. The reason is daemon
/// text other agents may read; what the agent printed stays in [`AgentExit::output`].
pub(crate) fn reason(status: ExitStatus) -> String {
    match status {
        ExitStatus::Unknown => "its agent exited; resume reopens it".to_owned(),
        known => format!("its agent exited with {known}; resume reopens it"),
    }
}

/// The last [`KEPT_LINES`] lines of a pane's text, without control or invisible formatting
/// characters, trailing blanks or tmux's own dead-pane notice.
fn last_lines(printed: &str) -> Vec<String> {
    let mut lines: Vec<String> = printed
        .lines()
        .map(|line| {
            let visible: String = line
                .chars()
                .map(|character| if character == '\t' { ' ' } else { character })
                .filter(|character| !character.is_control() && !is_invisible_format(*character))
                .collect();
            cut(visible.trim_end(), LINE_CHARACTERS)
        })
        .collect();
    let drop_trailing_blanks = |lines: &mut Vec<String>| {
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
    };
    drop_trailing_blanks(&mut lines);
    if lines
        .last()
        .is_some_and(|line| line.starts_with(DEAD_PANE_NOTICE))
    {
        lines.pop();
        drop_trailing_blanks(&mut lines);
    }
    let first_kept = lines.len().saturating_sub(KEPT_LINES);
    lines.split_off(first_kept)
}

/// Zero-width and bidirectional formatting characters, which could make a shown line read
/// as something else.
fn is_invisible_format(character: char) -> bool {
    matches!(
        character,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

fn cut(line: &str, characters: usize) -> String {
    match line.char_indices().nth(characters) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dead_pane_notice_and_trailing_blanks_are_dropped() {
        let printed = "hello\nError: refusing to start\n\nPane is dead (status 1, Tue Nov  3 16:22:23 2026)\n\n\n";

        assert_eq!(last_lines(printed), ["hello", "Error: refusing to start"]);
    }

    #[test]
    fn control_characters_are_removed_and_tabs_become_spaces() {
        assert_eq!(last_lines("a\tb\u{7}c\u{1b}"), ["a bc"]);
    }

    #[test]
    fn only_the_last_lines_are_kept() {
        let printed: String = (0..30).map(|number| format!("line {number}\n")).collect();

        let kept = last_lines(&printed);

        assert_eq!(kept.len(), KEPT_LINES);
        assert_eq!(kept.last().map(String::as_str), Some("line 29"));
    }

    #[test]
    fn bidirectional_and_zero_width_characters_are_removed() {
        assert_eq!(last_lines("ok\u{202E}desrever\u{200B}!"), ["okdesrever!"]);
    }

    #[test]
    fn the_reason_names_the_status_only() {
        assert_eq!(
            reason(ExitStatus::Code(1)),
            "its agent exited with status 1; resume reopens it"
        );
    }

    #[test]
    fn an_unknown_exit_keeps_the_plain_reason() {
        assert_eq!(
            reason(ExitStatus::Unknown),
            "its agent exited; resume reopens it"
        );
    }
}
