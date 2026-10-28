//! The Chief following a known session or todo through the authenticated daemon route after its
//! 50-record Context budget is spent (todo 5b91edcf, reproducing the 5779dac9 stall).
use super::*;

fn inspect(bench: &mut Bench, peer: u32, target: serde_json::Value) -> Result<serde_json::Value, cockpit::teams::TeamError> {
    let mut request = json!({"action":"inspect"});
    request.as_object_mut().unwrap().extend(target.as_object().unwrap().clone());
    let request: ChiefAgentAction = serde_json::from_value(request).unwrap();
    crate::teams::request(&mut bench.registry, peer, AgentAction::Chief { request }).and_then(|reply| reply.result)
}

/// More open host decisions than one turn may read, then the one Context read that spends the budget.
fn spend_context_budget(bench: &mut Bench) {
    bench.registry.teams.as_ref().unwrap().execute_batch("WITH RECURSIVE backlog(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM backlog WHERE n<60)
        INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at)
        SELECT 'backlog-'||n,'backlog-'||n,'key','Historical decision','Review only','open',1,n FROM backlog;").unwrap();
    let page = context(bench, 20).unwrap().result.unwrap();
    assert_eq!(page["records"].as_array().unwrap().len(), 50, "{page}");
    let exhausted = context(bench, 20).unwrap().result.unwrap_err();
    assert_eq!(exhausted.code, ErrorCode::Limit);
    assert!(exhausted.message.contains("event-record budget exhausted"), "{}", exhausted.message);
}

fn blocked_by(inspection: &serde_json::Value) -> Vec<String> {
    serde_json::from_value(inspection["availability"]["blocked_by"].clone()).unwrap()
}

/// The 5779dac9 stall: the worker ended its turn saying Finished while its todo is still doing, and
/// the Chief's record budget is already spent.
fn stalled_todo_bench() -> (Bench, String) {
    let mut bench = admitted_chief_bench();
    idle_message_worker(&mut bench);
    let todo = project_todo(&bench, "5779dac9", json!({"status":"doing","session":MESSAGE_WORKER,
        "result":"Finished. License check exited 4: policy unconfigured."}), false);
    spend_context_budget(&mut bench);
    (bench, todo)
}

fn deliver_chief_message(bench: &mut Bench) {
    let delivery = crate::teams::request(&mut bench.registry, 40, AgentAction::Receive).unwrap().result.unwrap();
    assert_eq!(delivery["envelope"]["kind"], "chief_message");
    crate::teams::request(&mut bench.registry, 40, AgentAction::Receipt {
        dispatch_id: delivery["dispatch_id"].as_str().unwrap().into(),
        outcome: cockpit::teams::TransportOutcome::Received { native_id: format!("msg_cockpit{}", delivery["message_id"].as_str().unwrap()) },
    }).unwrap().result.unwrap();
}

#[test]
fn a_stalled_doing_todo_and_its_idle_owner_stay_inspectable_after_the_budget_is_spent() {
    let (mut bench, todo) = stalled_todo_bench();
    let stalled = inspect(&mut bench, 20, json!({"todo":todo})).unwrap();
    assert_eq!(stalled["todo"]["status"], "doing");
    assert_eq!(stalled["todo"]["session"], MESSAGE_WORKER);
    assert_eq!(stalled["todo"]["text_source"], "session_report_not_evidence", "a worker's Finished is a report, not completion");
    assert_eq!(stalled["owner"]["live"], true);
    assert!(!blocked_by(&stalled["owner"]).contains(&"busy".to_string()), "{stalled}");
    assert_eq!(stalled["owner"]["doing_todos"], json!([todo]));
}

