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
            "prune":{"kind":"kept","reason":"checkout is occupied"}}],"truncated":false,
            "session_temps":[{"session":"s1","path":"/state/tmp/s1","measurement":measurement}]});
        let reply: Reply = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(reply).unwrap(), value);
    }
}

#[test]
fn a_clone_row_says_whether_prune_would_accept_it() {
    for prune in [
        json!({"kind":"eligible", "confirmation_required":true}),
        json!({"kind":"kept", "reason":"the clone has unpushed commits"}),
    ] {
        let value = json!({"kind":"storage", "clones":[{"checkout":2,"project":"app","incarnation":1,
            "target":"/p/.parallel/app/feature/target","holder":null,
            "measurement":{"kind":"complete", "apparent_bytes":8},"prune":prune}],"truncated":false});
        let reply: Reply = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(reply).unwrap(), value);
    }
}

#[test]
fn a_clone_row_from_a_daemon_without_verdicts_still_reads() {
    let value = json!({"kind":"storage", "clones":[{"checkout":2,"project":"app","incarnation":1,
        "target":"/p/.parallel/app/feature/target","holder":null,
        "measurement":{"kind":"missing"}}],"truncated":false});
    let reply: Reply = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(reply).unwrap(), value);
}

#[test]
fn output_retention_is_one_command_whose_only_argument_is_apply() {
    for apply in [false, true] {
        let value = json!({"kind":"storage-outputs", "apply":apply});
        let command: Command = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(command, Command::StorageOutputs { apply });
        assert_eq!(serde_json::to_value(command).unwrap(), value);
    }
}

#[test]
fn an_output_retention_row_separates_protected_unmeasured_and_selected_outputs() {
    let value = json!({"kind":"storage-outputs", "projects":[{"project":"app","outputs":7,
        "apparent_bytes":4096,"unmeasured":1,"protected":2,"selected":3,"selected_bytes":1024,
        "retired":3,"retired_bytes":1000,"failures":["execution 9: permission denied"]}],
        "applied":true,"truncated":false});
    let reply: Reply = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(reply).unwrap(), value);
}

#[test]
fn database_retention_is_one_command_whose_only_argument_is_apply() {
    for apply in [false, true] {
        let value = json!({"kind":"storage-database", "apply":apply});
        let command: Command = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(command, Command::StorageDatabase { apply });
        assert_eq!(serde_json::to_value(command).unwrap(), value);
    }
}

#[test]
fn a_database_report_names_each_rule_the_feed_and_the_backups() {
    let rule = json!({"selected":2,"selected_bytes":300,"retired":0,"retired_bytes":0});
    let value = json!({"kind":"storage-database", "database":{"file_bytes":4096,"wal_bytes":0,
        "retention_days":30,"sessions":rule,"run_outputs":rule,"feed_changes":12,
        "feed_changes_kept":100000,"backups":[{"path":"/s/backups/state-9.db","taken_at":9,
        "bytes":4096}],"backups_kept":7,"truncated":false,"wal_checkpoint_busy":false,"failures":[]},"applied":false});
    let reply: Reply = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(reply).unwrap(), value);
}
