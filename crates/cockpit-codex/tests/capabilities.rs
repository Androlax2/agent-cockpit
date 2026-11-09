//! What the adapter reports, and how it knows: tested only on the pinned version.

mod common;

use std::path::PathBuf;

use cockpit_codex::{CONFORMANCE_VERSION, Codex};
use cockpit_provider_api::{Availability, Capability, Provenance, Provider};

const REPORTED: [Capability; 9] = [
    Capability::Resume,
    Capability::Model,
    Capability::Effort,
    Capability::Events,
    Capability::Submit,
    Capability::Cancel,
    Capability::Outcome,
    Capability::CompactionThreshold,
    Capability::Quota,
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
fn the_validated_0166_release_has_available_capabilities() {
    let capabilities = on_version(Some("0.166.0")).capabilities();
    for capability in REPORTED {
        assert_eq!(
            capabilities.availability(capability),
            Availability::Available {
                version: "0.166.0".to_owned()
            },
            "{capability:?}"
        );
    }
}

#[test]
fn the_previous_0161_release_is_unverified_after_revalidation() {
    let capabilities = on_version(Some("0.161.0")).capabilities();
    for capability in REPORTED {
        assert_eq!(
            capabilities.availability(capability),
            Availability::Unverified,
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

/// The tested set (#48, #407, #783, #1281): compact, billing, commands, evidence, remote
/// control and screen reading are not.
#[test]
fn it_reports_nothing_beyond_resume_model_effort_bridge_compaction_threshold_and_quota() {
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
fn without_the_bridge_only_resume_model_effort_the_compaction_threshold_and_quota_are_reported() {
    let codex = Codex::new(
        PathBuf::from("/usr/bin/codex"),
        Some(CONFORMANCE_VERSION.to_owned()),
        None,
    );
    let capabilities = codex.capabilities();

    for capability in Capability::ALL {
        assert_eq!(
            capabilities.supports(capability),
            matches!(
                capability,
                Capability::Resume
                    | Capability::Model
                    | Capability::Effort
                    | Capability::CompactionThreshold
                    | Capability::Quota
            ),
            "{capability:?}"
        );
    }
}
