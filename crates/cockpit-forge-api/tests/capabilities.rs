//! What an adapter reports it can do, and the typed refusal for what it cannot.

use cockpit_forge_api::{Availability, Capabilities, Capability, ForgeError, Provenance};

fn tested() -> Provenance {
    Provenance::Tested {
        against: "program 1.0".to_owned(),
    }
}

#[test]
fn a_tested_capability_is_available_against_its_version() {
    let capabilities = Capabilities::none().with(Capability::IssueRead, tested());

    assert_eq!(
        capabilities.availability(Capability::IssueRead),
        Availability::Available {
            against: "program 1.0".to_owned()
        }
    );
}

#[test]
fn a_discovered_capability_is_unverified() {
    let capabilities = Capabilities::none().with(Capability::IssueRead, Provenance::Discovered);

    assert_eq!(
        capabilities.availability(Capability::IssueRead),
        Availability::Unverified
    );
    assert!(capabilities.require(Capability::IssueRead).is_ok());
}

#[test]
fn a_capability_not_reported_is_refused_with_the_typed_answer() {
    let error = Capabilities::none()
        .require(Capability::ChangeChecks)
        .unwrap_err();

    assert_eq!(
        error,
        ForgeError::Unsupported {
            capability: Capability::ChangeChecks
        }
    );
    assert!(error.is_unsupported());
    assert_eq!(
        error.to_string(),
        "the forge adapter does not report the `change-checks` capability"
    );
}

#[test]
fn a_withheld_capability_is_refused_with_its_reason() {
    let capabilities = Capabilities::none()
        .with(Capability::ChangeReview, tested())
        .withholding(
            Capability::ChangeReview,
            "the program is too old".to_owned(),
        );

    let error = capabilities.require(Capability::ChangeReview).unwrap_err();

    assert!(error.is_unsupported());
    assert_eq!(
        capabilities.availability(Capability::ChangeReview),
        Availability::Unavailable
    );
    assert!(error.to_string().ends_with(": the program is too old"));
}

#[test]
fn a_failed_call_is_not_an_unsupported_answer() {
    let failures = [
        ForgeError::NotFound {
            what: "issue 1".to_owned(),
        },
        ForgeError::Refused {
            reason: "no".to_owned(),
        },
        ForgeError::Unavailable {
            reason: "timeout".to_owned(),
        },
        ForgeError::Malformed {
            reason: "not JSON".to_owned(),
        },
    ];

    for failure in failures {
        assert!(!failure.is_unsupported(), "{failure:?}");
    }
}

#[test]
fn every_capability_has_a_distinct_name() {
    let mut names: Vec<&str> = Capability::ALL
        .iter()
        .map(|capability| capability.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();

    assert_eq!(names.len(), Capability::ALL.len());
}
