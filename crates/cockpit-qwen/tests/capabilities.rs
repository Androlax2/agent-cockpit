//! What the adapter reports, and how it knows: nothing is tested until the live conformance
//! test passes on a version (#1446).

mod common;

use std::path::PathBuf;

use cockpit_provider_api::{
    Availability, Capability, Missing, Probe, Provenance, Provider, SignInStatus,
};
use cockpit_qwen::{CONFORMANCE_VERSION, QwenCode};

const REPORTED: [Capability; 7] = [
    Capability::Resume,
    Capability::Submit,
    Capability::Cancel,
    Capability::Model,
    Capability::Compact,
    Capability::Events,
    Capability::Outcome,
];

#[test]
fn on_the_mapped_version_each_capability_is_discovered() {
    let capabilities = common::qwen().capabilities();

    for capability in REPORTED {
        assert_eq!(
            capabilities.provenance(capability),
            Some(&Provenance::Discovered),
            "{capability:?}"
        );
    }
}

#[test]
fn a_clean_probe_makes_each_capability_probed() {
    let capabilities = common::qwen()
        .with_probe(Probe::new(CONFORMANCE_VERSION, Vec::new()))
        .capabilities();

    for capability in REPORTED {
        assert_eq!(
            capabilities.provenance(capability),
            Some(&Provenance::Probed {
                version: CONFORMANCE_VERSION.to_owned()
            }),
            "{capability:?}"
        );
    }
}

#[test]
fn a_probe_missing_a_launch_flag_withholds_every_capability() {
    let missing = Missing {
        requirement: "flag --session-id in `qwen --help`".to_owned(),
        blocks: Capability::ALL.to_vec(),
    };
    let capabilities = common::qwen()
        .with_probe(Probe::new(CONFORMANCE_VERSION, vec![missing]))
        .capabilities();

    for capability in REPORTED {
        assert!(!capabilities.supports(capability), "{capability:?}");
    }
}

#[test]
fn what_qwen_has_no_surface_for_is_not_reported() {
    let capabilities = common::qwen().capabilities();

    for capability in [
        Capability::Effort,
        Capability::CompactionThreshold,
        Capability::RemoteControl,
        Capability::ScreenReading,
        Capability::Quota,
        Capability::Cost,
        Capability::Commands,
        Capability::Inbox,
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
fn without_its_bridge_it_reports_no_events_or_outcomes() {
    let capabilities = QwenCode::new(
        PathBuf::from("/usr/bin/qwen"),
        Some(CONFORMANCE_VERSION.to_owned()),
        None,
    )
    .capabilities();

    for capability in [Capability::Events, Capability::Outcome] {
        assert!(!capabilities.supports(capability), "{capability:?}");
    }
    assert!(capabilities.supports(Capability::Resume));
}

#[test]
fn it_reports_at_launch_only_through_its_hook_bridge() {
    let without_bridge = QwenCode::new(PathBuf::from("/usr/bin/qwen"), None, None);

    assert!(common::qwen().reports_at_launch());
    assert!(!without_bridge.reports_at_launch());
}

#[test]
fn the_adapter_names_itself_qwen_with_its_running_version() {
    let qwen = common::qwen();

    assert_eq!(qwen.name().as_str(), "qwen");
    assert_eq!(qwen.version(), Some(CONFORMANCE_VERSION));
}

#[test]
fn the_tested_versions_file_lists_no_version_yet() {
    assert_eq!(
        Provenance::for_version(Some(CONFORMANCE_VERSION), cockpit_qwen::TESTED_VERSIONS),
        Provenance::Discovered
    );
    assert!(cockpit_provider_api::parse_tested_versions(cockpit_qwen::TESTED_VERSIONS).is_err());
}

/// #1446 Q3: Qwen Code has no command that reports its sign-in.
#[test]
fn its_sign_in_is_unknown_with_the_owner_s_next_step() {
    let sign_in = common::qwen().sign_in();

    assert_eq!(
        sign_in.status,
        SignInStatus::Unknown {
            reason: "Qwen Code has no command that reports its sign-in".to_owned(),
            next: "start qwen and run /auth, or set the provider key".to_owned(),
        }
    );
}
