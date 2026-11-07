//! The runtime probe of a Claude Code version not on the tested list, against a fake `claude`
//! (#732).

mod common;

use cockpit_claude::{ClaudeCode, LAUNCH_ENVIRONMENT, LAUNCH_FLAGS, hooks, probe};
use cockpit_provider_api::{Availability, Capability, Provider};

const NEW_VERSION: &str = "9.9.9";

/// A `claude` whose `--help` documents `flags` and whose file holds the launch environment
/// names and the hook events, less those in `without`.
fn fake_claude(dir: &std::path::Path, flags: &[&str], without: &[&str]) -> std::path::PathBuf {
    let names: Vec<&str> = LAUNCH_ENVIRONMENT
        .iter()
        .map(|(name, _)| *name)
        .chain(hooks::HOOK_EVENTS)
        .filter(|name| !without.contains(name))
        .collect();
    let help: String = flags
        .iter()
        .map(|flag| format!("  {flag} <value>  x\n"))
        .collect();
    common::script(
        dir,
        "claude",
        &format!(
            "cat <<'HELP'\nUsage: claude [options]\n{help}HELP\n# {}",
            names.join(" ")
        ),
    )
}

fn everything_but(flag: &str) -> Vec<&'static str> {
    LAUNCH_FLAGS
        .into_iter()
        .filter(|each| *each != flag)
        .collect()
}

fn adapter(
    program: std::path::PathBuf,
    version: &str,
    found: cockpit_provider_api::Probe,
) -> ClaudeCode {
    ClaudeCode::new(program, Some(version.to_owned()), Some(common::bridge())).with_probe(found)
}

#[test]
fn a_harness_with_every_requirement_and_an_extra_flag_passes() {
    let dir = tempfile::tempdir().unwrap();
    let mut flags = LAUNCH_FLAGS.to_vec();
    flags.push("--brand-new-flag");
    let program = fake_claude(dir.path(), &flags, &[]);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(found.is_clean(), "{}", found.summary());
    let capabilities = adapter(program, NEW_VERSION, found).capabilities();
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
fn a_harness_missing_a_flag_reports_that_capability_unavailable_and_names_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), &everything_but("--effort"), &[]);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(found.summary().contains("--effort"), "{}", found.summary());
    let capabilities = adapter(program, NEW_VERSION, found).capabilities();
    assert_eq!(
        capabilities.availability(Capability::Effort),
        Availability::Unavailable
    );
    assert_eq!(
        capabilities.availability(Capability::Model),
        Availability::Probed {
            version: NEW_VERSION.to_owned()
        }
    );
}

#[test]
fn a_launch_asking_for_a_capability_the_probe_took_away_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), &everything_but("--effort"), &[]);
    let found = probe(&program, NEW_VERSION).unwrap();
    let claude = adapter(program, NEW_VERSION, found);

    let refusal = claude
        .launch(&cockpit_provider_api::LaunchRequest {
            working_dir: dir.path(),
            session_dir: dir.path(),
            conversation: &cockpit_provider_api::Conversation::New,
            first_message: None,
            effort: Some("high"),
            model: None,
            remote_control: false,
            compaction_threshold: None,
        })
        .unwrap_err();

    assert!(refusal.reason.contains("effort"), "{refusal}");
}

#[test]
fn a_harness_without_autocompact_refuses_a_compaction_threshold_naming_the_flag_and_version() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), &everything_but("--autocompact"), &[]);
    let found = probe(&program, NEW_VERSION).unwrap();
    let claude = adapter(program, NEW_VERSION, found);

    let capabilities = claude.capabilities();
    assert_eq!(
        capabilities.availability(Capability::CompactionThreshold),
        Availability::Unavailable
    );
    assert_eq!(
        capabilities.availability(Capability::Model),
        Availability::Probed {
            version: NEW_VERSION.to_owned()
        }
    );
    let refusal = claude
        .launch(&cockpit_provider_api::LaunchRequest {
            working_dir: dir.path(),
            session_dir: dir.path(),
            conversation: &cockpit_provider_api::Conversation::New,
            first_message: None,
            effort: None,
            model: None,
            remote_control: false,
            compaction_threshold: Some("200k"),
        })
        .unwrap_err();
    assert_eq!(
        refusal.reason,
        "it does not report the `compaction-threshold` capability: flag --autocompact in \
         `claude --help` is missing in 9.9.9"
    );
}

#[test]
fn a_flag_every_launch_passes_takes_every_capability_away() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), &everything_but("--session-id"), &[]);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(found.summary().contains("--session-id"));
    assert!(
        adapter(program, NEW_VERSION, found)
            .capabilities()
            .reported()
            .next()
            .is_none()
    );
}

#[test]
fn each_launch_flag_is_checked() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), &[], &[]);

    let found = probe(&program, NEW_VERSION).unwrap();

    for flag in LAUNCH_FLAGS {
        assert!(
            found.summary().contains(flag),
            "{flag}: {}",
            found.summary()
        );
    }
}

#[test]
fn a_launch_environment_name_the_program_lost_is_named() {
    let dir = tempfile::tempdir().unwrap();
    let lost = LAUNCH_ENVIRONMENT[0].0;
    let program = fake_claude(dir.path(), &LAUNCH_FLAGS, &[lost]);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(found.summary().contains(lost), "{}", found.summary());
    assert!(
        adapter(program, NEW_VERSION, found)
            .capabilities()
            .reported()
            .next()
            .is_none()
    );
}

#[test]
fn a_hook_event_the_program_lost_takes_away_only_what_the_bridge_reports() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), &LAUNCH_FLAGS, &["PostToolUse"]);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(
        found.summary().contains("PostToolUse"),
        "{}",
        found.summary()
    );
    let capabilities = adapter(program, NEW_VERSION, found).capabilities();
    for lost in [Capability::Events, Capability::Outcome, Capability::Inbox] {
        assert_eq!(capabilities.availability(lost), Availability::Unavailable);
    }
    assert!(capabilities.supports(Capability::Submit));
}

#[test]
fn a_listed_version_stays_tested_whatever_a_probe_found() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), &[], &[]);
    let found = probe(&program, cockpit_claude::CONFORMANCE_VERSION).unwrap();

    let capabilities = adapter(program, cockpit_claude::CONFORMANCE_VERSION, found).capabilities();

    assert_eq!(
        capabilities.availability(Capability::Effort),
        Availability::Available {
            version: cockpit_claude::CONFORMANCE_VERSION.to_owned()
        }
    );
}

#[test]
fn a_probe_of_another_version_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_claude(dir.path(), &[], &[]);
    let found = probe(&program, "9.9.8").unwrap();

    let capabilities = adapter(program, NEW_VERSION, found).capabilities();

    assert_eq!(
        capabilities.availability(Capability::Effort),
        Availability::Unverified
    );
}

#[test]
fn a_harness_that_cannot_be_asked_is_an_error_not_a_failed_probe() {
    let error = probe(std::path::Path::new("/nonexistent/claude"), NEW_VERSION).unwrap_err();

    assert!(error.reason.contains("/nonexistent/claude"), "{error}");
}
