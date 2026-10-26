//! Whole workflow through authenticated daemon routes and the real native adapter/recorders.
//! Native SDK results, launcher/process facts and approved command results are controlled fixtures;
//! none of this is live admission proof or execution against the user's running checkout.
use super::*;
use cockpit::teams::{TaskKey, TransportOutcome};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};

const WORKER_A: &str = "ses_3f2a9c0b2000AbCdEfGhIjKlMn";
const WORKER_B: &str = "ses_3f2a9c0b2001AbCdEfGhIjKlMn";
const COMMIT: &str = "1111111111111111111111111111111111111111";
const PEERS: [(u32, &str, &str, &str); 6] = [
    (10, SESSION_A, "claude", "project-a"),
    (20, OPENCODE_SESSION, "opencode", "project-a"),
    (30, SESSION_B, "claude", "project-a"),
    (40, WORKER_A, "opencode", "project-a"),
    (50, WORKER_B, "opencode", "project-b"),
    (60, CODEX_SESSION, "codex", "project-b"),
];

fn host(bench: &mut Bench, action: ChiefHostAction) -> Value {
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action }).unwrap().result.unwrap()
}

fn count(bench: &Bench, query: &str) -> i64 {
    bench.registry.teams.as_ref().unwrap().query_row(query, [], |row| row.get(0)).unwrap()
}

fn report_launches(bench: &mut Bench) {
    for (peer, session, agent, project) in PEERS {
        let directory = bench.fixture.root.join("home/IdeaProjects").join(project);
        fs::create_dir_all(directory.join(".git")).unwrap();
        bench.world.borrow_mut().sender_targets.insert(peer, target(&[("tmux_pane", &format!("%{peer}"))]));
        bench.world.borrow_mut().sender_directories.insert(peer, directory.to_string_lossy().into());
        bench.world.borrow_mut().sender_runtimes.insert(peer, format!("runtime-{peer}"));
        bench.world.borrow_mut().live_ids.insert(session.into());
        let state = bench.registry.sessions.get(session).map(|entry| entry.state.clone()).unwrap_or("waiting".into());
        let mut report = json!({"session":session,"agent":agent,"cwd":directory,"project":project,"state":state});
        if agent == "codex" { report["native_thread"] = json!(SESSION_A); }
        bench.registry.report_from(parse_session(&report.to_string()).unwrap(), peer).unwrap();
        let entry = bench.registry.sessions.get_mut(session).unwrap();
        entry.since = (crate::teams::now_ms() / 1000 - 600) as f64;
    }
}

fn fresh_facts(bench: &mut Bench) {
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefFocus {
        viewed: String::new(), workspace_active: false, gaming: false,
    }).unwrap().result.unwrap();
    for (_, session, agent, _) in PEERS {
        if agent != "opencode" { continue; }
        let identity = verified(bench, session);
        let idle = bench.registry.sessions[session].state == "waiting";
        crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefNativeObservation {
            session: identity, idle, pending_input: false, observed_at_ms: crate::teams::now_ms(),
        }).unwrap().result.unwrap();
    }
}

fn checkpoint(bench: &mut Bench, restart: bool) {
    if restart {
        bench.registry.publish();
        restart_chief_daemon(bench);
        report_launches(bench);
    }
    fresh_facts(bench);
}

fn verify_admission(bench: &mut Bench, session: &str) {
    let session = verified(bench, session);
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::VerifyChiefAdmission {
        session, adapter: "opencode-queued-v1".into(),
        evidence: "Controlled explicit host proof fixture ONLY; not production activation or real draft preservation proof".into(),
    }).unwrap().result.unwrap();
}

