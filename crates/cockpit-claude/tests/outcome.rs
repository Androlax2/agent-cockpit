//! A turn's outcome (#183): the bridge passes it on the agent channel, and the adapter
//! reports the `outcome` capability only when the bridge can deliver it.

mod common;

use std::path::PathBuf;

use cockpit_claude::{ClaudeCode, hooks};
use cockpit_provider_api::{Availability, Capability, Provider};

fn stop_failure_arguments(error: &str) -> Vec<String> {
    let payload = serde_json::json!({"hook_event_name": "StopFailure", "error": error});
    hooks::report_arguments(&hooks::normalize(&payload.to_string()).unwrap()).unwrap()
}

fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

#[test]
fn a_failed_turn_reports_its_reason() {
    assert_eq!(
        stop_failure_arguments("server_error"),
        words(&[
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "failed",
            "--outcome-reason",
            "server_error",
        ])
    );
}

#[test]
fn a_failure_without_a_reason_reports_failed_alone() {
    let payload = r#"{"hook_event_name":"StopFailure"}"#;

    let arguments = hooks::report_arguments(&hooks::normalize(payload).unwrap()).unwrap();

    assert_eq!(
        arguments,
        words(&[
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "failed"
        ])
    );
}

#[test]
fn a_reason_the_channel_would_refuse_is_left_out() {
    for reason in ["#(rm -rf ~)", "bidi \u{202E}txet", "a\u{0007}bell", " "] {
        assert_eq!(
            stop_failure_arguments(reason),
            words(&[
                "agent",
                "report",
                "--state",
                "waiting",
                "--outcome",
                "failed"
            ]),
            "{reason:?}"
        );
    }
}

#[test]
fn a_long_reason_is_cut_to_the_channels_limit() {
    let arguments = stop_failure_arguments(&"word ".repeat(40));
    let reason = arguments.last().unwrap();

    assert_eq!(reason.chars().count(), 80);
    assert!(reason.ends_with('…'));
}

#[test]
fn with_the_bridge_the_outcome_capability_is_reported() {
    let capabilities = common::claude().capabilities();

    assert!(matches!(
        capabilities.availability(Capability::Outcome),
        Availability::Available { .. }
    ));
}

#[test]
fn without_the_bridge_neither_events_nor_outcome_are_reported() {
    let claude = ClaudeCode::new(
        PathBuf::from("/usr/bin/claude"),
        Some(cockpit_claude::CONFORMANCE_VERSION.to_owned()),
        None,
    );

    let capabilities = claude.capabilities();

    for capability in [Capability::Events, Capability::Outcome] {
        assert_eq!(
            capabilities.availability(capability),
            Availability::Unavailable,
            "{capability:?}"
        );
    }
}
