//! Controlled authenticated-route fixtures, not live native admission or draft-safety proof.
use super::*;

fn chief_with_observed_todo() -> (Bench, VerifiedSession) {
    let mut bench = team_bench();
    enable(&mut bench, OPENCODE_SESSION);
    prepare_chief_boundary(&mut bench);
    let chief = verified(&mut bench, OPENCODE_SESSION);
    bench.registry.teams.as_ref().unwrap().execute(
        "INSERT INTO scheduling_reservations VALUES (?1,'observed-todo',?2,'observed-chief-work','observed')",
        (&chief.project, &chief.session_id),
    ).unwrap();
    (bench, chief)
}

#[test]
fn an_admitted_chief_receives_its_automatic_wake_without_releasing_its_observed_todo() {
    let (mut bench, chief) = chief_with_observed_todo();
    let worker = crate::chief::workers(&mut bench.registry)
        .unwrap()
        .into_iter()
        .find(|worker| worker.session == chief)
        .unwrap();
    assert_eq!(worker.blocked_by, ["legacy_work_in_progress"]);
    assert!(
        !worker.ready,
        "generic work dispatch must retain its ownership hold"
    );
    let dispatch = crate::teams::request(&mut bench.registry, 20, AgentAction::Receive)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(dispatch["envelope"]["kind"], "chief_wake", "{dispatch}");
    crate::teams::request(
        &mut bench.registry,
        20,
        AgentAction::Receipt {
            dispatch_id: dispatch["dispatch_id"].as_str().unwrap().into(),
            outcome: cockpit::teams::TransportOutcome::Received {
                native_id: "controlled-chief-wake".into(),
            },
        },
    )
    .unwrap()
    .result
    .unwrap();
    let context = context(&mut bench, 20).unwrap().result.unwrap();
    assert_eq!(context["bootstrap_pending"], false);
    assert!(!context["records"].as_array().unwrap().is_empty());
    let connection = bench.registry.teams.as_ref().unwrap();
    assert_eq!(connection.query_row(
        "SELECT count(*) FROM scheduling_reservations WHERE worker=?1 AND task_id='observed-todo' AND owner='observed'",
        [&chief.session_id], |row|row.get::<_, i64>(0),
    ).unwrap(), 1);
    assert!(cockpit::chief::store::legacy_worker_busy(connection, &chief.session_id).unwrap());
}

