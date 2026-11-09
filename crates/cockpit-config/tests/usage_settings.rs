//! The owner's quota settings (#58) and pacing (#412): defaults, provenance and what each
//! refuses.
use cockpit_config::{ConfigError, Environment, Pacing};

fn environment(home: &std::path::Path) -> Environment {
    Environment {
        home: Some(home.into()),
        ..Environment::default()
    }
}

fn write_owner_config(home: &std::path::Path, body: &str) {
    std::fs::create_dir_all(home.join(".config/torlek")).unwrap();
    std::fs::write(home.join(".config/torlek/config.toml"), body).unwrap();
}

#[test]
fn every_quota_setting_has_a_default_shown_with_its_source() {
    let home = tempfile::tempdir().unwrap();

    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();

    assert_eq!(config.usage.quota_alert_percents.value, [80, 95]);
    let shown = cockpit_config::render(&config);
    for line in [
        "usage.quota_refresh_minutes = 5  # product default",
        "usage.quota_fresh_minutes = 10  # product default",
        "usage.quota_alert_percents = [80, 95]  # product default",
        "usage.pacing = \"even\"  # product default",
        "usage.pacing_reserve_percent = 10  # product default",
        "usage.pacing_burst_percent = 15  # product default",
    ] {
        assert!(shown.contains(line), "missing {line}: {shown}");
    }
}

#[test]
fn the_owner_sets_each_quota_setting_and_show_names_the_line() {
    let home = tempfile::tempdir().unwrap();
    write_owner_config(
        home.path(),
        "[usage]\nquota_refresh_minutes = 2\nquota_fresh_minutes = 4\nquota_alert_percents = [50, 90]\n",
    );

    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();

    assert_eq!(config.usage.quota_refresh_minutes.value, 2);
    assert_eq!(config.usage.quota_fresh_minutes.value, 4);
    assert_eq!(config.usage.quota_alert_percents.value, [50, 90]);
    let shown = cockpit_config::render(&config);
    assert!(
        shown.contains("usage.quota_alert_percents = [50, 90]  # user config"),
        "{shown}"
    );
    assert!(shown.contains("config.toml:4"), "{shown}");
}

#[test]
fn a_project_cannot_set_a_quota_setting() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".torlek")).unwrap();
    std::fs::write(
        project.path().join(".torlek/config.toml"),
        "[usage]\nquota_alert_percents = [99]\n",
    )
    .unwrap();

    let loaded = cockpit_config::load(&environment(home.path()), Some(project.path()), &[]);

    assert!(
        matches!(loaded, Err(ConfigError::OwnerOnly { key: "usage", .. })),
        "{loaded:?}"
    );
}

#[test]
fn a_quota_setting_outside_its_rules_is_refused_with_its_key() {
    let home = tempfile::tempdir().unwrap();
    for (setting, key) in [
        ("quota_refresh_minutes = 0", "usage.quota_refresh_minutes"),
        (
            "quota_refresh_minutes = 1441",
            "usage.quota_refresh_minutes",
        ),
        (
            "quota_refresh_minutes = 20\nquota_fresh_minutes = 10",
            "usage.quota_fresh_minutes",
        ),
        ("quota_alert_percents = []", "usage.quota_alert_percents"),
        ("quota_alert_percents = [0]", "usage.quota_alert_percents"),
        ("quota_alert_percents = [101]", "usage.quota_alert_percents"),
        (
            "quota_alert_percents = [95, 80]",
            "usage.quota_alert_percents",
        ),
        (
            "quota_alert_percents = [80, 80]",
            "usage.quota_alert_percents",
        ),
        (
            "quota_alert_percents = [10, 20, 30, 40, 50, 60]",
            "usage.quota_alert_percents",
        ),
        (
            "pacing_reserve_percent = 91",
            "usage.pacing_reserve_percent",
        ),
        ("pacing_burst_percent = 101", "usage.pacing_burst_percent"),
    ] {
        write_owner_config(home.path(), &format!("[usage]\n{setting}\n"));

        let loaded = cockpit_config::load(&environment(home.path()), None, &[]);

        assert!(
            matches!(&loaded, Err(ConfigError::Invalid { key: refused, .. }) if refused == key),
            "{setting}: {loaded:?}"
        );
    }
}

#[test]
fn the_owner_turns_pacing_off_and_sets_its_shares() {
    let home = tempfile::tempdir().unwrap();
    write_owner_config(
        home.path(),
        "[usage]\npacing = \"off\"\npacing_reserve_percent = 20\npacing_burst_percent = 0\n",
    );

    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();

    assert_eq!(config.usage.pacing.value, Pacing::Off);
    assert_eq!(config.usage.pacing_reserve_percent.value, 20);
    assert_eq!(config.usage.pacing_burst_percent.value, 0);
    let shown = cockpit_config::render(&config);
    assert!(
        shown.contains("usage.pacing = \"off\"  # user config"),
        "{shown}"
    );
}

#[test]
fn an_unknown_pacing_is_refused() {
    let home = tempfile::tempdir().unwrap();
    write_owner_config(home.path(), "[usage]\npacing = \"fast\"\n");

    let loaded = cockpit_config::load(&environment(home.path()), None, &[]);

    assert!(loaded.is_err(), "{loaded:?}");
}