const INBOX_HARNESS: &str = r#"
import {appendFileSync} from 'node:fs'
import childProcess from 'node:child_process'
import {syncBuiltinESMExports} from 'node:module'
let completed
const completion = new Promise(resolve => { completed = resolve })
const spawn = childProcess.spawn
childProcess.spawn = (command, args, options) => {
  const child = spawn(command, args, options)
  if (command === 'cockpit-team') {
    let output = ''
    child.stdout.on('data', chunk => { output += chunk })
    child.on('close', code => {
      if (code === 0 && (args[0] === 'receipt' || (args[0] === 'receive' && (!output.trim() || output.trim() === 'null')))) completed()
    })
  }
  return child
}
syncBuiltinESMExports()
const [plugin, directory, log] = process.argv.slice(2)
const {default: definition} = await import(plugin)
async function* stream() { await new Promise(() => {}) }
const storage = new Map()
const cleanup = await definition.setup({
  storage:{get:async key=>storage.get(key),set:async(key,value)=>storage.set(key,value),remove:async key=>storage.delete(key)},
  integration:{connection:{active:async()=>undefined,resolve:async()=>undefined}},
  location:{directory}, event:{subscribe:()=>stream()}, tool:{hook:async()=>({dispose:async()=>{}})},
  session:{hook:async()=>({dispose:async()=>{}}), synthetic:async(input)=>{appendFileSync(log,JSON.stringify(input)+'\n'); return {id:input.id}}},
})
await Promise.race([completion, new Promise((_, reject) => setTimeout(() => reject(new Error('native receipt/empty receive did not complete')), 8000))])
await cleanup?.()
process.exit(0)
"#;

