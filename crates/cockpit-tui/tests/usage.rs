//! Each command the TUI runs reaches its own log by name (decision 17, #176), never what the
//! owner typed.

use std::time::{Duration, Instant};

use cockpit_tui::{App, Key, record_usage};

#[test]
fn the_tuis_log_counts_each_command_by_name_and_never_the_typed_text() {
    let state = tempfile::tempdir().unwrap();
    let log_dir = cockpit_diagnostics::log_dir_in(state.path(), "tui").unwrap();
    let diagnostics = cockpit_diagnostics::start(cockpit_diagnostics::Options::new(
        log_dir.clone(),
        "info".parse().unwrap(),
    ))
    .unwrap();
    let mut app = App::new();
    let now = Instant::now();

    tracing::dispatcher::with_default(diagnostics.dispatch(), || {
        for key in [Key::Char('?'), Key::Esc, Key::Char('N')]
            .into_iter()
            .chain("secret words".chars().map(Key::Char))
        {
            record_usage(&app.key(key, now));
        }
    });
    assert!(diagnostics.shutdown(Duration::from_secs(5)));

    let log = std::fs::read_to_string(log_dir.join("cockpit.log")).unwrap();
    for name in [
        "\"help\"",
        "\"close-overlay\"",
        "\"new-session-form\"",
        "\"form-edit\"",
    ] {
        assert!(
            log.contains(&format!("\"command\":{name}")),
            "{name} in {log}"
        );
    }
    assert!(!log.contains("secret"), "{log}");
}
