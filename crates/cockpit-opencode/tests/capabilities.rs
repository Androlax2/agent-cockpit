//! What the adapter reports, and how it knows it.

mod common;

use std::path::PathBuf;

use cockpit_opencode::{CONFORMANCE_VERSION, OpenCode};
use cockpit_provider_api::{Availability, Capability, Provenance, Provider};

#[test]
fn on_the_pinned_version_each_capability_is_tested() {
    let capabilities = common::opencode().capabilities();
    let reported: Vec<(Capability, Provenance)> = capabilities
        .reported()
        .map(|(capability, provenance)| (capability, provenance.clone()))
        .collect();
    let tested = Provenance::Tested {
        version: CONFORMANCE_VERSION.to_owned(),
    };

    assert_eq!(
        reported,
        [
            Capability::Resume,
            Capability::Submit,
            Capability::Events,
            Capability::Cancel,
            Capability::Outcome,
        ]
        .map(|capability| (capability, tested.clone()))
    );
}

#[test]
fn on_another_version_each_capability_is_unverified() {
    let opencode = OpenCode::new(
        PathBuf::from("/usr/bin/opencode"),
        Some("1.20.0".to_owned()),
        Some(common::bridge()),
    );

    for (capability, _) in opencode.capabilities().reported() {
        assert_eq!(
            opencode.capabilities().availability(capability),
            Availability::Unverified,
            "{}",
            capability.as_str()
        );
    }
}

#[test]
fn without_the_bridge_events_and_outcomes_are_unavailable() {
    let opencode = OpenCode::new(
        PathBuf::from("/usr/bin/opencode"),
        Some(CONFORMANCE_VERSION.to_owned()),
        None,
    );

    for capability in [Capability::Events, Capability::Outcome] {
        assert_eq!(
            opencode.capabilities().availability(capability),
            Availability::Unavailable
        );
    }
}

/// A launch cannot choose the TUI's variant, and no compaction, screen reading, remote
/// control or billing is offered; commands wait for their contract method.
#[test]
fn what_it_cannot_honor_is_not_reported() {
    let capabilities = common::opencode().capabilities();

    for capability in [
        Capability::Effort,
        Capability::Compact,
        Capability::ScreenReading,
        Capability::RemoteControl,
        Capability::Billing,
        Capability::Commands,
        Capability::Evidence,
    ] {
        assert!(
            !capabilities.supports(capability),
            "{}",
            capability.as_str()
        );
    }
}

#[test]
fn its_name_is_opencode() {
    assert_eq!(common::opencode().name().as_str(), "opencode");
    assert_eq!(common::opencode().version(), Some(CONFORMANCE_VERSION));
}