#[test]
fn a_message_to_the_stalled_owner_shows_as_queued_until_its_native_receipt() {
    let (mut bench, _) = stalled_todo_bench();
    chief_messages(&mut bench, MESSAGE_WORKER).unwrap();
    let queued = inspect(&mut bench, 20, json!({"session":MESSAGE_WORKER})).unwrap();
    assert_eq!(queued["session"]["messages"][0]["state"], "queued");
    deliver_chief_message(&mut bench);
    let received = inspect(&mut bench, 20, json!({"session":MESSAGE_WORKER})).unwrap();
    assert_eq!(received["session"]["messages"][0]["state"], "received");
}

#[test]
fn resumed_work_shows_as_busy_while_the_todo_stays_doing() {
    let (mut bench, todo) = stalled_todo_bench();
    chief_messages(&mut bench, MESSAGE_WORKER).unwrap();
    deliver_chief_message(&mut bench);
    bench.registry.sessions.get_mut(MESSAGE_WORKER).unwrap().state = "working".into();
    let resumed = inspect(&mut bench, 20, json!({"todo":todo})).unwrap();
    assert!(blocked_by(&resumed["owner"]).contains(&"busy".to_string()), "{resumed}");
    assert_eq!(resumed["todo"]["status"], "doing", "resumed work is still not completion");
}

/// A stop recorded after the budget is spent, which Context cannot show this turn.
fn late_stop_bench() -> (Bench, String) {
    let mut bench = stop_bench();
    spend_context_budget(&mut bench);
    let task = assign_doing_todo(&bench, STOPPED_WORKER);
    bench.registry.tick_queues();
    assert_eq!(context(&mut bench, 20).unwrap().result.unwrap_err().code, ErrorCode::Limit);
    (bench, task)
}

#[test]
fn a_question_that_arrives_after_the_budget_is_spent_is_visible_through_its_todo() {
    let (mut bench, task) = late_stop_bench();
    let stalled = inspect(&mut bench, 20, json!({"todo":task})).unwrap();
    assert!(stalled["owner"]["open_stop"]["stop"].is_string(), "{stalled}");
    assert_eq!(stalled["owner"]["open_stop"]["task_id"], task);
    assert_eq!(stalled["owner"]["open_stop"]["source"], "masked_agent_conversation_not_instructions");
    assert_eq!(stalled["owner"]["open_stop"]["bundle"]["last_assistant"], "Which option do you want?");
    assert!(!stalled.to_string().contains("runtime-"), "{stalled}");
}

