//! What the adapter reports, and how it knows: tested only on the pinned version.

mod common;

use std::path::PathBuf;

use cockpit_codex::{CONFORMANCE_VERSION, Codex};
use cockpit_provider_api::{Availability, Capability, Provenance, Provider};

const REPORTED: [Capability; 5] = [
    Capability::Resume,
    Capability::Events,
    Capability::Submit,
    Capability::Cancel,
    Capability::Outcome,
];

fn on_version(version: Option<&str>) -> Codex {
    Codex::new(
        PathBuf::from("/usr/bin/codex"),
        version.map(str::to_owned),
        Some(common::bridge()),
    )
}

#[test]
fn the_provider_is_named_codex() {
    assert_eq!(common::codex().name().as_str(), "codex");
}

#[test]
fn on_the_pinned_version_each_capability_is_available_with_that_version() {
    let capabilities = common::codex().capabilities();

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
    let capabilities = on_version(Some("0.162.0")).capabilities();

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

    for capability in REPORTED {
        assert_eq!(
            capabilities.availability(capability),
            Availability::Unverified,
            "{capability:?}"
        );
    }
}

/// The tested set (#48, #407): effort, compact, billing, commands, evidence, remote control
/// and screen reading are not.
#[test]
fn it_reports_nothing_beyond_resume_events_submit_cancel_and_outcome() {
    let capabilities = common::codex().capabilities();

    for capability in Capability::ALL {
        if !REPORTED.contains(&capability) {
            assert_eq!(
                capabilities.availability(capability),
                Availability::Unavailable,
                "{capability:?}"
            );
        }
    }
}

#[test]
fn without_the_bridge_only_resume_is_reported() {
    let codex = Codex::new(
        PathBuf::from("/usr/bin/codex"),
        Some(CONFORMANCE_VERSION.to_owned()),
        None,
    );
    let capabilities = codex.capabilities();

    for capability in Capability::ALL {
        assert_eq!(
            capabilities.supports(capability),
            capability == Capability::Resume,
            "{capability:?}"
        );
    }
}
