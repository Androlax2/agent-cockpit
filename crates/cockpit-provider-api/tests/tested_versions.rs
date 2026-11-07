//! An adapter's tested-versions file: which harness versions are `tested` (#732).

use cockpit_provider_api::{Provenance, parse_tested_versions};

const LIST: &str = "\
# Comments and blank lines are ignored.

2.1.316
2.1.318
";

#[test]
fn it_lists_each_version_line_and_skips_comments_and_blank_lines() {
    assert_eq!(parse_tested_versions(LIST).unwrap(), ["2.1.316", "2.1.318"]);
}

#[test]
fn every_listed_version_is_tested_on_itself() {
    for version in ["2.1.316", "2.1.318"] {
        assert_eq!(
            Provenance::for_version(Some(version), LIST),
            Provenance::Tested {
                version: version.to_owned()
            }
        );
    }
}

#[test]
fn a_version_outside_the_list_is_only_discovered() {
    assert_eq!(
        Provenance::for_version(Some("2.1.317"), LIST),
        Provenance::Discovered
    );
}

#[test]
fn a_version_that_only_starts_like_a_listed_one_is_only_discovered() {
    assert_eq!(
        Provenance::for_version(Some("2.1.31"), LIST),
        Provenance::Discovered
    );
}

#[test]
fn an_unknown_version_is_only_discovered() {
    assert_eq!(Provenance::for_version(None, LIST), Provenance::Discovered);
}

#[test]
fn a_list_without_a_version_is_refused() {
    let error = parse_tested_versions("# nothing tested yet\n").unwrap_err();

    assert!(error.contains("no version"), "{error}");
}

#[test]
fn a_line_that_is_not_a_version_is_refused_with_its_line_number() {
    let error = parse_tested_versions("2.1.316\nlatest\n").unwrap_err();

    assert!(
        error.contains("line 2") && error.contains("latest"),
        "{error}"
    );
}

#[test]
fn a_pre_release_version_is_accepted_and_a_dangling_suffix_is_not() {
    assert_eq!(
        parse_tested_versions("0.167.0-alpha.1\n").unwrap(),
        ["0.167.0-alpha.1"]
    );
    for line in ["2.1.316-", "2.1.316--", "2.1.316-.", "v2.1.316", "2"] {
        assert!(parse_tested_versions(line).is_err(), "{line}");
    }
}

#[test]
fn a_version_listed_twice_is_refused() {
    let error = parse_tested_versions("2.1.316\n2.1.318\n2.1.316\n").unwrap_err();

    assert!(
        error.contains("line 3") && error.contains("2.1.316"),
        "{error}"
    );
}