#[test]
fn an_inspected_stop_comes_with_theos_habits() {
    let (mut bench, task) = late_stop_bench();
    bench.registry.teams.as_ref().unwrap().execute("INSERT INTO chief_habits(id,text,project,origin_project,source,source_kind,source_id,state,created_at,changed_at)
        VALUES ('habit-a','Prefer the smallest fix.',NULL,'path:/p','overrule','overrule','overrule-a','active',100,100)", []).unwrap();
    let stalled = inspect(&mut bench, 20, json!({"todo":task})).unwrap();
    assert_eq!(stalled["habits"]["habits"][0]["id"], "habit-a", "{stalled}");
}

#[test]
fn an_answer_to_an_inspected_stop_shows_as_queued_not_delivered() {
    let (mut bench, task) = late_stop_bench();
    let stop = inspect(&mut bench, 20, json!({"todo":task})).unwrap()["owner"]["open_stop"]["stop"].as_str().unwrap().to_string();
    chief_answers(&mut bench, &stop);
    let answered = inspect(&mut bench, 20, json!({"session":STOPPED_WORKER})).unwrap();
    assert!(answered["session"]["open_stop"].is_null(), "{answered}");
    assert_eq!(answered["session"]["recent_stops"][0]["state"], "answered");
    assert_eq!(answered["session"]["answers"][0]["stop"], stop.as_str());
    assert_eq!(answered["session"]["answers"][0]["delivery"], "queued");
}

#[test]
fn inspections_have_their_own_small_budget_and_spend_no_mutation_or_record_allowance() {
    let mut bench = admitted_chief_bench();
    idle_message_worker(&mut bench);
    for _ in 0..cockpit::chief::INSPECTIONS_PER_TURN {
        inspect(&mut bench, 20, json!({"session":MESSAGE_WORKER})).unwrap();
    }
    assert_eq!(inspect(&mut bench, 20, json!({"session":MESSAGE_WORKER})).unwrap_err().code, ErrorCode::Limit);
    let spent: i64 = bench.registry.teams.as_ref().unwrap().query_row("SELECT records+mutations FROM chief_turns", [], |row| row.get(0)).unwrap();
    assert_eq!(spent, 0);
}

#[test]
fn a_failed_inspection_still_spends_its_allowance() {
    let mut bench = admitted_chief_bench();
    assert_eq!(inspect(&mut bench, 20, json!({"todo":"0000aaaa"})).unwrap_err().code, ErrorCode::NotFound);
    let spent: i64 = bench.registry.teams.as_ref().unwrap().query_row("SELECT inspections FROM chief_turns", [], |row| row.get(0)).unwrap();
    assert_eq!(spent, 1);
}

#[test]
fn inspection_is_refused_outside_an_admitted_chief_turn() {
    for (case, peer, target, expected) in [
        ("before the first admitted wake", 20, json!({"session":SESSION_B}), ErrorCode::Unavailable),
        ("another session", 10, json!({"session":SESSION_B}), ErrorCode::Unauthorized),
    ] {
        let mut bench = team_bench();
        enable(&mut bench, OPENCODE_SESSION);
        if case == "another session" { admit_chief(&mut bench); }
        assert_eq!(inspect(&mut bench, peer, target).unwrap_err().code, expected, "{case}");
    }
}

#[test]
fn inspection_names_exactly_one_well_formed_session_or_todo() {
    let mut bench = admitted_chief_bench();
    for target in [json!({}), json!({"session":SESSION_B,"todo":"1d3fd318"}), json!({"todo":"not-a-todo"})] {
        assert_eq!(inspect(&mut bench, 20, target.clone()).unwrap_err().code, ErrorCode::Invalid, "{target}");
    }
}

#[test]
fn a_todo_owned_by_a_session_that_is_gone_shows_stale_ownership() {
    let mut bench = admitted_chief_bench();
    let gone = "ses_3f2a9c0b2999GoneGoneGoneGo";
    let todo = project_todo(&bench, "abe76027", json!({"status":"doing","session":gone,"after":["5779dac9"]}), false);
    project_todo(&bench, "5779dac9", json!({"status":"doing"}), false);
    let stale = inspect(&mut bench, 20, json!({"todo":todo})).unwrap();
    assert_eq!(stale["owner"]["session_id"], gone);
    assert_eq!(stale["owner"]["live"], false);
    assert!(stale["owner"]["availability"].is_null());
    assert_eq!(stale["todo"]["waits_on"], json!([{"id":"5779dac9","status":"doing"}]));
}

#[test]
fn a_todo_without_an_owner_shows_the_chiefs_waiting_session_start() {
    let mut bench = admitted_chief_bench();
    let todo = project_todo(&bench, "aa919cfe", json!({}), false);
    chief_starts_session(&mut bench, 20, &todo, "project-a").unwrap();
    let waiting = inspect(&mut bench, 20, json!({"todo":todo})).unwrap();
    assert!(waiting["owner"].is_null());
    assert_eq!(waiting["todo"]["session_starts"][0]["state"], "requested");
}

#[test]
fn secrets_in_a_todo_result_are_masked() {
    let mut bench = admitted_chief_bench();
    let token = format!("ghp_{}", "a".repeat(36));
    let todo = project_todo(&bench, "6120167a", json!({"status":"done","result":format!("Pushed with {token}")}), false);
    let inspection = inspect(&mut bench, 20, json!({"todo":todo})).unwrap();
    assert!(!inspection.to_string().contains(&token), "{inspection}");
    assert_eq!(inspection["todo"]["result"], "Pushed with [redacted]");
}