// Run the production OpenCode plugin and cockpit-team client against the daemon routes. Only
// the external native SDK and launcher/kernel process mapping are replaced, not delivery logic.
fn native_inbox(bench: &mut Bench, peer: u32, session: &str) -> Vec<Value> {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let binary = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("cockpit-team");
    assert!(binary.is_file(), "build the cockpit-team binary before this suite");
    let shims = bench.fixture.root.join("native-bin");
    fs::create_dir_all(&shims).unwrap();
    if !shims.join("cockpit-team").exists() {
        std::os::unix::fs::symlink(binary, shims.join("cockpit-team")).unwrap();
    }
    let hook = bench.fixture.root.join("home/agent-cockpit/config/claude/.claude/hooks/session-state.sh");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    fs::write(hook, "#!/bin/bash\ncat >/dev/null\n").unwrap();
    let token = cockpit::util::token_hex(4);
    let socket = bench.fixture.root.join(format!("inbox-{token}.sock"));
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let answers = bench.fixture.answers("teams");
    cockpit::teams::client::publish_socket(&socket, answers.parent().unwrap()).unwrap();
    let harness = bench.fixture.root.join("inbox.mjs");
    let log = bench.fixture.root.join(format!("native-{token}.jsonl"));
    fs::write(&harness, INBOX_HARNESS).unwrap();
    let runtime = format!("runtime-{peer}");
    let mut child = Command::new("node").arg(&harness)
        .arg(checkout.join("config/opencode/.config/opencode/plugins/cockpit.ts"))
        .arg(&bench.registry.sessions[session].cwd).arg(&log)
        .env("CLAUDE_CODE_SESSION_ID", session).env("CLAUDE_ANSWERS_KEY", ANSWERS_KEY)
        .env("COCKPIT_TEAM_POLL_MS", "50")
        .env("PATH", format!("{}:{}", shims.display(), std::env::var("PATH").unwrap()))
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut first_request=true;
    loop {
        if let Ok((mut stream, _)) = listener.accept() {
            if first_request {std::thread::sleep(std::time::Duration::from_millis(450));first_request=false;}
            stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
            let process = crate::fifo::peer_pid(&stream).unwrap();
            bench.world.borrow_mut().sender_runtimes.insert(process, runtime.clone());
            let mut wire = String::new();
            stream.read_to_string(&mut wire).unwrap();
            let reply = crate::teams::request(&mut bench.registry, process, serde_json::from_str(&wire).unwrap()).unwrap();
            assert!(reply.result.is_ok(), "{reply:?}");
            stream.write_all(serde_json::to_string(&reply).unwrap().as_bytes()).unwrap();
            bench.world.borrow_mut().sender_runtimes.remove(&process);
        }
        if child.try_wait().unwrap().is_some() { break; }
        if std::time::Instant::now() > deadline { child.kill().unwrap(); panic!("native inbox fixture timed out"); }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
    let admitted: Vec<Value> = fs::read_to_string(log).unwrap_or_default().lines()
        .map(|line| serde_json::from_str(line).unwrap()).collect();
    for item in &admitted {
        assert_eq!(item["delivery"], "queue");
        assert_eq!(item["resume"], true);
        assert_eq!(item["sessionID"], session);
    }
    admitted
}

#[test]
fn chief_workflow_crash_before_bootstrap_receipt_never_opens_mutation_allowance_or_rewakes() {
    let mut bench = team_bench();
    report_launches(&mut bench);
    fresh_facts(&mut bench);
    verify_admission(&mut bench, OPENCODE_SESSION);
    let chief = verified(&mut bench, OPENCODE_SESSION);
    host(&mut bench, ChiefHostAction::Enable { request_id: "bootstrap-crash".into(), session: chief });
    let claimed = crate::teams::request(&mut bench.registry, 20, AgentAction::Receive).unwrap().result.unwrap();
    assert_eq!(claimed["envelope"]["kind"], "chief_wake");
    checkpoint(&mut bench, true);
    assert!(native_inbox(&mut bench, 20, OPENCODE_SESSION).is_empty());
    let receipt = AgentAction::Receipt { dispatch_id: claimed["dispatch_id"].as_str().unwrap().into(),
        outcome: TransportOutcome::Received { native_id: "fixture-may-already-have-arrived".into() } };
    for _ in 0..2 {
        crate::teams::request(&mut bench.registry, 20, receipt.clone()).unwrap().result.unwrap();
        checkpoint(&mut bench, true);
    }
    assert!(crate::teams::request(&mut bench.registry, 20, AgentAction::Chief { request: ChiefAgentAction::Status {
        summary: "A late receipt must not authorize a new chief mutation".into(),
    }}).unwrap().result.is_err());
    for _ in 0..5 { bench.registry.tick_queues(); }
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_turns"), 0);
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_delivery_attempts"), 1);
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_decisions WHERE state='open'"), 1);
    assert_eq!(bench.raised().iter().filter(|id| id.starts_with("chief-decision-")).count(), 1);
    host(&mut bench,ChiefHostAction::ReconcileDelivery {request_id:"handle-wake".into(),message:claimed["message_id"].as_str().unwrap().into(),expected_revision:1,
        evidence:cockpit::chief::ReconciliationEvidence::Handled {note:"Exact wake handled, not proof of admission".into()}});
    crate::teams::request(&mut bench.registry,20,receipt).unwrap().result.unwrap();
    assert_eq!(count(&bench,"SELECT count(*) FROM chief_turns"),0);
    assert!(crate::teams::request(&mut bench.registry,20,AgentAction::Chief {request:ChiefAgentAction::Status {summary:"Host handling cannot open an allowance".into()}}).unwrap().result.is_err());
}

// Execute the generated report contract with the real CLI and authenticated daemon routes.
// Only process identity and detached terminal launch are controlled fixture boundaries.
fn team_cli_script(bench: &mut Bench, peer: u32, script: &str) -> (std::process::Output, Vec<String>) {
    let binary = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("cockpit-team");
    let shims = bench.fixture.root.join("report-bin");
    fs::create_dir_all(&shims).unwrap();
    if !shims.join("cockpit-team").exists() { std::os::unix::fs::symlink(binary, shims.join("cockpit-team")).unwrap(); }
    let socket = bench.fixture.root.join(format!("report-{}.sock",cockpit::util::token_hex(4)));
    let listener = UnixListener::bind(&socket).unwrap(); listener.set_nonblocking(true).unwrap();
    let answers = bench.fixture.answers("teams");
    cockpit::teams::client::publish_socket(&socket, answers.parent().unwrap()).unwrap();
    let runtime = bench.world.borrow().sender_runtimes[&peer].clone();
    let mut child = Command::new("bash").args(["-c",script])
        .env("CLAUDE_ANSWERS_KEY",ANSWERS_KEY)
        .env("PATH",format!("{}:{}",shims.display(),std::env::var("PATH").unwrap()))
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = std::time::Instant::now()+std::time::Duration::from_secs(10);
    let mut requests = Vec::new();
    loop {
        if let Ok((mut stream,_)) = listener.accept() {
            stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
            let process = crate::fifo::peer_pid(&stream).unwrap();
            bench.world.borrow_mut().sender_runtimes.insert(process,runtime.clone());
            let mut wire = String::new(); stream.read_to_string(&mut wire).unwrap();
            let payload: Value = serde_json::from_str(&wire).unwrap();
            requests.push(payload["action"].as_str().unwrap().to_string());
            let reply = crate::teams::request(&mut bench.registry,process,serde_json::from_value(payload).unwrap())
                .unwrap_or_else(|error|cockpit::teams::TeamReply {request_id:"fixture-error".into(),result:Err(error)});
            stream.write_all(serde_json::to_string(&reply).unwrap().as_bytes()).unwrap();
            bench.world.borrow_mut().sender_runtimes.remove(&process);
        }
        if child.try_wait().unwrap().is_some() {break;}
        if std::time::Instant::now()>deadline {child.kill().unwrap();panic!("team CLI fixture timed out");}
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    (child.wait_with_output().unwrap(),requests)
}

fn team_cli_action(bench: &mut Bench, peer: u32, verb: &str, action: Value) -> Value {
    let (output,_) = team_cli_script(bench,peer,&format!("printf '%s' '{}' | cockpit-team {verb}",action));
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty(),"{}",String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).unwrap()
}

fn spawned_qa_report_contract() -> (Bench, String) {
    let mut bench = team_bench(); report_launches(&mut bench);
    bench.world.borrow_mut().default_agent=Some("claude".into());
    let task=TaskKey {project:verified(&mut bench,WORKER_A).project,task_id:"1d3fd318".into()};
    team_cli_action(&mut bench,40,"recruit",json!({"action":"recruit","recruitment":{
        "task":task,"submission_key":"report-recruit","role":"developer","reason":"Review the exact task",
        "recruits":[{"role":"qa","brief":"Review only this task"}]}}));
    let folder=bench.registry.sessions[WORKER_A].cwd.clone();
    team_cli_action(&mut bench,40,"handoff",json!({"action":"handoff","handoff":{
        "task":task,"submission_key":"report-handoff","branch":"feature/report","commit":COMMIT,"checkout":folder,
        "summary":"Exact implementation","tests":"Worker prose is not proof","figcraft":[]}}));
    let start=started(&bench).into_iter().find(|command|command.get(5).is_some_and(|role|role=="qa")).unwrap();
    assert_eq!(start[7],COMMIT);
    spawned_session_reports(&mut bench,43,NEW_SESSION,&start[4]);
    assert_eq!(role_of(&mut bench,NEW_SESSION).as_deref(),Some("qa"));
    (bench,start[8].clone())
}

fn filled_report_script(brief: &str, verdict: &str) -> String {
    brief.lines().filter(|line|line.starts_with("printf ")).collect::<Vec<_>>().join("\n")
        .replace("<a new key: letters, digits, - or _>","final-report")
        .replace("<the Developer's session_id from cockpit-team context>",WORKER_A)
        .replace("<pass or findings>",verdict)
        .replace("<what you found>","Reviewed exact commit; worker test prose is not execution proof")
}

#[test]
fn ordinary_spawned_qa_keeps_findings_only_reporting_and_automatic_exit() {
    let (mut bench,brief)=spawned_qa_report_contract();
    let (output,requests)=team_cli_script(&mut bench,43,&filled_report_script(&brief,"pass"));
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    assert_eq!(requests,vec!["send".to_string()]);
    crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert!(started(&bench).contains(&vec!["cockpit".into(),"exit".into(),NEW_SESSION.into()]));
}

/// A worker stopped long enough ago for the answer grace, its stop recorded, and a Chief turn admitted.
fn admitted_stop_bench() -> (Bench, String) {
    let mut bench = stop_bench();
    assign_doing_todo(&bench, STOPPED_WORKER);
    bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap().since = (crate::teams::now_ms()/1000-40) as f64;
    verify_admission(&mut bench, STOPPED_WORKER);
    let worker = verified(&mut bench, STOPPED_WORKER);
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefNativeObservation {
        session: worker, idle: true, pending_input: false, observed_at_ms: crate::teams::now_ms(),
    }).unwrap().result.unwrap();
    bench.registry.tick_queues();
    let stop: String = bench.registry.teams.as_ref().unwrap().query_row("SELECT id FROM chief_stops WHERE state='open'", [], |row| row.get(0)).unwrap();
    prepare_chief_boundary(&mut bench);
    bench.registry.sessions.get_mut(OPENCODE_SESSION).unwrap().since -= 7.0;
    let wake = crate::teams::request(&mut bench.registry, 20, AgentAction::Receive).unwrap().result.unwrap();
    assert_eq!(wake["envelope"]["kind"], "chief_wake");
    crate::teams::request(&mut bench.registry, 20, AgentAction::Receipt { dispatch_id: wake["dispatch_id"].as_str().unwrap().into(),
        outcome: TransportOutcome::Received { native_id: "chief-answer-wake".into() } }).unwrap().result.unwrap();
    (bench, stop)
}

