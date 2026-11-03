//! Pruning binds registered identities and exact displayed plans; report wire remains intact.
use cockpit_protocol::Command;
use serde_json::json;
#[test]
fn storage_prune_commands_preserve_explicit_intent() {
    for value in [
        json!({"kind":"storage-prune-preview","checkout":2,"prepare":false}),
        json!({"kind":"storage-prune-preview","checkout":2,"prepare":true}),
        json!({"kind":"storage-prune-apply","plan":7,"revision":1,"epoch":"displayed-epoch"}),
        json!({"kind":"storage-prune-status","operation":7}),
        json!({"kind":"storage-prune-abandon","operation":7,"revision":1,"epoch":"current-database-epoch"}),
    ] {
        let command: Command = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(command).unwrap(), value);
    }
}

#[test]
fn storage_prune_abandon_requires_explicit_revision_and_current_database_epoch() {
    for value in [
        json!({"kind":"storage-prune-abandon","operation":7}),
        json!({"kind":"storage-prune-abandon","operation":7,"revision":1}),
        json!({"kind":"storage-prune-abandon","path":"/tmp/target","revision":1,"epoch":"current"}),
    ] {
        assert!(serde_json::from_value::<Command>(value).is_err());
    }
}
#[test]
fn storage_prune_apply_requires_the_revision_and_never_accepts_a_path_as_identity() {
    for value in [
        json!({"kind":"storage-prune-apply","plan":7}),
        json!({"kind":"storage-prune-apply","path":"/tmp/target","revision":1}),
        json!({"kind":"storage-prune-preview","path":"/tmp/target","prepare":false}),
    ] {
        assert!(serde_json::from_value::<Command>(value).is_err());
    }
}

#[test]
fn storage_prune_apply_requires_the_displayed_database_epoch() {
    assert!(
        serde_json::from_value::<Command>(json!({
            "kind":"storage-prune-apply","plan":7,"revision":1
        }))
        .is_err(),
        "a numeric plan can be reused after database restore"
    );
}
