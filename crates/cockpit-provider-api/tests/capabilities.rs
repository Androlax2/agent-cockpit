//! Capabilities as a provider reports them: what it can do, and how Cockpit knows.

use cockpit_provider_api::{Availability, Capabilities, Capability, Provenance};

fn tested() -> Provenance {
    Provenance::Tested {
        version: "2.1.316".to_owned(),
    }
}

#[test]
fn a_capability_tested_against_the_running_version_is_available_with_that_version() {
    let capabilities = Capabilities::none().with(Capability::Resume, tested());

    assert_eq!(
        capabilities.availability(Capability::Resume),
        Availability::Available {
            version: "2.1.316".to_owned()
        }
    );
}

#[test]
fn a_capability_only_discovered_is_unverified_but_supported() {
    let capabilities = Capabilities::none().with(Capability::Submit, Provenance::Discovered);

    assert_eq!(
        capabilities.availability(Capability::Submit),
        Availability::Unverified
    );
    assert!(capabilities.supports(Capability::Submit));
}

#[test]
fn a_capability_the_provider_does_not_report_is_unavailable() {
    let capabilities = Capabilities::none().with(Capability::Resume, tested());

    assert_eq!(
        capabilities.availability(Capability::Cancel),
        Availability::Unavailable
    );
    assert!(!capabilities.supports(Capability::Cancel));
}

#[test]
fn reporting_a_capability_again_replaces_its_provenance() {
    let capabilities = Capabilities::none()
        .with(Capability::Effort, Provenance::Discovered)
        .with(Capability::Effort, tested());

    assert_eq!(capabilities.provenance(Capability::Effort), Some(&tested()));
    assert_eq!(capabilities.reported().count(), 1);
}

#[test]
fn requiring_an_unavailable_capability_names_it() {
    let error = Capabilities::none()
        .require(Capability::RemoteControl)
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "it does not report the `remote-control` capability"
    );
}

#[test]
fn every_capability_has_a_distinct_name() {
    let mut names: Vec<&str> = Capability::ALL.iter().map(|c| c.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), Capability::ALL.len());
}
