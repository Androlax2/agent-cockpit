//! Canary values stand in for hosted work: prompts, commands the agent runs and environment
//! values. None of them may reach a log file or a support bundle.

mod support;

use std::fs;
use std::path::Path;
use std::process::Command;

use cockpit_diagnostics::{BundleRequest, LevelOverride, plan_bundle};
use support::{current_log, record};

const PROMPT_CANARY: &str = "canary-prompt-6f1d";
const COMMAND_CANARY: &str = "canary-command-91ab";
const ENV_CANARY: &str = "canary-env-value-3c7e";
const CHILD_DIR_VAR: &str = "COCKPIT_DIAGNOSTICS_CANARY_DIR";

fn assert_no_canary(text: &str, place: &str) {
    for canary in [PROMPT_CANARY, COMMAND_CANARY, ENV_CANARY] {
        assert!(!text.contains(canary), "{canary} leaked into {place}");
    }
}

#[test]
fn records_carrying_hosted_content_fields_are_refused() {
    let log_dir = tempfile::tempdir().unwrap();
    record(log_dir.path(), "trace", |diagnostics| {
        tracing::info!(prompt = PROMPT_CANARY, "turn submitted");
        tracing::info!(shell_command = COMMAND_CANARY, "agent ran a command");
        tracing::info!(env = ENV_CANARY, "agent environment");
        assert_eq!(diagnostics.refused_records(), 3);
    });
    assert_no_canary(&current_log(log_dir.path()), "cockpit.log");
}

#[test]
fn a_span_carrying_hosted_content_hides_nothing_else() {
    let log_dir = tempfile::tempdir().unwrap();
    record(log_dir.path(), "info", |_| {
        let turn = tracing::info_span!("turn", prompt = PROMPT_CANARY);
        let _entered = turn.enter();
        tracing::info!("turn started");
    });
    let log = current_log(log_dir.path());
    assert_no_canary(&log, "cockpit.log");
    assert!(
        log.contains("turn started"),
        "the event itself is still logged: {log}"
    );
}

#[test]
fn a_refused_record_is_reported_by_callsite_without_its_values() {
    let log_dir = tempfile::tempdir().unwrap();
    record(log_dir.path(), "info", |_| {
        tracing::info!(prompt = PROMPT_CANARY, "turn submitted")
    });
    let log = current_log(log_dir.path());
    assert!(log.contains("record with hosted content refused"), "{log}");
    assert!(
        log.contains("canary.rs"),
        "the warning names the callsite: {log}"
    );
}

/// Runs in a child process so the canary is in the real environment of a process that
/// starts diagnostics, changes levels and writes a support bundle.
#[test]
fn environment_values_never_reach_logs_or_bundles() {
    let Ok(work_dir) = std::env::var(CHILD_DIR_VAR) else {
        let work_dir = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "environment_values_never_reach_logs_or_bundles",
                "--nocapture",
            ])
            .env(CHILD_DIR_VAR, work_dir.path())
            .env("COCKPIT_CANARY_SECRET", ENV_CANARY)
            .env("ANTHROPIC_API_KEY", ENV_CANARY)
            .status()
            .unwrap();
        assert!(status.success(), "the child scenario failed");
        assert_tree_has_no_canary(work_dir.path());
        return;
    };
    let work_dir = Path::new(&work_dir);
    let log_dir = work_dir.join("logs");
    record(&log_dir, "trace", |diagnostics| {
        tracing::info!("daemon started");
        diagnostics
            .level()
            .set_override(LevelOverride::parse("trace", Some("1m")).unwrap())
            .unwrap();
        diagnostics.level().revert().unwrap();
    });
    plan_bundle(&BundleRequest::new(vec![log_dir]))
        .unwrap()
        .write(&work_dir.join("bundle.tar"))
        .unwrap();
}

fn assert_tree_has_no_canary(directory: &Path) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_tree_has_no_canary(&path);
            continue;
        }
        let contents = fs::read(&path).unwrap();
        assert!(
            !contents.is_empty()
                || path.ends_with("cockpit.log")
                || path.ends_with("cockpit.log.lock"),
            "{} is empty",
            path.display()
        );
        assert_no_canary(
            &String::from_utf8_lossy(&contents),
            &path.display().to_string(),
        );
    }
}
