//! The owner's accounts per provider (#59): the order a launch tries them in, where each
//! account's credential files live, and what the list refuses.
use std::path::Path;

use cockpit_config::{ConfigError, DEFAULT_ACCOUNT, Environment};

fn environment(home: &Path) -> Environment {
    Environment {
        home: Some(home.into()),
        ..Environment::default()
    }
}

fn write_owner_config(home: &Path, body: &str) {
    std::fs::create_dir_all(home.join(".config/torlek")).unwrap();
    std::fs::write(home.join(".config/torlek/config.toml"), body).unwrap();
}

#[test]
fn without_a_list_each_provider_has_the_default_account_alone() {
    let home = tempfile::tempdir().unwrap();

    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();

    for provider in cockpit_config::KNOWN_PROVIDERS {
        assert_eq!(
            config.accounts.of(provider),
            [DEFAULT_ACCOUNT],
            "{provider}"
        );
    }
    let shown = cockpit_config::render(&config);
    assert!(
        shown.contains("accounts.claude = [\"default\"]  # product default"),
        "{shown}"
    );
}

#[test]
fn the_owners_list_is_the_order_and_show_names_its_line() {
    let home = tempfile::tempdir().unwrap();
    write_owner_config(
        home.path(),
        "[accounts]\nclaude = [\"work\", \"default\", \"spare-2\"]\n",
    );

    let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();

    assert_eq!(config.accounts.of("claude"), ["work", "default", "spare-2"]);
    assert_eq!(config.accounts.of("codex"), [DEFAULT_ACCOUNT]);
    let shown = cockpit_config::render(&config);
    assert!(
        shown.contains("accounts.claude = [\"work\", \"default\", \"spare-2\"]  # user config"),
        "{shown}"
    );
    assert!(shown.contains("config.toml:2"), "{shown}");
}

#[test]
fn a_project_cannot_list_accounts() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".torlek")).unwrap();
    std::fs::write(
        project.path().join(".torlek/config.toml"),
        "[accounts]\nclaude = [\"theirs\"]\n",
    )
    .unwrap();

    let loaded = cockpit_config::load(&environment(home.path()), Some(project.path()), &[]);

    assert!(
        matches!(
            loaded,
            Err(ConfigError::OwnerOnly {
                key: "accounts",
                ..
            })
        ),
        "{loaded:?}"
    );
}

#[test]
fn a_list_outside_its_rules_is_refused_with_its_key() {
    let home = tempfile::tempdir().unwrap();
    for list in [
        "[]",
        "[\"Work\"]",
        "[\"2nd\"]",
        "[\"work/other\"]",
        "[\"..\"]",
        "[\"\"]",
        "[\"a-name-longer-than-thirty-two-bytes\"]",
        "[\"work\", \"work\"]",
    ] {
        write_owner_config(home.path(), &format!("[accounts]\nclaude = {list}\n"));

        let loaded = cockpit_config::load(&environment(home.path()), None, &[]);

        assert!(
            matches!(&loaded, Err(ConfigError::Invalid { key, .. }) if key == "accounts.claude"),
            "{list}: {loaded:?}"
        );
    }
}

#[test]
fn every_known_provider_can_list_its_accounts() {
    let home = tempfile::tempdir().unwrap();
    for provider in cockpit_config::KNOWN_PROVIDERS {
        write_owner_config(
            home.path(),
            &format!("[accounts]\n{provider} = [\"work\"]\n"),
        );

        let config = cockpit_config::load(&environment(home.path()), None, &[]).unwrap();

        assert_eq!(config.accounts.of(provider), ["work"], "{provider}");
    }
}

#[test]
fn an_unknown_provider_is_refused_with_its_line() {
    let home = tempfile::tempdir().unwrap();
    write_owner_config(home.path(), "[accounts]\nnobody = [\"work\"]\n");

    let loaded = cockpit_config::load(&environment(home.path()), None, &[]);

    assert!(
        matches!(&loaded, Err(ConfigError::Parse { message, .. }) if message.contains("nobody")),
        "{loaded:?}"
    );
}

#[test]
fn the_default_account_keeps_the_providers_own_folder_and_another_has_its_own() {
    let home = tempfile::tempdir().unwrap();
    let environment = environment(home.path());
    let provider_dir = cockpit_config::provider_credentials_dir(&environment, "claude").unwrap();

    assert_eq!(
        cockpit_config::account_credentials_dir(&environment, "claude", DEFAULT_ACCOUNT),
        Some(provider_dir.clone())
    );
    assert_eq!(
        cockpit_config::account_credentials_dir(&environment, "claude", "work"),
        Some(provider_dir.join("work"))
    );
}
