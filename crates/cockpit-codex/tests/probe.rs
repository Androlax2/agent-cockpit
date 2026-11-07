//! The runtime probe of a Codex version not on the tested list, against a fake `codex` (#732).

mod common;

use cockpit_codex::{
    COMPACTION_THRESHOLD_KEY, CONFIG_FLAG, Codex, HOOKS_FEATURE, LAUNCH_FLAGS, SERVER_FLAGS,
    TUI_FLAGS, probe,
};
use cockpit_provider_api::{Availability, Capability, Provider};

const NEW_VERSION: &str = "9.9.9";

/// A `codex` documenting the launch and server flags it is given, less those in `without`, and
/// listing the hooks feature unless `hooks` is false. It knows the compaction threshold key
/// (refusing a word for it by name, as Codex does) unless the key is in `without`. The
/// app-server keeps `--config` whatever `without` says, so the TUI's own flag is probed alone.
fn fake_codex(dir: &std::path::Path, without: &[&str], hooks: bool) -> std::path::PathBuf {
    let documented = |flags: Vec<&str>, without: &[&str]| -> String {
        flags
            .into_iter()
            .filter(|flag| !without.contains(flag))
            .map(|flag| format!("  {flag} <value>  x\\n"))
            .collect()
    };
    let launch = documented(
        LAUNCH_FLAGS
            .into_iter()
            .chain(TUI_FLAGS)
            .chain([CONFIG_FLAG])
            .collect(),
        without,
    );
    let server_without: Vec<&str> = without
        .iter()
        .copied()
        .filter(|flag| *flag != CONFIG_FLAG)
        .collect();
    let server = documented(SERVER_FLAGS.to_vec(), &server_without);
    let features = if hooks {
        format!("{HOOKS_FEATURE}   stable   true\\nother   stable   false\\n")
    } else {
        "other   stable   false\\n".to_owned()
    };
    let typed_refusal = if without.contains(&COMPACTION_THRESHOLD_KEY) {
        features.clone()
    } else {
        format!(
            "Error: failed to load bootstrap configuration\\n\\nCaused by:\\n    invalid type: \
             string \"torlek-probe\", expected u8\\n    in `{COMPACTION_THRESHOLD_KEY}`\\n"
        )
    };
    common::script(
        dir,
        "codex",
        &format!(
            "case \"$*\" in\n\
             'app-server --help') printf '{server}' ;;\n\
             'features list') printf '{features}' ;;\n\
             '{CONFIG_FLAG} {COMPACTION_THRESHOLD_KEY}='*' features list') printf '{typed_refusal}' >&2 ;;\n\
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
fn a_harness_that_does_not_know_the_compaction_threshold_key_refuses_a_threshold_by_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_codex(dir.path(), &[COMPACTION_THRESHOLD_KEY], true);
    let found = probe(&program, NEW_VERSION).unwrap();
    let codex = adapter(program, found);

    let capabilities = codex.capabilities();
    assert_eq!(
        capabilities.availability(Capability::CompactionThreshold),
        Availability::Unavailable
    );
    assert_eq!(
        capabilities.availability(Capability::Resume),
        Availability::Probed {
            version: NEW_VERSION.to_owned()
        }
    );
    let refusal = codex
        .launch(&cockpit_provider_api::LaunchRequest {
            working_dir: dir.path(),
            session_dir: dir.path(),
            conversation: &cockpit_provider_api::Conversation::New,
            first_message: None,
            effort: None,
            model: None,
            remote_control: false,
            compaction_threshold: Some("75"),
        })
        .unwrap_err();
    assert_eq!(
        refusal.reason,
        "it does not report the `compaction-threshold` capability: key \
         model_post_turn_compact_threshold_percent in `codex --config`, refused by name when \
         not a percent is missing in 9.9.9"
    );
}

#[test]
fn a_harness_whose_tui_lacks_the_config_flag_loses_only_the_compaction_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let program = fake_codex(dir.path(), &[CONFIG_FLAG], true);

    let found = probe(&program, NEW_VERSION).unwrap();

    assert!(
        found.summary().contains("flag --config in `codex --help`"),
        "{}",
        found.summary()
    );
    let capabilities = adapter(program, found).capabilities();
    assert!(capabilities.supports(Capability::Resume));
    assert!(!capabilities.supports(Capability::CompactionThreshold));
}

#[test]
fn a_harness_that_cannot_be_asked_is_an_error_not_a_failed_probe() {
    let error = probe(std::path::Path::new("/nonexistent/codex"), NEW_VERSION).unwrap_err();

    assert!(error.reason.contains("/nonexistent/codex"), "{error}");
}
