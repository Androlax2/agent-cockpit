//! A read-only storage request and honest distinctions in its measurements.
use cockpit_protocol::{Command, Reply};
use serde_json::json;

#[test]
fn storage_has_no_deletion_or_arbitrary_path_argument() {
    let command: Command = serde_json::from_value(json!({"kind":"storage-report"})).unwrap();
    assert_eq!(
        serde_json::to_value(command).unwrap(),
        json!({"kind":"storage-report"})
    );
    for extra in [
        json!({"kind":"storage-report", "path":"/"}),
        json!({"kind":"storage-report", "prune":true}),
    ] {
        // Like existing commands, unknown fields confer no meaning or authority.
        assert_eq!(
            serde_json::from_value::<Command>(extra).unwrap(),
            Command::StorageReport
        );
    }
}

#[test]
fn an_unavailable_measurement_is_not_a_complete_zero() {
    for measurement in [
        json!({"kind":"complete", "apparent_bytes":0}),
        json!({"kind":"missing"}),
        json!({"kind":"incomplete", "apparent_bytes":8, "reason":"entry limit"}),
        json!({"kind":"unavailable", "reason":"permission denied"}),
    ] {
        let value = json!({"kind":"storage", "clones":[{"checkout":2,"project":"app","incarnation":1,
            "target":"/p/.parallel/app/feature/target","holder":null,"measurement":measurement,
            "prune":{"kind":"kept","reason":"checkout is occupied"}}],"truncated":false});
        let reply: Reply = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(reply).unwrap(), value);
    }
}

#[test]
fn a_clone_row_says_whether_prune_would_accept_it() {
    for prune in [
        json!({"kind":"eligible", "proof":"observed ended sessions: 4", "confirmation_required":true}),
        json!({"kind":"kept", "reason":"the clone has unpushed commits"}),
    ] {
        let value = json!({"kind":"storage", "clones":[{"checkout":2,"project":"app","incarnation":1,
            "target":"/p/.parallel/app/feature/target","holder":null,
            "measurement":{"kind":"complete", "apparent_bytes":8},"prune":prune}],"truncated":false});
        let reply: Reply = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(reply).unwrap(), value);
    }
}
