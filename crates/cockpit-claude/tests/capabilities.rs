//! What the adapter reports, and how it knows: tested only on the pinned version.

mod common;

use std::path::PathBuf;

use cockpit_claude::{CONFORMANCE_VERSION, ClaudeCode};
use cockpit_provider_api::{Availability, Capability, Provenance, Provider};

const REPORTED: [Capability; 9] = [
    Capability::Resume,
    Capability::Submit,
    Capability::Events,
    Capability::Outcome,
    Capability::Cancel,
    Capability::Effort,
    Capability::Compact,
    Capability::RemoteControl,
    Capability::ScreenReading,
];

fn on_version(version: Option<&str>) -> ClaudeCode {
    ClaudeCode::new(
        PathBuf::from("/usr/bin/claude"),
        version.map(str::to_owned),
        Some(common::bridge()),
    )
}

#[test]
fn on_the_pinned_version_each_capability_is_available_with_that_version() {
    let capabilities = common::claude().capabilities();

    for capability in REPORTED {
        assert_eq!(
            capabilities.availability(capability),
            Availability::Available {
                version: CONFORMANCE_VERSION.to_owned()
            },
            "{capability:?}"
        );
    }
}

#[test]
fn on_another_version_each_capability_is_unverified() {
    let capabilities = on_version(Some("2.2.0")).capabilities();

    for capability in REPORTED {
        assert_eq!(
            capabilities.provenance(capability),
            Some(&Provenance::Discovered),
            "{capability:?}"
        );
    }
}

#[test]
fn with_an_unknown_version_each_capability_is_unverified() {
    let capabilities = on_version(None).capabilities();

    assert_eq!(
        capabilities.availability(Capability::Resume),
        Availability::Unverified
    );
}

#[test]
fn commands_evidence_and_billing_are_not_reported_yet() {
    let capabilities = common::claude().capabilities();

    for capability in [
        Capability::Commands,
        Capability::Evidence,
        Capability::Billing,
    ] {
        assert_eq!(
            capabilities.availability(capability),
            Availability::Unavailable,
            "{capability:?}"
        );
    }
}

#[test]
fn the_adapter_names_itself_claude_with_its_running_version() {
    let claude = common::claude();

    assert_eq!(claude.name().as_str(), "claude");
    assert_eq!(claude.version(), Some(CONFORMANCE_VERSION));
}
