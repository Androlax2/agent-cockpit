//! The runtime probe's findings and the helpers adapters probe with (#732).

use std::time::Duration;

use cockpit_provider_api::{
    Availability, Capabilities, Capability, Missing, Probe, Provenance, command_output,
    documents_flag, names_missing_from_file,
};

const LIST: &str = "2.1.316\n";

fn missing_effort() -> Probe {
    Probe::new(
        "2.2.0",
        vec![Missing {
            requirement: "flag --effort in `claude --help`".to_owned(),
            blocks: vec![Capability::Effort],
        }],
    )
}

#[test]
fn a_listed_version_is_tested_whatever_a_probe_says() {
    let probe = Probe::new("2.1.316", vec![]);

    assert_eq!(
        Provenance::for_probe(Some("2.1.316"), LIST, Some(&probe), Capability::Effort),
        Some(Provenance::Tested {
            version: "2.1.316".to_owned()
        })
    );
}

#[test]
fn a_clean_probe_of_an_unlisted_version_reports_probed() {
    let probe = Probe::new("2.2.0", vec![]);

    assert_eq!(
        Provenance::for_probe(Some("2.2.0"), LIST, Some(&probe), Capability::Resume),
        Some(Provenance::Probed {
            version: "2.2.0".to_owned()
        })
    );
}

#[test]
fn a_missing_requirement_takes_away_only_the_capabilities_that_need_it() {
    let probe = missing_effort();

    assert_eq!(
        Provenance::for_probe(Some("2.2.0"), LIST, Some(&probe), Capability::Effort),
        None
    );
    assert!(Provenance::for_probe(Some("2.2.0"), LIST, Some(&probe), Capability::Model).is_some());
}

#[test]
fn a_probe_of_another_version_or_no_probe_leaves_the_version_discovered() {
    let probe = missing_effort();

    for (version, probe) in [(Some("2.2.1"), Some(&probe)), (Some("2.2.0"), None)] {
        assert_eq!(
            Provenance::for_probe(version, LIST, probe, Capability::Effort),
            Some(Provenance::Discovered)
        );
    }
    assert_eq!(
        Provenance::for_probe(None, LIST, Some(&probe), Capability::Effort),
        Some(Provenance::Discovered)
    );
}

#[test]
fn a_probed_capability_is_available_as_probed_not_as_tested() {
    let capabilities = Capabilities::none().with(
        Capability::Resume,
        Provenance::Probed {
            version: "2.2.0".to_owned(),
        },
    );

    assert_eq!(
        capabilities.availability(Capability::Resume),
        Availability::Probed {
            version: "2.2.0".to_owned()
        }
    );
}

#[test]
fn the_summary_names_each_missing_requirement_and_what_it_takes_away() {
    let summary = missing_effort().summary();

    assert!(summary.contains("2.2.0"), "{summary}");
    assert!(
        summary.contains("flag --effort in `claude --help`"),
        "{summary}"
    );
    assert!(summary.contains("effort"), "{summary}");
}

#[test]
fn a_clean_probe_summarizes_as_everything_found() {
    assert!(
        Probe::new("2.2.0", vec![])
            .summary()
            .contains("every required")
    );
}

#[test]
fn a_flag_is_documented_only_as_a_word_of_its_own() {
    let help = "  --effort <level>  how hard\n  --effort-extra  other\n  --model, -m  pick\n";

    assert!(documents_flag(help, "--effort"));
    assert!(documents_flag(help, "--model"));
    assert!(!documents_flag(help, "--mod"));
    assert!(!documents_flag(help, "--extra"));
    assert!(!documents_flag("only --effort-extra here", "--effort"));
}

#[test]
fn a_command_s_output_includes_standard_error() {
    let dir = tempfile::tempdir().unwrap();
    let program = script(dir.path(), "echo out; echo err >&2");

    let output = command_output(&program, &["--help"], Duration::from_secs(5)).unwrap();

    assert!(output.contains("out") && output.contains("err"), "{output}");
}

#[test]
fn a_command_that_never_ends_is_given_up_on() {
    let dir = tempfile::tempdir().unwrap();
    let program = script(dir.path(), "sleep 30");

    let error = command_output(&program, &["--help"], Duration::from_millis(200)).unwrap_err();

    assert!(error.reason.contains("did not answer"), "{error}");
}

#[test]
fn a_program_that_cannot_run_is_an_error_naming_it() {
    let error = command_output(
        std::path::Path::new("/nonexistent/harness"),
        &["--help"],
        Duration::from_secs(1),
    )
    .unwrap_err();

    assert!(error.reason.contains("/nonexistent/harness"), "{error}");
}

#[test]
fn names_missing_from_a_file_are_those_it_does_not_contain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("binary");
    std::fs::write(&path, b"\0\0SessionStart\0CLAUDE_CODE_DISABLE_AGENT_VIEW\0").unwrap();

    let missing = names_missing_from_file(
        &path,
        &[
            "SessionStart",
            "SessionEnd",
            "CLAUDE_CODE_DISABLE_AGENT_VIEW",
        ],
    )
    .unwrap();

    assert_eq!(missing, ["SessionEnd"]);
}

#[test]
fn a_name_split_across_two_chunks_is_still_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("binary");
    let chunk = 1 << 20;
    let mut bytes = vec![b'x'; chunk - 6];
    bytes.extend_from_slice(b"\0SessionStart\0");
    bytes.extend(vec![b'x'; 100]);
    std::fs::write(&path, bytes).unwrap();

    assert!(
        names_missing_from_file(&path, &["SessionStart"])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_name_inside_a_longer_name_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("binary");
    std::fs::write(
        &path,
        b"\0StopFailure\0SessionStart_extra\0\"Notification\"",
    )
    .unwrap();

    let missing = names_missing_from_file(
        &path,
        &["Stop", "StopFailure", "SessionStart", "Notification"],
    )
    .unwrap();

    assert_eq!(missing, ["Stop", "SessionStart"]);
}

#[test]
fn a_name_that_ends_the_file_is_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("binary");
    std::fs::write(&path, b"xx Stop").unwrap();

    assert!(
        names_missing_from_file(&path, &["Stop"])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_command_printing_more_than_the_cap_still_ends() {
    let dir = tempfile::tempdir().unwrap();
    let program = script(
        dir.path(),
        "head -c 3000000 /dev/zero | tr '\\0' x; echo done",
    );

    let output = command_output(&program, &["--help"], Duration::from_secs(10)).unwrap();

    assert!(output.len() >= 1 << 20);
}

#[test]
fn an_unreadable_file_is_an_error_not_every_name_missing() {
    let error =
        names_missing_from_file(std::path::Path::new("/nonexistent/binary"), &["a"]).unwrap_err();

    assert!(error.reason.contains("/nonexistent/binary"), "{error}");
}

fn script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("harness");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}