fn answered_stop_bench() -> (Bench, String) {
    let (mut bench, stop) = admitted_stop_bench();
    crate::teams::request(&mut bench.registry, 20, AgentAction::Chief { request: ChiefAgentAction::Answer {
        stop: stop.clone(), text: "Use option A.".into(), reason: "The task notes keep the old API.".into(),
    }}).unwrap().result.unwrap();
    (bench, stop)
}

fn answer_delivery(bench: &Bench) -> String {
    bench.registry.teams.as_ref().unwrap().query_row("SELECT delivery FROM chief_answers", [], |row| row.get(0)).unwrap()
}

#[test]
fn a_chief_answer_is_delivered_through_the_native_queue_with_the_boundary_text() {
    let (mut bench, stop) = answered_stop_bench();
    let admitted = native_inbox(&mut bench, STOPPED_WORKER_PEER, STOPPED_WORKER);
    assert_eq!(admitted.len(), 1, "{admitted:?}");
    let message: String = bench.registry.teams.as_ref().unwrap().query_row(
        "SELECT id FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_answer'", [], |row| row.get(0)).unwrap();
    assert_eq!(admitted[0]["id"], format!("msg_cockpit{message}"));
    let text = admitted[0]["text"].as_str().unwrap();
    assert!(text.starts_with("Use option A."), "{text}");
    assert!(text.ends_with(cockpit::chief::CHIEF_ANSWER_BOUNDARY), "{text}");
    assert_eq!(admitted[0]["description"], "Chief answer: Use option A.");
    assert_eq!(answer_delivery(&bench), "received");
    let state: String = bench.registry.teams.as_ref().unwrap().query_row("SELECT state FROM chief_stops WHERE id=?1", [&stop], |row| row.get(0)).unwrap();
    assert_eq!(state, "answered");
}

