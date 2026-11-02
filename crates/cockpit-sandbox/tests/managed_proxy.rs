//! A managed proxy refuses missing state without reaching any host service or endpoint.
use cockpit_config::{Environment, Profile, Source, Sourced};
use cockpit_sandbox::{ManagedProxy, Platform};

#[test]
fn missing_configuration_has_a_typed_reason_and_profile_specific_repair() {
    let home = tempfile::tempdir().unwrap();
    let environment = Environment {
        home: Some(home.path().into()),
        profile: Sourced::new(Profile::parse("next").unwrap(), Source::ProductDefault),
        ..Environment::default()
    };
    let proxy = ManagedProxy::new(&environment, &Platform::Linux).unwrap();
    let error = proxy.health().unwrap_err();
    assert_eq!(error.code(), "proxy-not-configured");
    assert!(
        error
            .next(&environment.profile.value)
            .contains("cockpit --profile next sandbox install")
    );
    assert!(
        proxy
            .data_dir()
            .starts_with(home.path().join(".local/state/cockpit-next"))
    );
    assert!(!proxy.data_dir().exists());
}
