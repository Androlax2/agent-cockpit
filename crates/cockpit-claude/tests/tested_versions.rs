//! The Claude Code tested-versions file the adapter reads (#732).

use cockpit_claude::{CONFORMANCE_VERSION, TESTED_VERSIONS};
use cockpit_provider_api::parse_tested_versions;

#[test]
fn the_tested_versions_file_is_well_formed() {
    parse_tested_versions(TESTED_VERSIONS).unwrap();
}

#[test]
fn the_fixture_version_is_a_tested_version() {
    let versions = parse_tested_versions(TESTED_VERSIONS).unwrap();

    assert!(versions.contains(&CONFORMANCE_VERSION), "{versions:?}");
}
