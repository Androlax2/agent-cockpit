#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

use std::fs;
use std::path::Path;
use std::time::Duration;

use cockpit_diagnostics::{Diagnostics, Directives, Options};
use serde_json::Value;

pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub fn start(log_dir: &Path, directives: &str) -> Diagnostics {
    let directives: Directives = directives.parse().expect("test directives are valid");
    cockpit_diagnostics::start(Options::new(log_dir.to_path_buf(), directives))
        .expect("diagnostics start in a fresh directory")
}

/// Runs `emit` with the started subscriber as the thread's default, then drains the writer.
pub fn record(log_dir: &Path, directives: &str, emit: impl FnOnce(&Diagnostics)) {
    let diagnostics = start(log_dir, directives);
    tracing::dispatcher::with_default(diagnostics.dispatch(), || emit(&diagnostics));
    assert!(
        diagnostics.shutdown(SHUTDOWN_TIMEOUT),
        "the writer drains within the timeout"
    );
}

pub fn current_log(log_dir: &Path) -> String {
    fs::read_to_string(log_dir.join("cockpit.log")).expect("cockpit.log is readable")
}

pub fn records(log_dir: &Path) -> Vec<Value> {
    current_log(log_dir)
        .lines()
        .map(|line| serde_json::from_str(line).expect("every line is a JSON record"))
        .collect()
}

pub fn messages(log_dir: &Path) -> Vec<String> {
    records(log_dir)
        .iter()
        .map(|record| {
            record["fields"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}
