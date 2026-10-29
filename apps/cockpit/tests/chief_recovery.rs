use cockpit::chief::{ChiefAgentAction, ChiefHostAction, WorkerState, store};
use cockpit::teams::{ErrorCode, VerifiedSession};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
mod common;

const NOW: i64 = 1_000_000;

fn session(runtime: &str) -> VerifiedSession {
    VerifiedSession {
        session_id: "ses_chief000000000000000000000".into(),
        agent: "opencode".into(),
        project: "path:/project-a".into(),
        runtime: runtime.into(),
    }
}

fn live(session: &VerifiedSession) -> WorkerState {
    WorkerState {
        session: session.clone(),
        role: None,
        ready: true,
        blocked_by: vec![],
        observed_at_ms: NOW,
    }
}

fn fixture() -> (Connection, String, VerifiedSession) {
    let mut connection = Connection::open_in_memory().unwrap();
    cockpit::teams::store::migrate(&mut connection).unwrap();
    let old = session("boot-a:10:100");
    let grant = store::host(
        &mut connection,
        &ChiefHostAction::Enable {
            request_id: "enable".into(),
            session: old.clone(),
        },
        &[live(&old)],
        NOW - 100,
    )
    .unwrap()
    .result
    .unwrap();
    let replacement = session("boot-b:20:200");
    store::verify_admission(
        &mut connection,
        &replacement,
        "opencode-queued-v1",
        "Controlled fixture, not a live verification",
        NOW,
    )
    .unwrap();
    (
        connection,
        grant["epoch"].as_str().unwrap().into(),
        replacement,
    )
}

fn request(
    epoch: &str,
    revision: i64,
    replacement: &VerifiedSession,
    key: &str,
) -> ChiefHostAction {
    serde_json::from_value(json!({"action":"recover","request_id":key,"expected_epoch":epoch,"expected_revision":revision,"session":replacement})).expect("Recover protocol")
}

fn recover(
    connection: &mut Connection,
    action: &ChiefHostAction,
    replacement: &VerifiedSession,
) -> Result<Value, cockpit::teams::TeamError> {
    store::host(connection, action, &[live(replacement)], NOW)
        .unwrap()
        .result
}

