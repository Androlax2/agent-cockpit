//! The runtime probe of a Codex version not on the tested list, against a fake `codex` (#732).

mod common;

use cockpit_codex::{Codex, HOOKS_FEATURE, LAUNCH_FLAGS, SERVER_FLAGS, TUI_FLAGS, probe};
use cockpit_provider_api::{Availability, Capability, Provider};

const NEW_VERSION: &str = "9.9.9";

/// A `codex` documenting the launch and server flags it is given, less those in `without`, and
/// listing the hooks feature unless `hooks` is false.
fn fake_codex(dir: &std::path::Path, without: &[&str], hooks: bool) -> std::path::PathBuf {
    let documented = |flags: Vec<&str>| -> String {
        flags
            .into_iter()
            .filter(|flag| !without.contains(flag))
            .map(|flag| format!("  {flag} <value>  x\\n"))
            .collect()
    };
    let launch = documented(LAUNCH_FLAGS.into_iter().chain(TUI_FLAGS).collect());
    let server = documented(SERVER_FLAGS.to_vec());
    let features = if hooks {
        format!("{HOOKS_FEATURE}   stable   true\\nother   stable   false\\n")
    } else {
        "other   stable   false\\n".to_owned()
    };
    common::script(
        dir,
        "codex",
        &format!(
            "case \"$*\" in\n\
             'app-server --help') printf '{server}' ;;\n\
             'features list') printf '{features}' ;;\n\
             *) printf '{launch}  --brand-new-flag  y\\n' ;;\n\
             esac"
        ),
    )
}

fn adapter(program: std::path::PathBuf, found: cockpit_provider_api::Probe) -> Codex {
    Codex::new(
        program,
        Some(NEW_VERSION.to_owned()),
        Some(common::bridge()),
    )
    .with_probe(found)
}

#[test]
fn a_harness_with_every_requirement_and_an_extra_flag_passes() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_codex(dir.path(), &[], true);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(found.is_clean(), "{}", found.summary());
    let capabilities = adapter(program, found).capabilities();
    assert_eq!(
        capabilities.availability(Capability::Resume),
        Availability::Probed {
            version: NEW_VERSION.to_owned()
        }
    );
}

#[test]
fn a_harness_missing_a_launch_flag_reports_every_capability_unavailable_and_names_it() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_codex(dir.path(), &["--remote"], true);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(found.summary().contains("--remote"), "{}", found.summary());
    assert!(
        adapter(program, found)
            .capabilities()
            .reported()
            .next()
            .is_none()
    );
}

#[test]
fn a_missing_server_flag_is_named_with_the_command_that_should_list_it() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_codex(dir.path(), &["--listen"], true);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(
        found
            .summary()
            .contains("--listen in `codex app-server --help`"),
        "{}",
        found.summary()
    );
}

#[test]
fn a_harness_without_the_hooks_feature_keeps_resume_and_loses_what_the_bridge_adds() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_codex(dir.path(), &[], false);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(
        found.summary().contains(HOOKS_FEATURE),
        "{}",
        found.summary()
    );
    let capabilities = adapter(program, found).capabilities();
    assert!(capabilities.supports(Capability::Resume));
    for lost in [
        Capability::Events,
        Capability::Submit,
        Capability::Cancel,
        Capability::Outcome,
    ] {
        assert_eq!(capabilities.availability(lost), Availability::Unavailable);
    }
}

#[test]
fn a_harness_that_cannot_be_asked_is_an_error_not_a_failed_probe() {
    let error = probe(std::path::Path::new("/nonexistent/codex"), NEW_VERSION).unwrap_err();

    assert!(error.reason.contains("/nonexistent/codex"), "{error}");
}