#[test]
fn theos_answer_to_an_escalation_is_delivered_natively_even_after_the_chief_is_disabled() {
    let (mut bench, stop) = admitted_stop_bench();
    let escalated = crate::teams::request(&mut bench.registry, 20, AgentAction::Chief { request: serde_json::from_value(
        json!({"action":"escalate","stop":stop,"category":"security","reason":"The question touches credentials.","suggested_answer":"Ask Theo."})).unwrap()
    }).unwrap().result.unwrap();
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_answers"), 0, "an escalation sends nothing");
    let epoch: String = bench.registry.teams.as_ref().unwrap().query_row("SELECT epoch FROM chief_grants WHERE state='active'", [], |row| row.get(0)).unwrap();
    host(&mut bench, ChiefHostAction::Disable { request_id: "disable-before-theo".into(), expected_epoch: epoch });
    host(&mut bench, ChiefHostAction::AnswerDecision { request_id: "theo-answers".into(), decision: escalated["decision"].as_str().unwrap().into(),
        expected_revision: 1, answer: "Use the staging credentials and tell me which.".into() });
    let admitted = native_inbox(&mut bench, STOPPED_WORKER_PEER, STOPPED_WORKER);
    assert_eq!(admitted.len(), 1, "{admitted:?}");
    let text = admitted[0]["text"].as_str().unwrap();
    assert!(text.starts_with("Use the staging credentials"), "{text}");
    assert!(text.ends_with(cockpit::chief::THEO_ANSWER_LABEL), "{text}");
    assert!(!text.contains(cockpit::chief::CHIEF_ANSWER_BOUNDARY), "{text}");
    assert_eq!(admitted[0]["description"], "Theo's answer: Use the staging credentials and tell me which.");
    assert_eq!(answer_delivery(&bench), "received");
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_answers WHERE source='theo'"), 1);
}

