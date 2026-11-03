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
/// Characters of the last line the session's reason quotes.
const REASON_CHARACTERS: usize = 200;
/// What tmux writes into a pane whose process ended under `remain-on-exit`.
const DEAD_PANE_NOTICE: &str = "Pane is dead (";

/// The agent's exit as its terminal shows it. A terminal that is gone, or whose agent pane
/// still runs, reads as an unknown status with no output.
pub(crate) fn read(tmux: &Server, name: &SessionName) -> Result<AgentExit, Error> {
    if !tmux.sessions()?.contains(name) {
        return Ok(unknown());
    }
    let pane = crate::lifecycle::agent_pane_info(tmux, name)?;
    let status = match pane.state {
        PaneState::Running => return Ok(unknown()),
        PaneState::Exited { status: Some(code) } => ExitStatus::Code(code),
        PaneState::Exited { status: None } => ExitStatus::Signal,
    };
    let printed = tmux.capture_text(&pane.id, HISTORY_LINES)?;
    Ok(AgentExit {
        status,
        output: last_lines(&printed),
    })
}

pub(crate) fn unknown() -> AgentExit {
    AgentExit {
        status: ExitStatus::Unknown,
        output: Vec::new(),
    }
}

/// The session's reason for an agent that exited: its status and the last line it printed.
pub(crate) fn reason(exit: &AgentExit) -> String {
    let ended = match exit.status {
        ExitStatus::Code(code) => format!("its agent exited with status {code}"),
        ExitStatus::Signal => "its agent exited on a signal".to_owned(),
        ExitStatus::Unknown => "its agent exited".to_owned(),
    };
    match exit.output.last() {
        Some(line) => format!(
            "{ended}: {}; resume reopens it",
            cut(line, REASON_CHARACTERS)
        ),
        None => format!("{ended}; resume reopens it"),
    }
}

/// The last [`KEPT_LINES`] lines of a pane's text, without control characters, trailing
/// blanks or tmux's own dead-pane notice.
fn last_lines(printed: &str) -> Vec<String> {
    let mut lines: Vec<String> = printed
        .lines()
        .map(|line| {
            let visible: String = line
                .chars()
                .map(|character| if character == '\t' { ' ' } else { character })
                .filter(|character| !character.is_control())
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
    fn the_reason_names_the_status_and_the_last_line() {
        let exit = AgentExit {
            status: ExitStatus::Code(1),
            output: vec!["first".to_owned(), "Error: refusing to start".to_owned()],
        };

        assert_eq!(
            reason(&exit),
            "its agent exited with status 1: Error: refusing to start; resume reopens it"
        );
    }

    #[test]
    fn an_unknown_exit_keeps_the_plain_reason() {
        assert_eq!(reason(&unknown()), "its agent exited; resume reopens it");
    }
}
