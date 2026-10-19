//! claude-fade in a real (pseudo-)terminal, between it and a shell standing in for the nested tmux
//! client: keystrokes pass untouched, colours fade after tmux's focus-out and come back after focus-in.

mod common;

use common::{Terminal, contains};
use std::path::Path;

const PRINT_RED_ON_ENTER: &str = r#"stty -echo; printf ready; read line; printf '\033[31mred\033[0m\n'; sleep 1"#;

fn relay(script: &str) -> Terminal {
    let mut terminal = Terminal::start(Path::new(env!("CARGO_BIN_EXE_claude-fade")), &["sh", "-c", script], &[]);
    terminal.pump(5.0, Some(b"ready"));
    terminal
}

#[test]
fn keystrokes_and_focus_reports_reach_the_command_unchanged() {
    let mut terminal = relay(r#"stty -echo; printf ready; read line; printf 'got:%s|\n' "$line"; sleep 1"#);
    terminal.type_keys(b"a\x1b[Ob\x1b[Ic\r");
    terminal.pump(5.0, Some(b"|"));
    assert!(contains(&terminal.screen, b"got:a\x1b[Ob\x1b[Ic|"), "{:?}", String::from_utf8_lossy(&terminal.screen));
}

#[test]
fn colours_fade_after_a_focus_out() {
    let mut terminal = relay(PRINT_RED_ON_ENTER);
    terminal.type_keys(b"\x1b[O\r");
    terminal.pump(5.0, Some(b"red"));
    assert!(contains(&terminal.screen, b"\x1b[31m\x1b[2mred"), "{:?}", String::from_utf8_lossy(&terminal.screen));
}

#[test]
fn colours_come_back_after_a_focus_in() {
    let mut terminal = relay(PRINT_RED_ON_ENTER);
    terminal.type_keys(b"\x1b[O\x1b[I\r");
    terminal.pump(5.0, Some(b"red"));
    assert!(contains(&terminal.screen, b"\x1b[31mred"), "{:?}", String::from_utf8_lossy(&terminal.screen));
}

#[test]
fn the_relay_ends_with_the_commands_exit_code() {
    let mut terminal = relay("printf ready; exit 3");
    assert_eq!(terminal.child.wait().unwrap().code(), Some(3));
}