fn escalate_stop(bench: &mut Bench, stop: &str) -> String {
    let escalated = crate::teams::request(&mut bench.registry, 20, AgentAction::Chief { request: serde_json::from_value(
        json!({"action":"escalate","stop":stop,"category":"scope","reason":"Changing the API is outside the task."})).unwrap()
    }).unwrap().result.unwrap();
    escalated["decision"].as_str().unwrap().to_string()
}

fn stop_and_decision_states(bench: &Bench, stop: &str, decision: &str) -> (String, String) {
    let connection = bench.registry.teams.as_ref().unwrap();
    (connection.query_row("SELECT state FROM chief_stops WHERE id=?1", [stop], |row| row.get(0)).unwrap(),
        connection.query_row("SELECT state FROM chief_decisions WHERE id=?1", [decision], |row| row.get(0)).unwrap())
}

#[test]
fn a_telemetry_gap_keeps_an_escalated_question_stop_open_until_its_boundary_changes() {
    let (mut bench, stop) = admitted_stop_bench();
    let since = bench.registry.sessions[STOPPED_WORKER].since as i64;
    bench.registry.teams.as_ref().unwrap().execute("UPDATE chief_stops SET boundary=?2 WHERE id=?1", (&stop, format!("needs-you:{since}"))).unwrap();
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "needs-you".into(); entry.needs_input = true;
    report_worker_permissions(&mut bench, 1, 0);
    let decision = escalate_stop(&mut bench, &stop);
    bench.registry.chief_telemetry.remove(STOPPED_WORKER);
    bench.registry.tick_queues();
    assert_eq!(stop_and_decision_states(&bench, &stop, &decision), ("escalated".into(), "open".into()), "a restarted daemon has no telemetry yet");
    let unavailable = serde_json::from_value(json!({"action":"native_telemetry","sequence":2,"counts":null})).unwrap();
    crate::teams::request(&mut bench.registry, STOPPED_WORKER_PEER, unavailable).unwrap().result.unwrap();
    bench.registry.tick_queues();
    assert_eq!(stop_and_decision_states(&bench, &stop, &decision), ("escalated".into(), "open".into()), "the TUI left the session");
    bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap().since += 5.0;
    bench.registry.tick_queues();
    assert_eq!(stop_and_decision_states(&bench, &stop, &decision), ("superseded".into(), "superseded".into()));
}

#[test]
fn an_escalation_closes_when_the_agent_moves_on_after_the_chief_is_disabled() {
    let (mut bench, stop) = admitted_stop_bench();
    let decision = escalate_stop(&mut bench, &stop);
    let epoch: String = bench.registry.teams.as_ref().unwrap().query_row("SELECT epoch FROM chief_grants WHERE state='active'", [], |row| row.get(0)).unwrap();
    host(&mut bench, ChiefHostAction::Disable { request_id: "disable-then-move-on".into(), expected_epoch: epoch });
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "working".into(); entry.since += 5.0;
    bench.registry.tick_queues();
    assert_eq!(stop_and_decision_states(&bench, &stop, &decision), ("superseded".into(), "superseded".into()));
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "waiting".into(); entry.since += 5.0;
    bench.registry.tick_queues();
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_stops"), 1, "without a Chief no new stop is recorded");
}

