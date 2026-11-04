//! #682: `--bell on|off` is the one way the daemon tells the column about the owner's
//! `tui.attention_bell`; any other word is a usage error, not a silent default.

fn column_with(bell_words: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_torlek-tui"))
        .args([
            "--column",
            "--socket",
            "/nonexistent/daemon.sock",
            "--profile",
            "dev",
            "--state-dir",
            "/nonexistent/state",
        ])
        .args(bell_words)
        .env_clear()
        .output()
        .unwrap()
}

#[test]
fn a_bell_word_that_is_neither_on_nor_off_is_a_usage_error() {
    let output = column_with(&["--bell", "loud"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("[--bell on|off]"),
        "{output:?}"
    );
}

#[test]
fn a_bell_flag_without_its_word_is_a_usage_error() {
    let output = column_with(&["--bell"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
}

#[test]
fn an_unknown_flag_after_the_state_directory_is_a_usage_error() {
    let output = column_with(&["--quiet"]);

    assert_eq!(output.status.code(), Some(2), "{output:?}");
}