#[test]
fn recovery_replaces_the_grant_without_rewriting_its_approved_identity() {
    let (mut connection, epoch, replacement) = fixture();
    let action = request(&epoch, 1, &replacement, "recover");
    let result = recover(&mut connection, &action, &replacement).unwrap();
    assert_eq!(result["previous_epoch"], epoch);
    assert_ne!(result["epoch"], epoch);
    assert_eq!(result["session"], json!(replacement));
    assert_eq!(result["state"], "active");
    let original: (String, String, i64) = connection
        .query_row(
            "SELECT runtime,state,revision FROM chief_grants WHERE epoch=?1",
            [&epoch],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(original, ("boot-a:10:100".into(), "disabled".into(), 2));
    let hash: String = connection
        .query_row(
            "SELECT approval_hash FROM chief_grants WHERE state='active'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        hash,
        cockpit::audit::sha256_hex(&serde_json::to_vec(&action).unwrap())
    );
    assert!(
        connection
            .execute(
                "UPDATE chief_grants SET runtime='forged' WHERE epoch=?1",
                [&epoch]
            )
            .is_err()
    );
    let event: String = connection
        .query_row(
            "SELECT body FROM chief_events WHERE kind='recovered'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let event: Value = serde_json::from_str(&event).unwrap();
    assert_eq!(event["previous_session"]["runtime"], "boot-a:10:100");
    assert_eq!(event["session"], json!(replacement));
    assert_eq!(event["previous_epoch"], epoch);
    assert_eq!(event["epoch"], result["epoch"]);
    assert_eq!(event["request_id"], "recover");
}

#[test]
fn recovering_a_paused_chief_does_not_resume_it_or_queue_a_new_wake() {
    let (mut connection, epoch, replacement) = fixture();
    store::host(
        &mut connection,
        &ChiefHostAction::Pause {
            request_id: "pause".into(),
            expected_epoch: epoch.clone(),
        },
        &[],
        NOW,
    )
    .unwrap()
    .result
    .unwrap();
    let result = recover(
        &mut connection,
        &request(&epoch, 2, &replacement, "recover"),
        &replacement,
    )
    .unwrap();
    assert_eq!(result["state"], "paused");
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM chief_inbox WHERE chief_epoch=?1",
                [result["epoch"].as_str().unwrap()],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert!(
        store::agent(
            &mut connection,
            &replacement,
            &ChiefAgentAction::Context {
                after: 0,
                limit: 50
            },
            &[live(&replacement)],
            NOW
        )
        .unwrap()
        .result
        .is_err()
    );
}

#[test]
fn recovery_requires_the_exact_retained_grant_and_same_conversation() {
    for change in [
        "epoch",
        "revision",
        "disabled",
        "session",
        "agent",
        "project",
        "same-runtime",
        "not-live",
        "team",
    ] {
        let (mut connection, epoch, mut replacement) = fixture();
        let mut expected_epoch = epoch.clone();
        let mut revision = 1;
        match change {
            "epoch" => expected_epoch = "unknown".into(),
            "revision" => revision = 2,
            "disabled" => {
                connection
                    .execute("UPDATE chief_grants SET state='disabled'", [])
                    .unwrap();
            }
            "session" => replacement.session_id = "ses_other000000000000000000000".into(),
            "agent" => {
                replacement.agent = "claude".into();
                replacement.session_id = "aaaaaaaa-0000-4000-8000-000000000000".into();
            }
            "project" => replacement.project = "path:/project-b".into(),
            "same-runtime" => replacement.runtime = "boot-a:10:100".into(),
            "team" => {
                connection
                    .execute(
                        "INSERT INTO teams VALUES ('task','path:/project-a','aaaaaaaa',1)",
                        [],
                    )
                    .unwrap();
                connection.execute("INSERT INTO members VALUES ('task',?1,'developer','opencode',?2,'member-epoch',NULL)", params![replacement.session_id,replacement.runtime]).unwrap();
            }
            _ => {}
        }
        store::verify_admission(
            &mut connection,
            &replacement,
            "opencode-queued-v1",
            "Fixture",
            NOW,
        )
        .unwrap();
        let workers = if change == "not-live" {
            vec![]
        } else {
            vec![live(&replacement)]
        };
        let result = store::host(
            &mut connection,
            &request(&expected_epoch, revision, &replacement, "recover"),
            &workers,
            NOW,
        )
        .unwrap()
        .result;
        assert!(result.is_err(), "{change}: {result:?}");
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM chief_grants", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1,
            "{change}"
        );
    }
}

#[test]
fn recovery_requires_recent_admission_of_the_exact_new_runtime() {
    for (case, verified_at) in [
        ("missing", NOW),
        ("old-runtime", NOW),
        ("other-session", NOW),
        ("wrong-adapter", NOW),
        ("expired", NOW - 300_001),
        ("future", NOW + 1),
        ("boundary", NOW - 300_000),
    ] {
        let (mut connection, epoch, replacement) = fixture();
        connection
            .execute("DELETE FROM chief_admission_verifications", [])
            .unwrap();
        let mut verified = replacement.clone();
        if case == "old-runtime" {
            verified.runtime = "boot-a:10:100".into();
        }
        if case == "other-session" {
            verified.session_id = "ses_other000000000000000000000".into();
        }
        if case != "missing" {
            store::verify_admission(
                &mut connection,
                &verified,
                if case == "wrong-adapter" {
                    "other"
                } else {
                    "opencode-queued-v1"
                },
                "Fixture",
                verified_at,
            )
            .unwrap();
        }
        let result = recover(
            &mut connection,
            &request(&epoch, 1, &replacement, "recover"),
            &replacement,
        );
        assert_eq!(result.is_ok(), case == "boundary", "{case}: {result:?}");
    }
}

#[test]
fn recovery_replay_is_only_a_receipt_and_cannot_reactivate_a_disabled_replacement() {
    let (mut connection, epoch, replacement) = fixture();
    let action = request(&epoch, 1, &replacement, "recover");
    let result = recover(&mut connection, &action, &replacement).unwrap();
    connection
        .execute("UPDATE chief_grants SET state='disabled'", [])
        .unwrap();
    connection
        .execute("DELETE FROM chief_admission_verifications", [])
        .unwrap();
    assert_eq!(
        store::host(&mut connection, &action, &[], NOW + 600_000)
            .unwrap()
            .result
            .unwrap(),
        result
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM chief_grants WHERE state!='disabled'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let retargeted = request(&epoch, 2, &replacement, "recover");
    assert_eq!(
        recover(&mut connection, &retargeted, &replacement)
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
}

#[test]
fn recovery_rolls_back_retirement_and_delivery_cleanup_if_new_grant_creation_fails() {
    let (mut connection, epoch, replacement) = fixture();
    connection.execute_batch("CREATE TRIGGER fail_recovery BEFORE INSERT ON chief_grants BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        recover(
            &mut connection,
            &request(&epoch, 1, &replacement, "recover"),
            &replacement
        )
        .is_err()
    );
    assert_eq!(
        connection
            .query_row("SELECT state FROM chief_grants", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "active"
    );
    assert_eq!(
        connection
            .query_row("SELECT state FROM chief_inbox", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "queued"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM chief_actions WHERE request_id='recover'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn recovery_preserves_uncertain_deliveries_and_never_retargets_them() {
    let (mut connection, epoch, replacement) = fixture();
    let old_identity = serde_json::to_string(&session("boot-a:10:100")).unwrap();
    for (index, state) in ["queued", "submitting", "uncertain"]
        .into_iter()
        .enumerate()
    {
        let id = format!("message-{index}");
        let envelope = json!({"message_id":id,"kind":"chief_message"});
        connection.execute("INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at) VALUES (?1,?2,?3,?4,?2,'null',?5,?6,0)", params![id,epoch,format!("worker-{index}"),old_identity,envelope.to_string(),state]).unwrap();
        if state != "queued" {
            connection.execute("INSERT INTO chief_delivery_attempts(id,recipient,recipient_runtime,recipient_epoch,kind,envelope,state,started_at) VALUES (?1,?2,'boot-a:10:100',?3,'chief_message',?4,?5,0)", params![id,format!("worker-{index}"),epoch,envelope.to_string(),state]).unwrap();
        }
    }
    recover(
        &mut connection,
        &request(&epoch, 1, &replacement, "recover"),
        &replacement,
    )
    .unwrap();
    for (index, expected) in ["cancelled", "submitting", "uncertain"]
        .into_iter()
        .enumerate()
    {
        let (state, identity, stored_epoch): (String, String, String) = connection
            .query_row(
                "SELECT state,recipient_identity,chief_epoch FROM chief_inbox WHERE id=?1",
                [format!("message-{index}")],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, expected);
        assert_eq!(identity, old_identity);
        assert_eq!(stored_epoch, epoch);
    }
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_delivery_attempts WHERE state IN ('submitting','uncertain') AND recipient_runtime='boot-a:10:100'", [], |row| row.get::<_,i64>(0)).unwrap(), 2);
}

#[test]
fn concurrent_recovery_approvals_create_only_one_replacement() {
    let sandbox = common::sandbox::Sandbox::new();
    let path = sandbox.home.join("protected/recovery.db");
    let replacement = session("boot-b:20:200");
    let mut connection = cockpit::teams::store::open(&path).unwrap();
    let old = session("boot-a:10:100");
    let grant = store::host(
        &mut connection,
        &ChiefHostAction::Enable {
            request_id: "enable".into(),
            session: old.clone(),
        },
        &[live(&old)],
        NOW,
    )
    .unwrap()
    .result
    .unwrap();
    let epoch = grant["epoch"].as_str().unwrap().to_string();
    store::verify_admission(
        &mut connection,
        &replacement,
        "opencode-queued-v1",
        "Fixture",
        NOW,
    )
    .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let clients: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|key| {
            let mut connection = cockpit::teams::store::open(&path).unwrap();
            let barrier = barrier.clone();
            let replacement = replacement.clone();
            let epoch = epoch.clone();
            std::thread::spawn(move || {
                barrier.wait();
                recover(
                    &mut connection,
                    &request(&epoch, 1, &replacement, key),
                    &replacement,
                )
                .is_ok()
            })
        })
        .collect();
    assert_eq!(
        clients
            .into_iter()
            .filter_map(|client| client.join().unwrap().then_some(()))
            .count(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM chief_grants", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn agents_and_the_phone_cannot_submit_a_recovery_action() {
    let action = json!({"action":"recover","request_id":"recover","expected_epoch":"epoch","expected_revision":1,"session":session("new-boot:20:200")});
    assert!(
        serde_json::from_value::<cockpit::teams::AgentAction>(
            json!({"action":"chief","request":action})
        )
        .is_err()
    );
    let error = cockpit::remote::chief_action(json!({"action":action})).unwrap_err();
    assert_eq!(error["outcome_uncertain"], false);
    assert_eq!(
        error["message"],
        "This Chief action is not available from the phone"
    );
}

#[test]
fn recovery_requires_a_new_native_bootstrap_instead_of_inheriting_the_old_turn() {
    let (mut connection, epoch, replacement) = fixture();
    let old = session("boot-a:10:100");
    common::chief::admit(&mut connection, &[live(&old)], &old, NOW);
    let status = ChiefAgentAction::Status {
        summary: "Fixture status, not permission".into(),
    };
    store::agent(&mut connection, &old, &status, &[live(&old)], NOW)
        .unwrap()
        .result
        .unwrap();
    recover(
        &mut connection,
        &request(&epoch, 1, &replacement, "recover"),
        &replacement,
    )
    .unwrap();
    assert!(
        store::agent(
            &mut connection,
            &old,
            &ChiefAgentAction::Context {
                after: 0,
                limit: 50
            },
            &[live(&replacement)],
            NOW
        )
        .unwrap()
        .result
        .is_err()
    );
    assert!(
        store::agent(
            &mut connection,
            &replacement,
            &status,
            &[live(&replacement)],
            NOW
        )
        .unwrap()
        .result
        .is_err()
    );
    common::chief::admit(&mut connection, &[live(&replacement)], &replacement, NOW);
    store::agent(
        &mut connection,
        &replacement,
        &status,
        &[live(&replacement)],
        NOW,
    )
    .unwrap()
    .result
    .unwrap();
}

#[test]
fn recovery_retires_pending_requests_but_preserves_started_and_reserved_launches() {
    let (mut connection, epoch, replacement) = fixture();
    for (id, state, token) in [
        ("pending", "requested", None),
        ("started", "started", Some("reserved-token")),
        ("reserved", "requested", Some("uncertain-token")),
    ] {
        connection.execute("INSERT INTO chief_session_starts(id,epoch,project,task_id,reason,state,created_at,token) VALUES (?1,?2,'path:/project-a','aaaaaaaa','Fixture request',?3,?4,?5)",params![id,epoch,state,NOW,token]).unwrap();
    }
    let task = cockpit::teams::TaskKey {
        project: "path:/project-a".into(),
        task_id: "bbbbbbbb".into(),
    };
    let token = store::reserve_legacy(&mut connection, &task, None, NOW).unwrap();
    let before: Value =
        store::snapshot(&connection, &[live(&replacement)], 0, 50).unwrap()["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["kind"] == "legacy_launch")
            .unwrap()
            .clone();
    recover(
        &mut connection,
        &request(&epoch, 1, &replacement, "recover"),
        &replacement,
    )
    .unwrap();
    let states: Vec<(String, String)> = connection
        .prepare("SELECT id,state FROM chief_session_starts ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        states,
        vec![
            ("pending".into(), "expired".into()),
            ("reserved".into(), "expired".into()),
            ("started".into(), "started".into())
        ]
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT token FROM chief_session_starts WHERE id='reserved'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "uncertain-token"
    );
    let after = store::snapshot(&connection, &[live(&replacement)], 0, 50).unwrap();
    let after = after["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "legacy_launch" && row["id"] == token)
        .unwrap();
    assert_eq!(*after, before);
}

/// A retained grant for `boot-a` whose runtime is gone, without any verification of the new runtime.
fn restarted() -> (Connection, String, VerifiedSession) {
    let (connection, epoch, replacement) = fixture();
    connection
        .execute("DELETE FROM chief_admission_verifications", [])
        .unwrap();
    (connection, epoch, replacement)
}

fn recovery_check(connection: &Connection, workers: &[WorkerState], now_ms: i64) -> Value {
    store::snapshot_page(connection, workers, 0, 50, None, now_ms).unwrap()["recovery_check"]
        .clone()
}

fn send_check(
    connection: &mut Connection,
    attempt: &str,
    workers: &[WorkerState],
    key: &str,
) -> Result<Value, cockpit::teams::TeamError> {
    store::host(
        connection,
        &ChiefHostAction::SendCheck {
            request_id: key.into(),
            attempt: attempt.into(),
        },
        workers,
        NOW,
    )
    .unwrap()
    .result
}

#[test]
fn a_restarted_chief_gets_a_test_message_whose_confirmation_lets_recovery_through() {
    let (mut connection, epoch, replacement) = restarted();
    let shown = recovery_check(&connection, &[live(&replacement)], NOW);
    assert_eq!(shown["id"], epoch);
    assert_eq!(shown["check_session"], json!(replacement));
    assert!(
        shown["check"].is_null() && shown["verified_at_ms"].is_null(),
        "{shown}"
    );
    let check =
        send_check(&mut connection, &epoch, &[live(&replacement)], "check-1").unwrap()["check"]
            .as_str()
            .unwrap()
            .to_string();
    assert_eq!(
        send_check(&mut connection, &epoch, &[live(&replacement)], "check-2").unwrap()["check"],
        check,
        "a pending check is never sent twice"
    );
    let dispatch = store::claim_check(&mut connection, &replacement, "idle-1", NOW)
        .unwrap()
        .expect("the new runtime takes its check");
    assert_eq!(dispatch.envelope["kind"], "chief_check");
    store::finish_for(
        &mut connection,
        &replacement,
        &dispatch.id,
        &cockpit::teams::TransportOutcome::Received {
            native_id: format!("msg_cockpit{}", dispatch.message_id),
        },
        NOW,
    )
    .unwrap();
    assert_eq!(
        recovery_check(&connection, &[live(&replacement)], NOW)["check"]["state"],
        "received"
    );
    assert!(
        recover(
            &mut connection,
            &request(&epoch, 1, &replacement, "early"),
            &replacement
        )
        .is_err(),
        "a received check alone is not Theo's confirmation"
    );
    store::verify_admission(
        &mut connection,
        &replacement,
        "opencode-queued-v1",
        "Theo saw the delivery check arrive",
        NOW,
    )
    .unwrap();
    assert_eq!(
        recovery_check(&connection, &[live(&replacement)], NOW)["verified_at_ms"],
        NOW
    );
    recover(
        &mut connection,
        &request(&epoch, 1, &replacement, "recover"),
        &replacement,
    )
    .unwrap();
    assert!(
        recovery_check(&connection, &[live(&replacement)], NOW).is_null(),
        "a recovered Chief needs no check"
    );
}

#[test]
fn a_recovery_check_is_offered_again_once_its_verification_expired() {
    let (mut connection, epoch, replacement) = restarted();
    store::verify_admission(
        &mut connection,
        &replacement,
        "opencode-queued-v1",
        "Fixture",
        NOW - 300_001,
    )
    .unwrap();
    let shown = recovery_check(&connection, &[live(&replacement)], NOW);
    assert!(
        shown["verified_at_ms"].is_null() && shown["check"].is_null(),
        "{shown}"
    );
    assert!(send_check(&mut connection, &epoch, &[live(&replacement)], "check").is_ok());
    store::verify_admission(
        &mut connection,
        &replacement,
        "opencode-queued-v1",
        "Fixture",
        NOW,
    )
    .unwrap();
    assert!(
        send_check(&mut connection, &epoch, &[live(&replacement)], "fresh").is_err(),
        "a fresh verification needs no second check"
    );
}

#[test]
fn no_recovery_check_without_exactly_one_new_runtime_of_the_retained_conversation() {
    for case in [
        "old-runtime-live",
        "none-live",
        "other-session",
        "other-project",
        "two-new",
        "team",
        "disabled",
        "startup-attempt",
    ] {
        let (mut connection, epoch, replacement) = restarted();
        let mut workers = vec![live(&replacement)];
        let mut attempt = epoch.clone();
        match case {
            "old-runtime-live" => workers.push(live(&session("boot-a:10:100"))),
            "none-live" => workers.clear(),
            "other-session" => {
                workers[0].session.session_id = "ses_other000000000000000000000".into()
            }
            "other-project" => workers[0].session.project = "path:/project-b".into(),
            "two-new" => workers.push(live(&session("boot-c:30:300"))),
            "team" => {
                connection
                    .execute(
                        "INSERT INTO teams VALUES ('task','path:/project-a','aaaaaaaa',1)",
                        [],
                    )
                    .unwrap();
                connection.execute("INSERT INTO members VALUES ('task',?1,'developer','opencode',?2,'member-epoch',NULL)", params![replacement.session_id,replacement.runtime]).unwrap();
            }
            "disabled" => {
                connection
                    .execute("UPDATE chief_grants SET state='disabled'", [])
                    .unwrap();
            }
            "startup-attempt" => attempt = "another-attempt".into(),
            _ => unreachable!(),
        }
        if case != "startup-attempt" {
            assert!(
                recovery_check(&connection, &workers, NOW).is_null(),
                "{case}"
            );
        }
        assert!(
            send_check(&mut connection, &attempt, &workers, "check").is_err(),
            "{case}"
        );
        assert_eq!(connection.query_row("SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_check'", [], |row| row.get::<_,i64>(0)).unwrap(), 0, "{case}");
    }
}

/// A retained grant for `boot-a` in a database on disk, with a check queued for `boot-b`.
fn check_queued_on_disk(
    sandbox: &common::sandbox::Sandbox,
) -> (std::path::PathBuf, String, VerifiedSession) {
    let path = sandbox.home.join("protected/check.db");
    let mut connection = cockpit::teams::store::open(&path).unwrap();
    let old = session("boot-a:10:100");
    let grant = store::host(
        &mut connection,
        &ChiefHostAction::Enable {
            request_id: "enable".into(),
            session: old.clone(),
        },
        &[live(&old)],
        NOW - 100,
    )
    .unwrap()
    .result
    .unwrap();
    let replacement = session("boot-b:20:200");
    let check = send_check(
        &mut connection,
        grant["epoch"].as_str().unwrap(),
        &[live(&replacement)],
        "check",
    )
    .unwrap()["check"]
        .as_str()
        .unwrap()
        .to_string();
    (path, check, replacement)
}

#[test]
fn a_queued_recovery_check_survives_reopening_the_store_and_is_received() {
    // Every open cleared old dispatch leftovers and cancelled pending checks with them (b4329b6f).
    let sandbox = common::sandbox::Sandbox::new();
    let (path, check, replacement) = check_queued_on_disk(&sandbox);
    let mut reopened = cockpit::teams::store::open(&path).unwrap();
    let dispatch = store::claim_check(&mut reopened, &replacement, "idle-1", NOW)
        .unwrap()
        .expect("the check is still queued after a reopen");
    assert_eq!(dispatch.message_id, check);
    store::finish_for(
        &mut reopened,
        &replacement,
        &dispatch.id,
        &cockpit::teams::TransportOutcome::Received {
            native_id: format!("msg_cockpit{}", dispatch.message_id),
        },
        NOW,
    )
    .unwrap();
    assert_eq!(
        recovery_check(&reopened, &[live(&replacement)], NOW)["check"]["state"],
        "received"
    );
}

#[test]
fn a_recovery_check_in_flight_during_a_reopen_still_records_its_receipt() {
    let sandbox = common::sandbox::Sandbox::new();
    let (path, _, replacement) = check_queued_on_disk(&sandbox);
    let mut connection = cockpit::teams::store::open(&path).unwrap();
    let dispatch = store::claim_check(&mut connection, &replacement, "idle-1", NOW)
        .unwrap()
        .unwrap();
    let mut reopened = cockpit::teams::store::open(&path).unwrap();
    store::finish_for(
        &mut reopened,
        &replacement,
        &dispatch.id,
        &cockpit::teams::TransportOutcome::Received {
            native_id: format!("msg_cockpit{}", dispatch.message_id),
        },
        NOW,
    )
    .unwrap();
    assert_eq!(
        recovery_check(&reopened, &[live(&replacement)], NOW)["check"]["state"],
        "received"
    );
}

#[test]
fn a_recovery_check_reaches_only_the_new_runtime_it_names() {
    for recipient in [session("boot-a:10:100"), session("boot-c:30:300")] {
        let (mut connection, epoch, replacement) = restarted();
        send_check(&mut connection, &epoch, &[live(&replacement)], "check").unwrap();
        assert!(
            store::claim_check(&mut connection, &recipient, "idle-1", NOW)
                .unwrap()
                .is_none(),
            "{recipient:?}"
        );
        assert!(
            store::claim_check(&mut connection, &replacement, "idle-1", NOW)
                .unwrap()
                .is_none(),
            "a check offered to another runtime is cancelled, never redirected"
        );
    }
}