#[test]
fn a_chief_answer_never_reaches_a_permission_prompt() {
    let (mut bench, _) = answered_stop_bench();
    report_worker_permissions(&mut bench, 1, 0);
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "needs-you".into(); entry.needs_input = false;
    assert!(crate::teams::request(&mut bench.registry, STOPPED_WORKER_PEER, AgentAction::Receive).unwrap().result.unwrap().is_null());
    assert_eq!(answer_delivery(&bench), "queued");
}

#[test]
fn a_chief_answer_reaches_a_session_stopped_at_its_question() {
    let (mut bench, stop) = answered_stop_bench();
    let since = bench.registry.sessions[STOPPED_WORKER].since as i64;
    bench.registry.teams.as_ref().unwrap().execute("UPDATE chief_stops SET boundary=?2 WHERE id=?1", (&stop, format!("needs-you:{since}"))).unwrap();
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "needs-you".into(); entry.needs_input = true;
    assert!(crate::teams::request(&mut bench.registry, STOPPED_WORKER_PEER, AgentAction::Receive).unwrap().result.unwrap().is_null(),
        "without fresh telemetry an open permission cannot be ruled out");
    report_worker_permissions(&mut bench, 1, 0);
    let delivery = crate::teams::request(&mut bench.registry, STOPPED_WORKER_PEER, AgentAction::Receive).unwrap().result.unwrap();
    assert_eq!(delivery["envelope"]["kind"], "chief_answer", "{delivery}");
}

/// A runtime waiting on a form or permission still runs its session: OpenCode reports it as running.
fn report_worker_prompts(bench: &mut Bench, sequence: i64, permissions: u32, forms: u32) {
    let action = serde_json::from_value(json!({"action":"native_telemetry","sequence":sequence,"counts":{
        "running_sessions":1,"pending_inbox":0,"pending_permissions":permissions,"pending_forms":forms}})).unwrap();
    crate::teams::request(&mut bench.registry, STOPPED_WORKER_PEER, action).unwrap().result.unwrap();
}

/// A worker stopped at an open single-question form, with the Chief's choice for it queued.
fn form_reply_bench() -> Bench {
    let (mut bench, stop) = admitted_stop_bench();
    let since = bench.registry.sessions[STOPPED_WORKER].since as i64;
    bench.registry.teams.as_ref().unwrap().execute("UPDATE chief_stops SET boundary=?2,bundle=json_set(bundle,'$.question',json(?3)) WHERE id=?1",
        (&stop, format!("needs-you:{since}"), json!({"header":"Option","question":"Which option?","options":["A","B"],"question_count":1}).to_string())).unwrap();
    crate::teams::request(&mut bench.registry, 20, AgentAction::Chief { request: ChiefAgentAction::AnswerForm {
        stop: stop.clone(), choice: "B".into(), reason: "The task notes pick B.".into(),
    }}).unwrap().result.unwrap();
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "needs-you".into(); entry.needs_input = false;
    bench
}

#[test]
fn a_chief_form_reply_reaches_a_session_waiting_on_its_form_and_never_a_permission_prompt() {
    let mut bench = form_reply_bench();
    report_worker_prompts(&mut bench, 1, 1, 1);
    assert!(crate::teams::request(&mut bench.registry, STOPPED_WORKER_PEER, AgentAction::Receive).unwrap().result.unwrap().is_null(),
        "an open permission prompt holds the reply");
    report_worker_prompts(&mut bench, 2, 0, 1);
    let delivery = crate::teams::request(&mut bench.registry, STOPPED_WORKER_PEER, AgentAction::Receive).unwrap().result.unwrap();
    assert_eq!(delivery["envelope"]["kind"], "chief_form_reply", "{delivery}");
    assert_eq!(delivery["envelope"]["choice"], "B");
    assert_eq!(answer_delivery(&bench), "queued", "only the plugin's receipt settles it");
}

