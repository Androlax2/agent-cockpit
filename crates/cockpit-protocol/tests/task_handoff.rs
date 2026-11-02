//! The owner handoff is additive; older callers retain their task-start representation.
use cockpit_protocol::Command;
use serde_json::json;

#[test]
fn handoff_roundtrips_the_complete_displayed_identity() {
    let value = json!({"kind":"task-handoff", "item":7, "intent_revision":2,
        "state_revision":3, "session":"s0123456789ab", "expected_launch":4});
    let command: Command = serde_json::from_value(value.clone()).unwrap();
    assert!(matches!(
        command,
        Command::TaskHandoff {
            item: 7,
            intent_revision: 2,
            state_revision: 3,
            expected_launch: 4,
            ..
        }
    ));
    assert_eq!(serde_json::to_value(command).unwrap(), value);
}

#[test]
fn an_incomplete_handoff_identity_is_not_a_request() {
    let value = json!({"kind":"task-handoff", "item":7, "intent_revision":2,
        "state_revision":3, "session":"s0123456789ab", "expected_launch":4});
    for field in [
        "item",
        "intent_revision",
        "state_revision",
        "session",
        "expected_launch",
    ] {
        let mut incomplete = value.clone();
        incomplete.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<Command>(incomplete).is_err(),
            "{field} must be present"
        );
    }
}

#[test]
fn existing_task_start_callers_keep_their_wire_contract() {
    let value = json!({"kind":"task-start", "item":7});
    let command: Command = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(
        command,
        Command::TaskStart {
            launch_token: None,
            item: 7
        }
    );
    assert_eq!(serde_json::to_value(command).unwrap(), value);
}
