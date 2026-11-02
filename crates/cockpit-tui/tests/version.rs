//! The installed TUI's headless packaging smoke path.

#[test]
fn version_runs_without_a_terminal_daemon_or_home() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit-tui"))
        .arg("--version")
        .env_clear()
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("cockpit-tui {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}