#[test]
fn an_observed_chief_todo_never_bypasses_native_focus_or_admission_holds() {
    for hold in [
        "native-busy",
        "native-input",
        "native-stale",
        "viewed",
        "gaming",
        "focus-stale",
        "human-input",
        "admission",
    ] {
        let (mut bench, _) = chief_with_observed_todo();
        match hold {
            "native-busy" => {
                bench
                    .registry
                    .chief_native
                    .get_mut(OPENCODE_SESSION)
                    .unwrap()
                    .1 = false
            }
            "native-input" => {
                bench
                    .registry
                    .chief_native
                    .get_mut(OPENCODE_SESSION)
                    .unwrap()
                    .2 = true
            }
            "native-stale" => {
                bench
                    .registry
                    .chief_native
                    .get_mut(OPENCODE_SESSION)
                    .unwrap()
                    .3 -= 31_000
            }
            "viewed" => {
                bench.registry.chief_focus =
                    Some((OPENCODE_SESSION.into(), true, false, crate::teams::now_ms()))
            }
            "gaming" => bench.registry.chief_focus.as_mut().unwrap().2 = true,
            "focus-stale" => bench.registry.chief_focus.as_mut().unwrap().3 -= 31_000,
            "human-input" => {
                bench
                    .registry
                    .sessions
                    .get_mut(OPENCODE_SESSION)
                    .unwrap()
                    .needs_input = true
            }
            "admission" => {
                bench
                    .registry
                    .teams
                    .as_ref()
                    .unwrap()
                    .execute("DELETE FROM chief_admission_verifications", [])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let dispatch = crate::teams::request(&mut bench.registry, 20, AgentAction::Receive)
            .unwrap()
            .result
            .unwrap();
        assert!(dispatch.is_null(), "{hold}: {dispatch}");
    }
}

#[test]
fn an_observed_chief_todo_keeps_automatic_coordination_working_on_the_next_native_turn() {
    let (mut bench, _) = chief_with_observed_todo();
    let first = crate::teams::request(&mut bench.registry, 20, AgentAction::Receive)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(first["envelope"]["kind"], "chief_wake");
    crate::teams::request(
        &mut bench.registry,
        20,
        AgentAction::Receipt {
            dispatch_id: first["dispatch_id"].as_str().unwrap().into(),
            outcome: cockpit::teams::TransportOutcome::Received {
                native_id: "controlled-first-wake".into(),
            },
        },
    )
    .unwrap()
    .result
    .unwrap();
    let first_context = context(&mut bench, 20).unwrap().result.unwrap();
    assert_eq!(first_context["has_more"], false);

    bench
        .registry
        .teams
        .as_ref()
        .unwrap()
        .execute(
            "UPDATE chief_coordination SET last_admitted_at=last_admitted_at-31000",
            [],
        )
        .unwrap();
    let directory = bench.registry.sessions[SESSION_A].cwd.clone();
    bench.registry.report_from(parse_session(&json!({
        "session":SESSION_A,"agent":"claude","project":"project-a","cwd":directory,"state":"working",
    }).to_string()).unwrap(),10).unwrap();
    prepare_chief_boundary(&mut bench);
    bench
        .registry
        .sessions
        .get_mut(OPENCODE_SESSION)
        .unwrap()
        .since -= 7.0;
    crate::chief::tick(&mut bench.registry, crate::teams::now_ms()).unwrap();

    let second = crate::teams::request(&mut bench.registry, 20, AgentAction::Receive)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(second["envelope"]["kind"], "chief_wake", "{second}");
    assert_ne!(second["message_id"], first["message_id"]);
    crate::teams::request(
        &mut bench.registry,
        20,
        AgentAction::Receipt {
            dispatch_id: second["dispatch_id"].as_str().unwrap().into(),
            outcome: cockpit::teams::TransportOutcome::Received {
                native_id: "controlled-second-wake".into(),
            },
        },
    )
    .unwrap()
    .result
    .unwrap();
    let next_context = crate::teams::request(
        &mut bench.registry,
        20,
        AgentAction::Chief {
            request: ChiefAgentAction::Context {
                after: first_context["event_cursor"].as_i64().unwrap(),
                limit: 50,
            },
        },
    )
    .unwrap()
    .result
    .unwrap();
    assert_eq!(next_context["snapshot_required"], false);
    assert!(
        next_context["next"].as_i64().unwrap() > first_context["event_cursor"].as_i64().unwrap()
    );
    assert_eq!(reservations_of(&bench, "observed-todo"), 1);
}

#[test]
fn an_observed_chief_todo_never_bypasses_real_work_or_launch_reservations() {
    for hold in [
        "legacy",
        "other-project",
        "reserved",
        "submitting",
        "uncertain",
    ] {
        let (mut bench, chief) = chief_with_observed_todo();
        let connection = bench.registry.teams.as_ref().unwrap();
        if matches!(hold, "legacy" | "other-project") {
            let project = if hold == "other-project" {
                "path:/other-project"
            } else {
                &chief.project
            };
            let owner = if hold == "other-project" {
                "observed"
            } else {
                "legacy"
            };
            connection.execute("INSERT INTO scheduling_reservations VALUES (?1,'other-task',?2,'other-work',?3)",
                (project,&chief.session_id,owner)).unwrap();
        }
        if matches!(hold, "reserved" | "submitting" | "uncertain") {
            connection
                .execute(
                    "INSERT INTO legacy_launches VALUES ('other-work',?1,'other-task',?2,?3,1,?4)",
                    (
                        &chief.project,
                        &chief.session_id,
                        hold,
                        crate::teams::now_ms(),
                    ),
                )
                .unwrap();
        }
        let dispatch = crate::teams::request(&mut bench.registry, 20, AgentAction::Receive)
            .unwrap()
            .result
            .unwrap();
        assert!(dispatch.is_null(), "{hold}: {dispatch}");
    }
}