#[test]
fn a_chief_form_reply_waits_while_the_runtime_is_busy_without_a_form() {
    let mut bench = form_reply_bench();
    report_worker_prompts(&mut bench, 1, 0, 0);
    assert!(crate::teams::request(&mut bench.registry, STOPPED_WORKER_PEER, AgentAction::Receive).unwrap().result.unwrap().is_null());
    assert_eq!(answer_delivery(&bench), "queued");
}

#[test]
fn theo_overrules_a_chief_answer_end_to_end() {
    let (mut bench, _stop) = answered_stop_bench();
    assert_eq!(native_inbox(&mut bench, STOPPED_WORKER_PEER, STOPPED_WORKER).len(), 1, "the Chief's answer went out first");
    let answer: String = bench.registry.teams.as_ref().unwrap().query_row("SELECT id FROM chief_answers", [], |row| row.get(0)).unwrap();
    let correction = "Keep option A but leave the public API untouched.";
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "working".into(); entry.since += 2.0;
    host(&mut bench, ChiefHostAction::Overrule { request_id: "theo-overrules".into(), answer: answer.clone(), correction: correction.into() });
    assert!(crate::teams::request(&mut bench.registry, STOPPED_WORKER_PEER, AgentAction::Receive).unwrap().result.unwrap().is_null(),
        "the agent is mid-turn: the correction waits for its next stop");
    let overrule_delivery = |bench: &Bench| -> String { bench.registry.teams.as_ref().unwrap().query_row("SELECT delivery FROM chief_overrules", [], |row| row.get(0)).unwrap() };
    assert_eq!(overrule_delivery(&bench), "queued");
    let entry = bench.registry.sessions.get_mut(STOPPED_WORKER).unwrap();
    entry.state = "waiting".into(); entry.since += 3.0;
    let admitted = native_inbox(&mut bench, STOPPED_WORKER_PEER, STOPPED_WORKER);
    assert_eq!(admitted.len(), 1, "{admitted:?}");
    let message: String = bench.registry.teams.as_ref().unwrap().query_row(
        "SELECT id FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_correction'", [], |row| row.get(0)).unwrap();
    assert_eq!(admitted[0]["id"], format!("msg_cockpit{message}"));
    assert_eq!(admitted[0]["text"], format!("Correction from Theo to an earlier Chief answer (\"Use option A.\"): {correction}\n\n{}", cockpit::chief::CORRECTION_LABEL));
    assert_eq!(admitted[0]["description"], format!("Theo's correction: {correction}"));
    assert_eq!(overrule_delivery(&bench), "received");
    assert_eq!(answer_delivery(&bench), "received", "the overruled answer keeps its own delivery");
}

#[test]
fn a_correction_for_a_session_that_is_gone_is_cancelled_on_the_next_tick() {
    let (mut bench, _stop) = answered_stop_bench();
    assert_eq!(native_inbox(&mut bench, STOPPED_WORKER_PEER, STOPPED_WORKER).len(), 1);
    let answer: String = bench.registry.teams.as_ref().unwrap().query_row("SELECT id FROM chief_answers", [], |row| row.get(0)).unwrap();
    host(&mut bench, ChiefHostAction::Overrule { request_id: "overrule-gone".into(), answer, correction: "Leave the public API untouched.".into() });
    let live: Vec<_> = crate::chief::workers(&mut bench.registry).unwrap().into_iter().filter(|worker| worker.session.session_id != STOPPED_WORKER).collect();
    crate::chief::observe_stops_with(&mut bench.registry, &live, crate::teams::now_ms(), cockpit::todo::TodoList::load).unwrap();
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_overrules WHERE delivery='cancelled'"), 1);
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_inbox WHERE json_extract(envelope,'$.kind')='chief_correction' AND state='cancelled'"), 1);
}
