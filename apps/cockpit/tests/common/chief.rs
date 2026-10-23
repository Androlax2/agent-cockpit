#![allow(dead_code)]

use cockpit::chief::{store, WorkerState, GateKind, GateRequirement, WorkBrief, WorkRef};
use cockpit::teams::{TransportOutcome, VerifiedSession, Role, TaskKey};
use rusqlite::Connection;
use serde_json::json;

pub fn work() -> WorkRef {
    WorkRef { task: TaskKey { project: "project-a".into(), task_id: "1d3fd318".into() }, revision: 1 }
}

pub fn brief() -> WorkBrief {
    WorkBrief { work: work(), brief: "Implement the approved dispatcher contract.".into(), role: Role::Developer,
        prerequisites: vec![], gates: vec![GateRequirement { kind: GateKind::Tests, source: "host_run".into(),
            expected: json!({"commit":"1111111111111111111111111111111111111111", "command_fingerprint":"1111111111111111111111111111111111111111111111111111111111111111", "cwd":"/protected/reviewed"}) }] }
}

pub fn session() -> VerifiedSession {
    VerifiedSession { session_id: "aaaaaaaa-0000-4000-8000-000000000000".into(), agent: "claude".into(),
        project: "project-a".into(), runtime: "launch-123:pid-456:start-789".into() }
}

pub fn admit(connection: &mut Connection, live: &[WorkerState], chief: &VerifiedSession, now_ms: i64) {
    store::verify_admission(connection,chief,"opencode-queued-v1","Controlled test fixture, not a live support claim",now_ms).unwrap();
    store::observe(connection,live,now_ms).unwrap();
    let epoch: String = connection.query_row("SELECT epoch FROM chief_grants WHERE state='active'",[],|row|row.get(0)).unwrap();
    let dispatch = store::claim(connection,chief,&format!("bootstrap-{epoch}"),now_ms).unwrap().expect("verified native chief bootstrap");
    assert_eq!(dispatch.envelope["kind"],"chief_wake");
    store::finish_for(connection,chief,&dispatch.id,&TransportOutcome::Received {native_id:format!("native-{epoch}")},now_ms).unwrap();
}
