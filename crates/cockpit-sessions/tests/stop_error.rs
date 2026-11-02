use cockpit_processes::ExecutionId;
use cockpit_sessions::{Error, SessionId};

#[test]
fn an_unconfirmed_stop_does_not_claim_a_signal_was_sent() {
    let error = Error::StopUnconfirmed {
        session: SessionId::parse("s000000000001").unwrap(),
        execution: ExecutionId(42),
    };
    let message = error.to_string();
    assert!(message.contains("s000000000001") && message.contains("execution 42"));
    assert!(message.contains("keeps it"));
    assert!(!message.contains("SIGKILL"), "{message}");
    assert!(!message.contains("still has processes"), "{message}");
}
