//! Claude Code's activity read from the end of its terminal: the fallback for when hooks go
//! missing (report 05). Terminal bytes are untrusted display data, so each reading records
//! its confidence and none proves a task complete. Patterns are those the current cockpit's
//! `osd classify` matches, checked in its order: dialogs, then a turn, then the prompt.

use cockpit_provider_api::{Activity, Confidence, Observation, ObservedBy};

/// Lines a permission dialog's first option may sit below its question.
const DIALOG_OPTION_LINES: usize = 4;

pub(crate) fn read(screen: &str) -> Option<Observation> {
    let lines: Vec<&str> = screen.lines().collect();
    let (activity, confidence) = if has_permission_dialog(&lines) {
        (Activity::NeedsYou, Confidence::High)
    } else if lines.iter().any(|line| is_choice_menu_cursor(line)) {
        (Activity::NeedsYou, Confidence::Low)
    } else if lines
        .iter()
        .any(|line| is_running_spinner(line) || is_interrupt_hint(line))
    {
        (Activity::Working, Confidence::High)
    } else if lines.iter().any(|line| is_prompt_line(line)) {
        (Activity::Waiting, Confidence::Low)
    } else {
        return None;
    };
    Some(Observation {
        activity,
        observed_by: ObservedBy::Screen(confidence),
    })
}

/// The line's text inside a box: without leading and trailing spaces and `│`.
fn boxed_text(line: &str) -> &str {
    line.trim_matches(|c: char| c.is_whitespace() || c == '│')
}

/// "Do you want to proceed?" alone on a line, then "1. Yes" within a few lines.
fn has_permission_dialog(lines: &[&str]) -> bool {
    lines.iter().enumerate().any(|(index, line)| {
        boxed_text(line) == "Do you want to proceed?"
            && lines
                .iter()
                .skip(index + 1)
                .take(DIALOG_OPTION_LINES)
                .any(|option| {
                    let option = boxed_text(option)
                        .trim_start_matches(|c: char| c.is_whitespace() || c == '❯');
                    option
                        .strip_prefix("1.")
                        .is_some_and(|rest| rest.trim_start().starts_with("Yes"))
                })
    })
}

/// "❯ 2. Option": a numbered menu with the cursor on an option.
fn is_choice_menu_cursor(line: &str) -> bool {
    let Some(rest) = line
        .trim_start_matches(|c: char| c.is_whitespace() || c == '│')
        .strip_prefix('❯')
    else {
        return false;
    };
    let rest = rest.trim_start();
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let Some(after_number) = rest[digits..].strip_prefix('.') else {
        return false;
    };
    digits > 0
        && after_number.starts_with(char::is_whitespace)
        && !after_number.trim_start().is_empty()
}

/// "✻ Compiling… (12s": up to two symbols, a capitalized verb with an ellipsis, elapsed time.
fn is_running_spinner(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    words.windows(2).take(2).any(|pair| {
        let Some(verb) = pair[0].strip_suffix('…') else {
            return false;
        };
        let mut letters = verb.chars();
        let is_capitalized = letters.next().is_some_and(|c| c.is_ascii_uppercase())
            && letters.clone().next().is_some()
            && letters.all(|c| c.is_ascii_lowercase());
        let elapsed = pair[1].strip_prefix('(').unwrap_or("");
        let digits = elapsed.chars().take_while(char::is_ascii_digit).count();
        is_capitalized && digits > 0 && elapsed[digits..].starts_with(['s', 'm', 'h'])
    })
}

/// "esc to interrupt" (or "enter to interrupt and send") opening a status line.
fn is_interrupt_hint(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("esc to interrupt") || line.starts_with("enter to interrupt")
}

/// The prompt: `❯` then the space or no-break space Claude Code pads it with.
fn is_prompt_line(line: &str) -> bool {
    line.trim_start_matches(|c: char| c.is_whitespace() || c == '│')
        .strip_prefix('❯')
        .is_some_and(|rest| rest.starts_with([' ', '\u{a0}']) || rest.is_empty())
}
