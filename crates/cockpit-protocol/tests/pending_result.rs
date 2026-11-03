use cockpit_protocol::Command;

#[test]
fn recovery_requires_explicit_observed_revision_and_current_epoch() {
    for json in [
        r#"{"kind":"check-resolve-pending","project":"app","run":"execution-old-1","revision":1}"#,
        r#"{"kind":"check-resolve-pending","project":"app","run":"execution-old-1","database_epoch":"new"}"#,
    ] {
        assert!(serde_json::from_str::<Command>(json).is_err());
    }
    let command = Command::CheckResolvePending {
        project: "app".into(),
        run: "execution-old-1".into(),
        revision: 1,
        database_epoch: "new".into(),
    };
    let encoded = serde_json::to_string(&command).unwrap();
    assert_eq!(serde_json::from_str::<Command>(&encoded).unwrap(), command);
}
