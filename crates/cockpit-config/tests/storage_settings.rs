//! Owner-only pruning policy and provenance, with no activation of live settings.
use cockpit_config::{ConfigError, Environment};
fn environment(home: &std::path::Path) -> Environment {
    Environment {
        home: Some(home.into()),
        ..Environment::default()
    }
}
fn owner(home: &std::path::Path, body: &str) {
    std::fs::create_dir_all(home.join(".config/torlek")).unwrap();
    std::fs::write(home.join(".config/torlek/config.toml"), body).unwrap();
}
#[test]
fn storage_defaults_are_inert_and_report_each_threshold() {
    let home = tempfile::tempdir().unwrap();
    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();
    let shown = cockpit_config::render(&config);
    for line in [
        "storage.target_prune_automatic = false  # product default",
        "storage.target_prune_projects = []  # product default",
        "storage.target_prune_confirm_above_mib = 1024  # product default",
        "storage.target_prune_interval_minutes = 60  # product default",
        "storage.target_prune_base_ref = <unset>  # product default",
        "storage.sweep_on_session_end = true  # product default",
    ] {
        assert!(shown.contains(line), "missing {line}: {shown}");
    }
}
#[test]
fn storage_owner_policy_is_loaded_with_source_and_project_cannot_enable_it() {
    let home = tempfile::tempdir().unwrap();
    owner(
        home.path(),
        "[storage]\ntarget_prune_automatic = true\ntarget_prune_projects = ['app']\ntarget_prune_confirm_above_mib = 2\ntarget_prune_interval_minutes = 1\ntarget_prune_base_ref = 'refs/heads/main'\n",
    );
    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();
    let shown = cockpit_config::render(&config);
    assert!(
        shown.contains("storage.target_prune_automatic = true  # user config"),
        "{shown}"
    );
    assert!(shown.contains("config.toml:2"), "{shown}");
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".torlek")).unwrap();
    std::fs::write(
        project.path().join(".torlek/config.toml"),
        "[storage]\ntarget_prune_automatic = true\n",
    )
    .unwrap();
    assert!(matches!(
        cockpit_config::load(&environment(home.path()), Some(project.path()), &[]),
        Err(ConfigError::OwnerOnly { key: "storage", .. })
    ));
}
#[test]
fn storage_invalid_bounds_and_ambiguous_base_ref_are_rejected() {
    let home = tempfile::tempdir().unwrap();
    for setting in [
        "target_prune_confirm_above_mib = 0",
        "target_prune_confirm_above_mib = -1",
        "target_prune_confirm_above_mib = 18446744073709551615",
        "target_prune_interval_minutes = 0",
        "target_prune_base_ref = 'main'",
        "target_prune_projects = ['../outside']",
    ] {
        owner(home.path(), &format!("[storage]\n{setting}\n"));
        assert!(
            cockpit_config::load(&environment(home.path()), None, &[]).is_err(),
            "{setting}"
        );
    }
}
#[test]
fn output_retention_defaults_are_inert_and_reported() {
    let home = tempfile::tempdir().unwrap();
    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();
    let shown = cockpit_config::render(&config);
    for line in [
        "storage.output_retention_days = 14  # product default",
        "storage.output_budget_mib = 1024  # product default",
        "storage.output_budget_grace_hours = 24  # product default",
        "storage.output_prune_automatic = false  # product default",
        "storage.output_prune_projects = []  # product default",
        "storage.output_prune_interval_minutes = 60  # product default",
    ] {
        assert!(shown.contains(line), "missing {line}: {shown}");
    }
}
#[test]
fn output_retention_owner_policy_is_loaded_with_its_source() {
    let home = tempfile::tempdir().unwrap();
    owner(
        home.path(),
        "[storage]\noutput_retention_days = 3\noutput_budget_mib = 5\noutput_budget_grace_hours = 0\noutput_prune_automatic = true\noutput_prune_projects = ['app']\noutput_prune_interval_minutes = 10\n",
    );
    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();
    let storage = &config.storage;
    assert_eq!(
        (
            storage.output_retention_days.value,
            storage.output_budget_mib.value,
            storage.output_budget_grace_hours.value,
            storage.output_prune_automatic.value,
            storage.output_prune_projects.value.clone(),
            storage.output_prune_interval_minutes.value,
        ),
        (3, 5, 0, true, vec!["app".to_owned()], 10)
    );
}
#[test]
fn output_retention_invalid_bounds_are_rejected() {
    let home = tempfile::tempdir().unwrap();
    for setting in [
        "output_retention_days = 0",
        "output_budget_mib = 0",
        "output_budget_mib = 18446744073709551615",
        "output_prune_interval_minutes = 0",
        "output_prune_projects = ['../outside']",
    ] {
        owner(home.path(), &format!("[storage]\n{setting}\n"));
        assert!(
            cockpit_config::load(&environment(home.path()), None, &[]).is_err(),
            "{setting}"
        );
    }
}
#[test]
fn storage_owner_turns_the_session_end_sweep_off() {
    let home = tempfile::tempdir().unwrap();
    owner(home.path(), "[storage]\nsweep_on_session_end = false\n");
    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();
    assert!(!config.storage.sweep_on_session_end.value);
    let shown = cockpit_config::render(&config);
    assert!(
        shown.contains("storage.sweep_on_session_end = false  # user config"),
        "{shown}"
    );
}
