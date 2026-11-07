//! The runtime probe of an OpenCode version not on the tested list, against a fake `opencode`
//! (#732).

mod common;

use cockpit_opencode::{LAUNCH_FLAGS, OPENCODE_VARIABLES, OpenCode, probe};
use cockpit_provider_api::{Availability, Capability, Provider};

const NEW_VERSION: &str = "9.9.9";

/// An `opencode` whose `--help` (on standard error, as OpenCode prints it) documents
/// `LAUNCH_FLAGS` less `without_flags`, and whose file holds `OPENCODE_VARIABLES` less
/// `without_variables`.
fn fake_opencode(
    dir: &std::path::Path,
    without_flags: &[&str],
    without_variables: &[&str],
) -> std::path::PathBuf {
    let help: String = LAUNCH_FLAGS
        .into_iter()
        .filter(|flag| !without_flags.contains(flag))
        .map(|flag| format!("  {flag} <value>  x\\n"))
        .collect();
    let variables: Vec<&str> = OPENCODE_VARIABLES
        .into_iter()
        .filter(|name| !without_variables.contains(name))
        .collect();
    common::script(
        dir,
        "opencode",
        &format!(
            "printf '{help}  --brand-new-flag  y\\n' >&2\n# {}",
            variables.join(" ")
        ),
    )
}

fn adapter(program: std::path::PathBuf, found: cockpit_provider_api::Probe) -> OpenCode {
    OpenCode::new(
        program,
        Some(NEW_VERSION.to_owned()),
        Some(common::bridge()),
    )
    .with_probe(found)
}

#[test]
fn a_harness_with_every_requirement_and_an_extra_flag_passes() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_opencode(dir.path(), &[], &[]);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(found.is_clean(), "{}", found.summary());
    let capabilities = adapter(program, found).capabilities();
    for (capability, _) in capabilities.reported() {
        assert_eq!(
            capabilities.availability(capability),
            Availability::Probed {
                version: NEW_VERSION.to_owned()
            },
            "{capability:?}"
        );
    }
}

#[test]
fn a_harness_missing_a_flag_reports_every_capability_unavailable_and_names_it() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_opencode(dir.path(), &["--session"], &[]);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(found.summary().contains("--session"), "{}", found.summary());
    let capabilities = adapter(program, found).capabilities();
    assert_eq!(
        capabilities.availability(Capability::Resume),
        Availability::Unavailable
    );
}

#[test]
fn a_variable_the_program_no_longer_reads_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let lost = OPENCODE_VARIABLES[0];
    let program = fake_opencode(dir.path(), &[], &[lost]);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(found.summary().contains(lost), "{}", found.summary());
}

#[test]
fn a_harness_that_cannot_be_asked_is_an_error_not_a_failed_probe() {
    let error = probe(std::path::Path::new("/nonexistent/opencode"), NEW_VERSION).unwrap_err();

    assert!(error.reason.contains("/nonexistent/opencode"), "{error}");
}
