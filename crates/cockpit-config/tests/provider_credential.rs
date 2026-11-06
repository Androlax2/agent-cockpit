//! #968: a provider's credential for sandboxed sessions, one file per variable the harness
//! reads it from, read by the GitHub token's rules: a wrong file is refused and its contents
//! never reach an error. A fixture value, never a real credential.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cockpit_config::{
    ProviderCredentialError, ProviderCredentialState, TokenFileError, provider_credential_state,
    read_provider_credential,
};

const VARIABLES: [&str; 2] = ["EXAMPLE_API_KEY", "EXAMPLE_OAUTH_TOKEN"];
const VALUE: &str = "sk-example-oat01-fixture_value.0123";

fn credential_file(dir: &Path, variable: &str, contents: &str, mode: u32) {
    let path = dir.join(variable);
    std::fs::write(&path, contents).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn no_file_means_the_provider_has_no_credential() {
    let dir = tempfile::tempdir().unwrap();

    assert!(
        read_provider_credential(dir.path(), &VARIABLES)
            .unwrap()
            .is_none()
    );
}

#[test]
fn the_one_file_present_gives_its_variable_and_value() {
    let dir = tempfile::tempdir().unwrap();
    credential_file(
        dir.path(),
        "EXAMPLE_OAUTH_TOKEN",
        &format!("{VALUE}\n"),
        0o600,
    );

    let credential = read_provider_credential(dir.path(), &VARIABLES)
        .unwrap()
        .unwrap();

    assert_eq!(credential.variable, "EXAMPLE_OAUTH_TOKEN");
    assert_eq!(credential.expose(), VALUE);
    assert!(!format!("{credential:?}").contains(VALUE));
}

#[test]
fn two_files_present_are_refused_naming_both() {
    let dir = tempfile::tempdir().unwrap();
    for variable in VARIABLES {
        credential_file(dir.path(), variable, VALUE, 0o600);
    }

    let error = read_provider_credential(dir.path(), &VARIABLES).unwrap_err();

    assert!(
        matches!(&error, ProviderCredentialError::MoreThanOne { paths } if paths.len() == 2),
        "{error:?}"
    );
    assert!(!error.to_string().contains(VALUE), "{error}");
}

#[test]
fn a_file_others_may_read_is_refused_naming_the_fix() {
    let dir = tempfile::tempdir().unwrap();
    credential_file(dir.path(), "EXAMPLE_API_KEY", VALUE, 0o644);

    let error = read_provider_credential(dir.path(), &VARIABLES).unwrap_err();

    assert!(
        matches!(
            error,
            ProviderCredentialError::File(TokenFileError::TooOpen { mode: 0o644, .. })
        ),
        "{error:?}"
    );
}

#[test]
fn a_value_a_header_could_not_carry_is_refused_without_showing_it() {
    for contents in ["two words", "line\nsecond", "key:value"] {
        let dir = tempfile::tempdir().unwrap();
        credential_file(dir.path(), "EXAMPLE_API_KEY", contents, 0o600);

        let error = read_provider_credential(dir.path(), &VARIABLES).unwrap_err();

        assert!(
            matches!(
                error,
                ProviderCredentialError::File(TokenFileError::Malformed { .. })
            ),
            "{contents:?}: {error:?}"
        );
        assert!(!error.to_string().contains(contents), "{error}");
    }
}

#[test]
fn a_missing_credential_names_each_file_one_may_go_in() {
    let dir = tempfile::tempdir().unwrap();

    let state = provider_credential_state(dir.path(), &VARIABLES);

    assert_eq!(
        state,
        ProviderCredentialState::Missing {
            files: VARIABLES
                .iter()
                .map(|variable| dir.path().join(variable))
                .collect()
        }
    );
}
