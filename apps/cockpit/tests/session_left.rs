//! cockpit-left: Left in the cockpit's session pane goes back to the column from the start
//! of Claude's prompt, and reaches Claude anywhere else (tmux.conf binds it).

mod common;
use common::sandbox::*;

/// capture-pane as tmux does it: the line's trailing spaces dropped, unless -N keeps them.
const CAPTURE: &str = r#"capture() { if [[ " $* " == *" -N "* ]]; then printf '%s\n' "$LINE"; else printf '%s\n' "${LINE%"${LINE##*[! ]}"}"; fi; }"#;

/// What tmux was asked after Left, with the pane's cursor at `cursor_x` on a line reading `line`.
fn left_with(line: &str, cursor_x: usize) -> Vec<String> {
    let sandbox = Sandbox::new();
    sandbox.shim("tmux", &format!(r#"{CAPTURE}
LINE='{line}'
case $1 in
  display-message) echo "{cursor_x} 7" ;;
  capture-pane) capture "$@" ;;
esac"#));
    let output = sandbox.run(bin().join("cockpit-left"), &["%11"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    sandbox.calls("tmux").into_iter().filter(|call| !call.starts_with("tmux display-message") && !call.starts_with("tmux capture-pane")).collect()
}

#[test]
fn left_on_an_empty_prompt_goes_back_to_the_column() {
    for line in ["❯ ", "❯\u{a0}", "  ❯ ", "│ ❯ "] {
        let cursor_x = line.chars().count();
        assert_eq!(left_with(line, cursor_x), ["tmux select-pane -L -t %11"], "{line:?}");
    }
}

#[test]
fn left_at_the_start_of_the_text_goes_back_to_the_column() {
    // Nothing is left of the cursor in the prompt: Claude would not move it.
    assert_eq!(left_with("❯ fix the login", 2), ["tmux select-pane -L -t %11"]);
}

#[test]
fn left_inside_the_text_reaches_claude() {
    assert_eq!(left_with("❯ fix the login", 7), ["tmux send-keys -t %11 Left"]);
}

#[test]
fn left_off_the_prompt_line_reaches_claude() {
    // A second line of the prompt, or output: the cursor is not after ❯.
    for (line, cursor_x) in [("and then the tests", 0), ("❯ 1. Yes", 9)] {
        assert_eq!(left_with(line, cursor_x), ["tmux send-keys -t %11 Left"], "{line:?}");
    }
}

#[test]
fn left_on_an_empty_opencode_prompt_goes_back_to_the_column() {
    // As tmux showed it: the input box's ┃, two spaces, then the cursor on the dim placeholder.
    let line = format!("{}┃  Ask anything… \"Fix a TODO in the codebase\"", " ".repeat(86));
    assert_eq!(left_with(&line, 89), ["tmux select-pane -L -t %11"]);
}

#[test]
fn left_on_an_empty_opencode_prompt_inside_a_session_goes_back_to_the_column() {
    // As tmux showed it on 2026-10-19 inside a session: no placeholder, so the two spaces after ┃
    // are the line's last characters, which capture-pane drops without -N.
    assert_eq!(left_with("  ┃  ", 5), ["tmux select-pane -L -t %11"]);
}

#[test]
fn left_inside_opencode_text_reaches_opencode() {
    assert_eq!(left_with("  ┃  fix the login", 10), ["tmux send-keys -t %11 Left"]);
}

/// What tmux was asked after Left in the viewer %11 while the cockpit shows `shown`: the shown
/// session's own pane %20 reads `own_line` at `own_x`, the viewer's copy reads something else.
fn left_showing(shown: &str, own_line: &str, own_x: usize) -> Vec<String> {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.runtime.join("cockpit.current"), format!("{shown}\n")).unwrap();
    std::fs::write(sandbox.runtime.join("cockpit.json"),
                   serde_json::json!({"sessions": [{"session": OPENCODE_SESSION, "tmux_pane": "%20"}]}).to_string()).unwrap();
    sandbox.shim("tmux", &format!(r#"{CAPTURE}
LINE='{own_line}'
case "$*" in
  "list-panes -t =claude-retry-opencode-1"*) echo "%20" ;;
  "display-message -p -t %20 "*) echo "{own_x} 64" ;;
  "display-message -p -t %11 "*) echo "3 37" ;;
  "capture-pane -p -N -t %20 "*) capture "$@" ;;
  "capture-pane -p -N -t %11 "*) printf '%s\n' 'some other line of the viewer' ;;
esac"#));
    let output = sandbox.run(bin().join("cockpit-left"), &["%11"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    sandbox.calls("tmux").into_iter().filter(|call| call.starts_with("tmux select-pane") || call.starts_with("tmux send-keys")).collect()
}

#[test]
fn left_reads_the_shown_sessions_own_pane_not_the_viewers_copy() {
    // As tmux showed it on 2026-10-19: opencode's window 70 rows high in a 41-row viewer.
    for shown in [OPENCODE_SESSION, "claude-retry-opencode-1"] {
        assert_eq!(left_showing(shown, "  ┃  ", 5), ["tmux select-pane -L -t %11"], "{shown}");
    }
}

#[test]
fn left_in_the_shown_sessions_text_still_goes_to_it_through_the_viewer() {
    assert_eq!(left_showing(OPENCODE_SESSION, "  ┃  fix the login", 10), ["tmux send-keys -t %11 Left"]);
}
