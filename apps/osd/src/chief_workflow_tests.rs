//! Whole workflow through authenticated daemon routes and the real native adapter/recorders.
//! Native SDK results, launcher/process facts and approved command results are controlled fixtures;
//! none of this is live admission proof or execution against the user's running checkout.
use super::*;
use cockpit::chief::{Evidence, GateKind, GateRequirement, WorkBrief, WorkProposal, WorkRef};
use cockpit::teams::{Role, TaskKey, TransportOutcome};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::unix::{fs::PermissionsExt, net::UnixListener};
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

fn agent(bench: &mut Bench, peer: u32, action: ChiefAgentAction) -> Value {
    crate::teams::request(&mut bench.registry, peer, AgentAction::Chief { request: action })
        .unwrap().result.unwrap()
}

fn host(bench: &mut Bench, action: ChiefHostAction) -> Value {
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action }).unwrap().result.unwrap()
}

fn snapshot(bench: &mut Bench) -> Value {
    let mut snapshot = crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefSnapshot {
        after: 0, limit: 50, generation: None,
    }).unwrap().result.unwrap();
    while snapshot["has_more"] == true {
        let page = crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefSnapshot {
            after: snapshot["next"].as_i64().unwrap(), limit: 50,
            generation: Some(snapshot["generation"].as_str().unwrap().into()),
        }).unwrap().result.unwrap();
        assert_ne!(page["invalidated"], true, "stable fixture review must paginate without mixing generations");
        snapshot["records"].as_array_mut().unwrap().extend(page["records"].as_array().unwrap().clone());
        snapshot["next"] = page["next"].clone();
        snapshot["has_more"] = page["has_more"].clone();
    }
    snapshot
}

fn count(bench: &Bench, query: &str) -> i64 {
    bench.registry.teams.as_ref().unwrap().query_row(query, [], |row| row.get(0)).unwrap()
}

fn work_state(bench: &mut Bench, work: &WorkRef) -> String {
    snapshot(bench)["records"].as_array().unwrap().iter()
        .find(|record| record["kind"] == "work" && record["brief"]["work"] == json!(work))
        .unwrap()["state"].as_str().unwrap().into()
}

fn assignment_state(bench: &mut Bench, assignment: &str) -> String {
    snapshot(bench)["records"].as_array().unwrap().iter()
        .find(|record| record["kind"] == "assignment" && record["id"] == assignment)
        .unwrap()["state"].as_str().unwrap().into()
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

struct GitBoundary { previous_path: std::ffi::OsString, phase: PathBuf }

impl GitBoundary {
    fn new(bench: &Bench) -> Self {
        let directory = bench.fixture.root.join("git-boundary");
        fs::create_dir(&directory).unwrap();
        let phase = directory.join("phase");
        fs::write(&phase, "before").unwrap();
        let script = format!(r#"#!/bin/bash
set -eu
test "$1" = -C
case "$3" in
  status) ;;
  patch-id) cat >/dev/null; echo '{} 0000000000000000000000000000000000000000' ;;
  rev-parse) if [ "$4" = 'HEAD^{{tree}}' ]; then echo '{}'; elif [ "$(cat '{}')" = before ]; then echo '{}'; else echo '{}'; fi ;;
  rev-list) echo '{}' ;;
  show) echo 'controlled reviewed patch bytes' ;;
  *) echo "unexpected external git command: $*" >&2; exit 99 ;;
esac
"#, "a".repeat(40), "f".repeat(40), phase.display(), "0".repeat(40), "2".repeat(40), "2".repeat(40));
        fs::write(directory.join("git"), script).unwrap();
        fs::set_permissions(directory.join("git"), fs::Permissions::from_mode(0o700)).unwrap();
        let previous_path = std::env::var_os("PATH").unwrap();
        // Fixture's ENVIRONMENT lock serializes all process environment changes.
        unsafe { std::env::set_var("PATH", format!("{}:{}", directory.display(), previous_path.to_string_lossy())); }
        Self { previous_path, phase }
    }
}

impl Drop for GitBoundary {
    fn drop(&mut self) { unsafe { std::env::set_var("PATH", &self.previous_path); } }
}

fn proposal(bench: &mut Bench, peer: u32, key: &str, proposal: WorkProposal) -> Value {
    let action = ChiefAgentAction::ProposeWork { submission_key: key.into(), proposal };
    let pending = agent(bench, peer, action.clone());
    assert_eq!(agent(bench, peer, action), pending);
    assert_eq!(pending["provenance"], "agent_proposal_not_authority");
    pending
}

fn approve(bench: &mut Bench, pending: &Value, request_id: &str, restart: bool) -> WorkBrief {
    let brief: WorkBrief = serde_json::from_value(pending["brief"].clone()).unwrap();
    let action = ChiefHostAction::ApproveWork {
        request_id: request_id.into(), brief: brief.clone(), approval_hash: pending["approval_hash"].as_str().unwrap().into(),
        expected_revision: brief.work.revision - 1,
    };
    let result = host(bench, action.clone());
    checkpoint(bench, restart);
    assert_eq!(host(bench, action), result);
    brief
}

fn bind(bench: &mut Bench, session: &str) {
    let session = verified(bench, session);
    host(bench, ChiefHostAction::BindWorker {
        request_id: format!("bind-{}", session.agent), session, role: Role::Developer, expected_epoch: None,
    });
}

fn queued_workflow() -> (Bench, WorkBrief, ChiefAgentAction, Value) {
    let mut bench = team_bench();
    report_launches(&mut bench);
    fresh_facts(&mut bench);
    verify_admission(&mut bench, OPENCODE_SESSION);
    verify_admission(&mut bench, WORKER_B);
    let chief = verified(&mut bench, OPENCODE_SESSION);
    let grant = host(&mut bench, ChiefHostAction::Enable { request_id: "crash-enable".into(), session: chief });
    assert_eq!(native_inbox(&mut bench, 20, OPENCODE_SESSION).len(), 1);
    bind(&mut bench, WORKER_B);
    let task = TaskKey { project: verified(&mut bench, WORKER_B).project, task_id: "example/project-b#9".into() };
    let pending = proposal(&mut bench, 50, "crash-proposal", WorkProposal {
        task, brief: "Only this explicitly approved task".into(), role: Role::Developer, prerequisites: vec![],
        gates: vec![GateRequirement { kind: GateKind::Tests, source: "host_attestation".into(), expected: json!({"commit":COMMIT}) }],
    });
    let brief = approve(&mut bench, &pending, "crash-approve", true);
    let dispatch = ChiefAgentAction::Dispatch { submission_key: "crash-dispatch".into(), work: brief.work.clone(), worker: WORKER_B.into() };
    (bench, brief, dispatch, grant)
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
        assignment: None, summary: "A late receipt must not authorize a new chief mutation".into(),
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
    assert!(crate::teams::request(&mut bench.registry,20,AgentAction::Chief {request:ChiefAgentAction::Status {assignment:None,summary:"Host handling cannot open an allowance".into()}}).unwrap().result.is_err());
}

#[test]
fn chief_workflow_assignmentless_coordination_and_stop_recover_after_lost_receipt() {
    for kind in ["coordination","stop"] {
        for stopped in [false,true] {
            let (mut bench,brief,dispatch,_)=queued_workflow();
            let assigned=agent(&mut bench,20,dispatch);let assignment=assigned["assignment"].as_str().unwrap().to_string();
            assert_eq!(native_inbox(&mut bench,50,WORKER_B).len(),1);
            agent(&mut bench,50,ChiefAgentAction::Accept {assignment:assignment.clone()});
            bench.registry.sessions.get_mut(WORKER_B).unwrap().since-=1.0;
            let coordination_boundary=bench.registry.sessions[WORKER_B].since;
            if kind=="coordination" {
                agent(&mut bench,20,ChiefAgentAction::Coordinate {submission_key:"lost-clarification".into(),work:brief.work.clone(),recipient:WORKER_B.into(),body:"Please report exact blocker".into()});
            } else {host(&mut bench,ChiefHostAction::RevokeWork {request_id:"stop-running".into(),work:brief.work.clone()});}
            fresh_facts(&mut bench);
            let claimed=crate::teams::request(&mut bench.registry,50,AgentAction::Receive).unwrap().result.unwrap();
            assert_eq!(claimed["envelope"]["kind"],if kind=="coordination" {"chief_coordinate"} else {"host_stop_notice"});
            checkpoint(&mut bench,true);
            bench.registry.tick_queues();
            let review=snapshot(&mut bench);
            assert_eq!(review["records"].as_array().unwrap().iter().filter(|record|record["kind"]=="decision" && record["decision_key"]=="delivery").count(),1);
            let evidence=if stopped {cockpit::chief::ReconciliationEvidence::SafeStop {note:"Exact recipient work cannot execute".into(),confirmed_stopped:true}} else {cockpit::chief::ReconciliationEvidence::Handled {note:"Exact message handled, work ownership retained".into()}};
            host(&mut bench,ChiefHostAction::ReconcileDelivery {request_id:"recover-exact-message".into(),message:claimed["message_id"].as_str().unwrap().into(),expected_revision:1,evidence});
            crate::teams::request(&mut bench.registry,50,AgentAction::Receipt {dispatch_id:claimed["dispatch_id"].as_str().unwrap().into(),outcome:TransportOutcome::Received {native_id:"late-after-host-recovery".into()}}).unwrap().result.unwrap();
            assert_eq!(assignment_state(&mut bench,&assignment),if stopped {"cancelled"} else if kind=="coordination" {"accepted"} else {"uncertain"});
            if kind=="coordination" && !stopped {
                agent(&mut bench,20,ChiefAgentAction::Coordinate {submission_key:"later-clarification".into(),work:brief.work,recipient:WORKER_B.into(),body:"Later message".into()});
                // Restart re-ages fixture sessions from wall time; retain a distinct consumed-turn identity.
                bench.registry.sessions.get_mut(WORKER_B).unwrap().since=coordination_boundary-1.0;fresh_facts(&mut bench);
                assert_eq!(native_inbox(&mut bench,50,WORKER_B).len(),1);
                for _ in 0..3 {agent(&mut bench,50,ChiefAgentAction::Status {assignment:Some(assignment.clone()),summary:"Blocked on exact clarification after recovery".into()});}
                checkpoint(&mut bench,true);
                let context=agent(&mut bench,20,ChiefAgentAction::ContextSnapshot {after:0,limit:50,generation:None});
                assert!(context["records"].as_array().unwrap().iter().any(|record|record["kind"]=="worker_status" && record["summary"]=="Blocked on exact clarification after recovery" && record["source"]=="assigned_worker_report_not_evidence"));
                assert_eq!(count(&bench,"SELECT count(*) FROM chief_events WHERE kind='worker_status'"),1);
            }
        }
    }
}

#[test]
fn chief_workflow_crash_after_assignment_claim_retains_ownership_until_exact_host_reconciliation() {
    for handling in ["handled", "safe_stop"] {
        let (mut bench, brief, dispatch, _) = queued_workflow();
        let assigned = agent(&mut bench, 20, dispatch.clone());
        let assignment = assigned["assignment"].as_str().unwrap().to_string();
        let claimed = crate::teams::request(&mut bench.registry, 50, AgentAction::Receive).unwrap().result.unwrap();
        assert_eq!(claimed["envelope"]["kind"], "chief_assignment");
        checkpoint(&mut bench, true);
        assert_eq!(assignment_state(&mut bench, &assignment), "uncertain");
        assert_eq!(agent(&mut bench, 20, dispatch), assigned);
        let accept = ChiefAgentAction::Accept { assignment: assignment.clone() };
        assert!(crate::teams::request(&mut bench.registry, 50, AgentAction::Chief { request: accept.clone() }).unwrap().result.is_err());
        assert!(native_inbox(&mut bench, 50, WORKER_B).is_empty());
        assert!(crate::teams::request(&mut bench.registry, 20, AgentAction::Chief { request: ChiefAgentAction::Dispatch {
            submission_key: "blind-reassignment".into(), work: brief.work, worker: WORKER_B.into(),
        }}).unwrap().result.is_err());
        for _ in 0..5 { bench.registry.tick_queues(); }
        let review = snapshot(&mut bench);
        let decisions: Vec<_> = review["records"].as_array().unwrap().iter()
            .filter(|record| record["kind"] == "decision" && record["state"] == "open").collect();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0]["decision_key"], "reconcile");
        assert_eq!(count(&bench, "SELECT count(*) FROM scheduling_reservations WHERE owner='chief'"), 1);
        let record = review["records"].as_array().unwrap().iter().find(|record| record["kind"] == "assignment").unwrap();
        let reconcile = ChiefHostAction::Reconcile { request_id: "reconcile-crash".into(), assignment: assignment.clone(),
            expected_revision: record["revision"].as_i64().unwrap(), evidence: if handling == "handled" {
                cockpit::chief::ReconciliationEvidence::Handled { note: "Controlled host confirmation: the exact native message was handled".into() }
            } else {
                cockpit::chief::ReconciliationEvidence::SafeStop { note: "Controlled host confirmation: old work is stopped and cannot execute".into(), confirmed_stopped: true }
            },
        };
        let reconciled = host(&mut bench, reconcile.clone());
        checkpoint(&mut bench, true);
        assert_eq!(host(&mut bench, reconcile), reconciled);
        if handling == "handled" {
            assert_eq!(agent(&mut bench, 50, accept)["status"], "accepted");
            assert_eq!(count(&bench, "SELECT count(*) FROM scheduling_reservations WHERE owner='chief'"), 1);
        } else {
            assert_eq!(assignment_state(&mut bench, &assignment), "cancelled");
            assert_eq!(count(&bench, "SELECT count(*) FROM scheduling_reservations WHERE owner='chief'"), 0);
        }
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_delivery_attempts WHERE kind='chief_assignment'"), 1);
    }
}

#[test]
fn chief_workflow_human_input_precedes_queued_work_and_revoke_cancels_without_native_admission() {
    let (mut bench, brief, dispatch, grant) = queued_workflow();
    let assigned = agent(&mut bench, 20, dispatch);
    checkpoint(&mut bench, true);
    let worker = verified(&mut bench, WORKER_B);
    crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::ChiefNativeObservation {
        session: worker, idle: true, pending_input: true, observed_at_ms: crate::teams::now_ms(),
    }).unwrap().result.unwrap();
    assert!(native_inbox(&mut bench, 50, WORKER_B).is_empty());
    assert_eq!(assignment_state(&mut bench, assigned["assignment"].as_str().unwrap()), "queued");
    let revoke = ChiefHostAction::RevokeWork { request_id: "revoke-queued".into(), work: brief.work.clone() };
    let revoked = host(&mut bench, revoke.clone());
    checkpoint(&mut bench, true);
    assert_eq!(host(&mut bench, revoke), revoked);
    assert!(native_inbox(&mut bench, 50, WORKER_B).is_empty());
    assert_eq!(work_state(&mut bench, &brief.work), "revoked");
    assert_eq!(assignment_state(&mut bench, assigned["assignment"].as_str().unwrap()), "cancelled");
    host(&mut bench, ChiefHostAction::Disable { request_id: "disable-revoked".into(), expected_epoch: grant["epoch"].as_str().unwrap().into() });
    checkpoint(&mut bench, true);
    assert_eq!(count(&bench, "SELECT count(*) FROM chief_delivery_attempts WHERE kind='chief_assignment'"), 0);
    assert_eq!(count(&bench, "SELECT count(*) FROM scheduling_reservations WHERE owner='chief'"), 0);
    assert!(bench.world.borrow().started.is_empty());
}

#[test]
fn chief_workflow_records_real_gate_producers_before_cross_project_dispatch_with_boundary_restarts() {
    // Removing any daemon auth, exact approval, source replay, native receipt, safe-idle ownership
    // or prerequisite check breaks this chain; no readiness/evidence rows are inserted by the test.
    for restart in [false, true] {
        let mut bench = team_bench();
        report_launches(&mut bench);
        fresh_facts(&mut bench);
        let git_boundary = GitBoundary::new(&bench);
        let notification_baseline=bench.raised();
        let live = bench.fixture.root.join("home/agent-cockpit");
        fs::create_dir_all(live.join(".git")).unwrap();
        let task_a = TaskKey { project: verified(&mut bench, WORKER_A).project, task_id: "example/project-a#8".into() };
        let task_b = TaskKey { project: verified(&mut bench, WORKER_B).project, task_id: "example/project-b#8".into() };
        let chief = verified(&mut bench, OPENCODE_SESSION);
        let enable = ChiefHostAction::Enable { request_id: "workflow-enable".into(), session: chief };
        assert_eq!(crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: enable.clone() })
            .unwrap().result.unwrap_err().code, ErrorCode::Unavailable);
        verify_admission(&mut bench, OPENCODE_SESSION);
        let grant = host(&mut bench, enable.clone());
        checkpoint(&mut bench, restart);
        assert_eq!(host(&mut bench, enable), grant);
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_grants"), 1);

        let pending = proposal(&mut bench, 40, "workflow-a", WorkProposal {
            task: task_a.clone(), brief: "Implement only the approved cross-project prerequisite".into(), role: Role::Developer,
            prerequisites: vec![], gates: vec![
                GateRequirement { kind: GateKind::Tests, source: "host_run".into(), expected: json!({"commit":COMMIT,"command_fingerprint":cockpit::audit::sha256_hex(b"make test"),"cwd":live}) },
                GateRequirement { kind: GateKind::Qa, source: "qa".into(), expected: json!({"commit":COMMIT}) },
                GateRequirement { kind: GateKind::Apply, source: "reviewed_apply".into(), expected: json!({"commit":COMMIT}) },
            ],
        });
        checkpoint(&mut bench, restart);
        let mut changed: WorkBrief = serde_json::from_value(pending["brief"].clone()).unwrap();
        changed.brief = "Widened unapproved work".into();
        assert!(crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Chief { action: ChiefHostAction::ApproveWork {
            request_id: "tampered".into(), brief: changed, approval_hash: pending["approval_hash"].as_str().unwrap().into(), expected_revision: 0,
        }}).unwrap().result.is_err());
        let approved_a = approve(&mut bench, &pending, "approve-a", restart);
        assert!(crate::teams::request(&mut bench.registry, 20, AgentAction::Chief { request: ChiefAgentAction::Dispatch {
            submission_key: "before-bootstrap".into(), work: approved_a.work.clone(), worker: WORKER_A.into(),
        }}).unwrap().result.is_err(), "selection alone does not open chief mutation allowance");
        let bootstrap = native_inbox(&mut bench, 20, OPENCODE_SESSION);
        assert_eq!(bootstrap.len(), 1);
        assert!(bootstrap[0]["text"].as_str().unwrap().contains("Material coordination"));
        checkpoint(&mut bench, restart);
        assert!(native_inbox(&mut bench, 20, OPENCODE_SESSION).is_empty());
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_turns"), 1);

        // Existing host-approved team mapping supplies Developer and current QA authority.
        let linked = crate::teams::request(&mut bench.registry, 40, AgentAction::Propose { proposal: cockpit::teams::Proposal {
            submission_key: "workflow-team".into(), task: task_a.clone(), reason: "Review this exact task".into(), members: vec![
                cockpit::teams::Member { session_id: WORKER_A.into(), role: Role::Developer },
                cockpit::teams::Member { session_id: SESSION_B.into(), role: Role::Qa },
            ],
        }}).unwrap().result.unwrap();
        crate::teams::host(&mut bench.registry, std::process::id(), HostRequest::Control { action: cockpit::teams::HostAction::Approve {
            request_id: "workflow-link".into(), proposal_id: linked["proposal_id"].as_str().unwrap().into(),
            expected_revision: 1, mapping_hash: linked["mapping_hash"].as_str().unwrap().into(),
        }}).unwrap().result.unwrap();
        bind(&mut bench, WORKER_B);
        bind(&mut bench, SESSION_A);
        bind(&mut bench, CODEX_SESSION);
        let pending_b = proposal(&mut bench, 20, "workflow-b", WorkProposal {
            task: task_b, brief: "Implement the dependent only after the exact prerequisite completes".into(), role: Role::Developer,
            prerequisites: vec![approved_a.work.clone()], gates: vec![GateRequirement {
                kind: GateKind::Tests, source: "host_attestation".into(), expected: json!({"commit":COMMIT}),
            }],
        });
        let approved_b = approve(&mut bench, &pending_b, "approve-b", restart);
        let dispatch_a = ChiefAgentAction::Dispatch { submission_key: "dispatch-a".into(), work: approved_a.work.clone(), worker: WORKER_A.into() };
        assert!(crate::teams::request(&mut bench.registry, 20, AgentAction::Chief { request: dispatch_a.clone() }).unwrap().result.is_err(), "worker live proof is separate from chief selection");
        verify_admission(&mut bench, WORKER_A);
        verify_admission(&mut bench, WORKER_B);
        fresh_facts(&mut bench);
        let review = snapshot(&mut bench);
        for monitored in [SESSION_A, SESSION_B, CODEX_SESSION] {
            let worker = review["records"].as_array().unwrap().iter().find(|record| record["kind"] == "worker" && record["session"]["session_id"] == monitored).unwrap();
            assert!(worker["ready"] == false || worker["ready"] == 0);
            assert!(worker["blocked_by"].as_array().unwrap().contains(&json!("native_admission_unverified_for_adapter")));
        }
        for (key, worker) in [("refuse-claude", SESSION_A), ("refuse-codex", CODEX_SESSION)] {
            assert!(crate::teams::request(&mut bench.registry, 20, AgentAction::Chief { request: ChiefAgentAction::Dispatch {
                submission_key: key.into(), work: approved_a.work.clone(), worker: worker.into(),
            }}).unwrap().result.is_err());
        }
        let dispatch_b = ChiefAgentAction::Dispatch { submission_key: "dispatch-b".into(), work: approved_b.work.clone(), worker: WORKER_B.into() };
        assert!(crate::teams::request(&mut bench.registry, 20, AgentAction::Chief { request: dispatch_b.clone() }).unwrap().result.is_err());
        let assigned_a = agent(&mut bench, 20, dispatch_a.clone());
        let assignment = assigned_a["assignment"].as_str().unwrap().to_string();
        checkpoint(&mut bench, restart);
        assert_eq!(agent(&mut bench, 20, dispatch_a), assigned_a);
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_assignments"), 1);
        let delivery = native_inbox(&mut bench, 40, WORKER_A);
        assert_eq!(delivery.len(), 1);
        assert!(delivery[0]["text"].as_str().unwrap().contains(&pending["approval_hash"].as_str().unwrap()));
        checkpoint(&mut bench, restart);
        let receipt = AgentAction::Receipt { dispatch_id: delivery[0]["metadata"]["cockpit_team"]["dispatch_id"].as_str().unwrap().into(),
            outcome: TransportOutcome::Received { native_id: delivery[0]["id"].as_str().unwrap().into() } };
        assert!(!crate::teams::request(&mut bench.registry, 50, receipt.clone()).is_ok_and(|reply| reply.result.is_ok()));
        crate::teams::request(&mut bench.registry, 40, receipt).unwrap().result.unwrap();
        assert!(native_inbox(&mut bench, 40, WORKER_A).is_empty());
        bench.registry.report_from(parse_session(&json!({"session":WORKER_A,"agent":"opencode","project":"project-a","state":"working"}).to_string()).unwrap(), 40).unwrap();
        fresh_facts(&mut bench);
        let accept = ChiefAgentAction::Accept { assignment: assignment.clone() };
        assert!(crate::teams::request(&mut bench.registry, 50, AgentAction::Chief { request: accept.clone() }).unwrap().result.is_err());
        let accepted = agent(&mut bench, 40, accept.clone());
        checkpoint(&mut bench, restart);
        assert_eq!(agent(&mut bench, 40, accept), accepted);
        agent(&mut bench, 40, ChiefAgentAction::Status { assignment: Some(assignment.clone()), summary: "Tests passed (unverified worker prose)".into() });
        assert_eq!(work_state(&mut bench, &approved_a.work), "approved");

        let copies = bench.fixture.root.join("reviewed-patches");
        fs::create_dir(&copies).unwrap();
        fs::write(copies.join("0001.patch"), format!("From {COMMIT} Mon Sep 17 00:00:00 2001\ncontrolled patch\n")).unwrap();
        let token = cockpit::chief::evidence::prepare_apply(bench.registry.teams.as_mut().unwrap(), &assignment, live.to_str().unwrap(), &copies, crate::teams::now_ms()).unwrap();
        checkpoint(&mut bench, restart);
        fs::write(&git_boundary.phase, "after").unwrap();
        for _ in 0..2 { cockpit::chief::evidence::finish_apply(bench.registry.teams.as_mut().unwrap(), &token, true, crate::teams::now_ms()).unwrap(); }
        checkpoint(&mut bench, restart);
        assert_eq!(work_state(&mut bench, &approved_a.work), "approved");
        let binding = cockpit::chief::evidence::RunBinding { assignment: assignment.clone(), gate: GateKind::Tests };
        let run = cockpit::chief::evidence::prepare_run(bench.registry.teams.as_ref().unwrap(), &binding, "make test", live.to_str().unwrap()).unwrap();
        checkpoint(&mut bench, restart);
        // The explicit approved command's external result is controlled; no host command is run.
        for _ in 0..2 { cockpit::chief::evidence::finish_run(bench.registry.teams.as_mut().unwrap(), "workflow-test", &run, "make test", 0, crate::teams::now_ms()).unwrap(); }
        checkpoint(&mut bench, restart);
        assert_eq!(work_state(&mut bench, &approved_a.work), "approved", "QA is still required");
        let handoff = AgentAction::Handoff { handoff: cockpit::teams::Handoff {
            task: task_a.clone(), submission_key: "workflow-handoff".into(), branch: "feature/workflow".into(), commit: COMMIT.into(),
            checkout: bench.registry.sessions[WORKER_A].cwd.clone(), summary: "Exact approved prerequisite implemented".into(), tests: "Worker prose is not proof".into(), figcraft: vec![],
        }};
        let handed = crate::teams::request(&mut bench.registry, 40, handoff.clone()).unwrap().result.unwrap();
        checkpoint(&mut bench, restart);
        assert_eq!(crate::teams::request(&mut bench.registry, 40, handoff).unwrap().result.unwrap(), json!({"handoff_id":handed["handoff_id"]}));
        let qa = AgentAction::QaResult { result: cockpit::teams::QaResult {
            submission_key: "workflow-qa".into(), task: task_a, handoff_id: handed["handoff_id"].as_str().unwrap().into(),
            verdict: cockpit::teams::QaVerdict::Pass, tests: vec!["Reviewed the exact handoff; test execution is separately recorded".into()], findings: "No blocking findings".into(),
        }};
        assert!(!crate::teams::request(&mut bench.registry, 10, qa.clone()).is_ok_and(|reply| reply.result.is_ok()));
        let reviewed = crate::teams::request(&mut bench.registry, 30, qa.clone()).unwrap().result.unwrap();
        checkpoint(&mut bench, restart);
        assert_eq!(crate::teams::request(&mut bench.registry, 30, qa).unwrap().result.unwrap(), reviewed);
        assert_eq!(work_state(&mut bench, &approved_a.work), "completed");
        assert_eq!(assignment_state(&mut bench, &assignment), "working", "completion is not safe idle release");
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_sources"), 2);
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_evidence"), 3);
        let evidence_review = snapshot(&mut bench);
        let qa_evidence = evidence_review["records"].as_array().unwrap().iter()
            .find(|record| record["kind"] == "evidence" && record["source"] == "qa").unwrap();
        assert_eq!(qa_evidence["result"]["provenance"], "approved_reviewer_attestation_not_test_execution");
        let apply_source = evidence_review["records"].as_array().unwrap().iter()
            .find(|record| record["kind"] == "source" && record["evidence"]["source"] == "reviewed_apply").unwrap();
        assert_eq!(apply_source["evidence"]["result"]["patches"][0]["source_commit"], COMMIT);
        assert_eq!(apply_source["evidence"]["result"]["live_commit"], "2".repeat(40));
        std::thread::sleep(std::time::Duration::from_millis(2));
        bench.registry.sessions.get_mut(WORKER_A).unwrap().state = "waiting".into();
        bench.registry.sessions.get_mut(WORKER_A).unwrap().since = (crate::teams::now_ms() / 1000 - 600) as f64;
        bench.registry.tick_queues();
        assert_eq!(assignment_state(&mut bench, &assignment), "working", "old native busy facts cannot release ownership");
        fresh_facts(&mut bench);
        bench.registry.tick_queues();
        assert_eq!(assignment_state(&mut bench, &assignment), "completed");
        checkpoint(&mut bench, restart);
        bench.registry.tick_queues();
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_events WHERE kind='safe_idle_release'"), 1);
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_events WHERE kind='work_completed'"), 1);
        assert_eq!(count(&bench, "SELECT count(*) FROM scheduling_reservations WHERE owner='chief'"), 0);

        let assigned_b = agent(&mut bench, 20, dispatch_b.clone());
        checkpoint(&mut bench, restart);
        assert_eq!(agent(&mut bench, 20, dispatch_b), assigned_b);
        let delivery_b = native_inbox(&mut bench, 50, WORKER_B);
        assert_eq!(delivery_b.len(), 1);
        let assignment_b = assigned_b["assignment"].as_str().unwrap().to_string();
        agent(&mut bench, 50, ChiefAgentAction::Accept { assignment: assignment_b.clone() });
        checkpoint(&mut bench, restart);
        std::thread::sleep(std::time::Duration::from_millis(2));
        for _ in 0..5 { bench.registry.tick_queues(); }
        let decisions: Vec<Value> = snapshot(&mut bench)["records"].as_array().unwrap().iter()
            .filter(|record| record["kind"] == "decision" && record["state"] == "open").cloned().collect();
        assert_eq!(decisions.len(), 1, "only the missing exact dependent evidence requires Theo: {decisions:?}");
        assert_eq!(decisions[0]["decision_key"], "missing_evidence");
        let expected_notifications=vec![format!("chief-decision-{}",decisions[0]["id"].as_str().unwrap())];
        assert_eq!(bench.raised().into_iter().filter(|id|!notification_baseline.contains(id)).collect::<Vec<_>>(),expected_notifications);
        checkpoint(&mut bench, restart);
        for _ in 0..3 { bench.registry.tick_queues(); }
        assert_eq!(bench.raised().into_iter().filter(|id|!notification_baseline.contains(id)).collect::<Vec<_>>(),expected_notifications);
        let attest = ChiefHostAction::AttestEvidence { request_id: "dependent-attestation".into(), evidence: Evidence {
            id: "dependent-proof".into(), work: approved_b.work.clone(), kind: GateKind::Tests, commit: COMMIT.into(), source: "host_attestation".into(),
            result: json!({"passed":true,"note":"Controlled explicit host attestation of the approved Tests gate"}),
        }};
        let attested = host(&mut bench, attest.clone());
        checkpoint(&mut bench, restart);
        assert_eq!(host(&mut bench, attest), attested);
        assert_eq!(work_state(&mut bench, &approved_b.work), "completed");
        let decision = snapshot(&mut bench)["records"].as_array().unwrap().iter()
            .find(|record| record["kind"] == "decision" && record["id"] == decisions[0]["id"]).unwrap().clone();
        assert_eq!(decision["state"], "open", "recorded evidence is not a host answer");
        let resolve = ChiefHostAction::Resolve { request_id: "resolve-workflow".into(), decision: decision["id"].as_str().unwrap().into(),
            expected_revision: decision["revision"].as_i64().unwrap(), resolution: "Reviewed the recorded evidence; no additional authority granted".into() };
        let resolved = host(&mut bench, resolve.clone());
        assert_eq!(resolved["grants_authority"], false);
        checkpoint(&mut bench, restart);
        assert_eq!(host(&mut bench, resolve), resolved);
        let disable = ChiefHostAction::Disable { request_id: "disable-workflow".into(), expected_epoch: grant["epoch"].as_str().unwrap().into() };
        let disabled = host(&mut bench, disable.clone());
        checkpoint(&mut bench, restart);
        assert_eq!(host(&mut bench, disable), disabled);
        assert!(context(&mut bench, 20).unwrap().result.is_err());
        assert!(native_inbox(&mut bench, 20, OPENCODE_SESSION).is_empty());
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_assignments"), 2);
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_delivery_attempts WHERE kind='chief_assignment'"), 2);
        assert_eq!(count(&bench, "SELECT count(*) FROM chief_turns"), 1);
        assert!(bench.world.borrow().started.is_empty(), "no recruitment/host execution is allowed");
    }
}

// Execute the generated report contract with the real CLI and authenticated daemon routes.
// Only process identity and detached terminal launch are controlled fixture boundaries.
fn team_cli_script(bench: &mut Bench, peer: u32, script: &str) -> (std::process::Output, Vec<(String, i64)>) {
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
            // A separate connection observes committed evidence before the terminal send arrives.
            let durable = cockpit::teams::store::Connection::open(cockpit::paths::team_database()).unwrap()
                .query_row("SELECT count(*) FROM chief_evidence WHERE source='qa'",[],|row|row.get(0)).unwrap();
            requests.push((payload["action"].as_str().unwrap().into(),durable));
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

fn spawned_qa_report_contract(chief_bound: bool) -> (Bench, String, Option<(WorkRef, WorkRef)>) {
    let mut bench = team_bench(); report_launches(&mut bench);
    bench.world.borrow_mut().default_agent=Some("claude".into());
    let task=TaskKey {project:verified(&mut bench,WORKER_A).project,task_id:"1d3fd318".into()};
    team_cli_action(&mut bench,40,"recruit",json!({"action":"recruit","recruitment":{
        "task":task,"submission_key":"report-recruit","role":"developer","reason":"Review the exact task",
        "recruits":[{"role":"qa","brief":"Review only this task"}]}}));
    let works = if chief_bound {
        fresh_facts(&mut bench);
        verify_admission(&mut bench,OPENCODE_SESSION);
        let chief=verified(&mut bench,OPENCODE_SESSION);
        host(&mut bench,ChiefHostAction::Enable {request_id:"report-enable".into(),session:chief});
        assert_eq!(native_inbox(&mut bench,20,OPENCODE_SESSION).len(),1);
        let pending=proposal(&mut bench,20,"report-work",WorkProposal {task:task.clone(),brief:"Exact reviewed work".into(),
            role:Role::Developer,prerequisites:vec![],gates:vec![GateRequirement {kind:GateKind::Qa,source:"qa".into(),expected:json!({"commit":COMMIT})}]});
        let parent=approve(&mut bench,&pending,"report-approve",false);
        let dependent_project=verified(&mut bench,WORKER_B).project;
        let pending=proposal(&mut bench,20,"report-dependent",WorkProposal {
            task:TaskKey {project:dependent_project,task_id:"2d3fd318".into()},brief:"Dependent waits for exact QA".into(),
            role:Role::Developer,prerequisites:vec![parent.work.clone()],gates:vec![GateRequirement {kind:GateKind::Tests,source:"host_attestation".into(),expected:json!({"commit":COMMIT})}]});
        let dependent=approve(&mut bench,&pending,"report-dependent-approve",false);
        host(&mut bench,ChiefHostAction::AttestEvidence {request_id:"report-dependent-evidence".into(),evidence:Evidence {
            id:"report-dependent-proof".into(),work:dependent.work.clone(),kind:GateKind::Tests,commit:COMMIT.into(),source:"host_attestation".into(),
            result:json!({"passed":true,"note":"Controlled host attestation fixture, not QA test prose"})}});
        assert_eq!(work_state(&mut bench,&dependent.work),"approved");
        Some((parent.work,dependent.work))
    } else {None};
    let folder=bench.registry.sessions[WORKER_A].cwd.clone();
    team_cli_action(&mut bench,40,"handoff",json!({"action":"handoff","handoff":{
        "task":task,"submission_key":"report-handoff","branch":"feature/report","commit":COMMIT,"checkout":folder,
        "summary":"Exact implementation","tests":"Worker prose is not proof","figcraft":[]}}));
    let start=started(&bench).into_iter().find(|command|command.get(5).is_some_and(|role|role=="qa")).unwrap();
    assert_eq!(start[7],COMMIT);
    spawned_session_reports(&mut bench,43,NEW_SESSION,&start[4]);
    assert_eq!(role_of(&mut bench,NEW_SESSION).as_deref(),Some("qa"));
    (bench,start[8].clone(),works)
}

fn filled_report_script(brief: &str, verdict: &str) -> String {
    brief.lines().filter(|line|line.starts_with("printf ")).collect::<Vec<_>>().join("\n")
        .replace("<a new key: letters, digits, - or _>","final-report")
        .replace("<the Developer's session_id from cockpit-team context>",WORKER_A)
        .replace("<pass or findings>",verdict)
        .replace("<what you found>","Reviewed exact commit; worker test prose is not execution proof")
}

#[test]
fn spawned_chief_qa_publishes_structured_verdict_before_terminal_findings_and_dependency_progress() {
    for verdict in ["pass","findings"] {
        let (mut bench,brief,works)=spawned_qa_report_contract(true);
        let (parent,dependent)=works.unwrap();
        let script=filled_report_script(&brief,verdict);
        let (output,requests)=team_cli_script(&mut bench,43,&script);
        assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
        assert_eq!(requests,vec![("qa_result".into(),0),("send".into(),1)],"durable QA must precede terminal send");
        assert!(output.stderr.is_empty());
        let evidence=snapshot(&mut bench)["records"].as_array().unwrap().iter()
            .find(|record|record["kind"]=="evidence" && record["source"]=="qa").unwrap().clone();
        assert_eq!(evidence["work"],json!(parent)); assert_eq!(evidence["commit"],COMMIT);
        assert_eq!(evidence["result"]["verdict"],verdict);
        assert_eq!(evidence["result"]["reviewer"],NEW_SESSION);
        assert_eq!(evidence["result"]["provenance"],"approved_reviewer_attestation_not_test_execution");
        assert_eq!(count(&bench,"SELECT count(*) FROM chief_evidence WHERE kind='tests' AND source='qa'"),0);
        assert_eq!(work_state(&mut bench,&parent),if verdict=="pass" {"completed"} else {"approved"});
        assert_eq!(work_state(&mut bench,&dependent),if verdict=="pass" {"completed"} else {"approved"});
        crate::teams::host_snapshot(&mut bench.registry).unwrap();
        assert!(started(&bench).contains(&vec!["cockpit".into(),"exit".into(),NEW_SESSION.into()]));
        // The QA runtime has exited; its committed final message still reaches the Developer.
        bench.world.borrow_mut().sender_runtimes.remove(&43);
        bench.world.borrow_mut().live_ids.remove(NEW_SESSION);
        let delivery=crate::teams::request(&mut bench.registry,40,AgentAction::Receive).unwrap().result.unwrap();
        assert_eq!(delivery["envelope"]["kind"],"findings");
        assert_eq!(delivery["envelope"]["handoff"]["id"],evidence["result"]["handoff_id"]);
        assert!(delivery["envelope"]["body"].as_str().unwrap().contains("not execution proof"));
    }
}

#[test]
fn failed_spawned_chief_qa_publication_is_visible_and_does_not_send_terminal_findings() {
    let (mut bench,brief,works)=spawned_qa_report_contract(true);
    let handoff: String=bench.registry.teams.as_ref().unwrap().query_row("SELECT handoff_id FROM spawns WHERE session_id=?1",[NEW_SESSION],|row|row.get(0)).unwrap();
    let script=filled_report_script(&brief,"pass").replace(&handoff,&"0".repeat(32));
    let (output,requests)=team_cli_script(&mut bench,43,&script);
    assert!(!output.status.success(),"a failed structured report must fail the generated sequence");
    assert!(!output.stderr.is_empty(),"publication failure must be visible");
    assert_eq!(requests,vec![("qa_result".into(),0)]);
    crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert_eq!(count(&bench,"SELECT count(*) FROM chief_evidence WHERE source='qa'"),0);
    assert_eq!(count(&bench,"SELECT count(*) FROM messages WHERE json_extract(envelope,'$.kind')='findings'"),0);
    assert!(!started(&bench).iter().any(|command|command.get(1).is_some_and(|verb|verb=="exit")));
    let (parent,dependent)=works.unwrap();
    assert_eq!(work_state(&mut bench,&parent),"approved"); assert_eq!(work_state(&mut bench,&dependent),"approved");
}

#[test]
fn ordinary_spawned_qa_keeps_findings_only_reporting_and_automatic_exit() {
    let (mut bench,brief,_)=spawned_qa_report_contract(false);
    let (output,requests)=team_cli_script(&mut bench,43,&filled_report_script(&brief,"pass"));
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    assert_eq!(requests,vec![("send".into(),0)]);
    crate::teams::host_snapshot(&mut bench.registry).unwrap();
    assert!(started(&bench).contains(&vec!["cockpit".into(),"exit".into(),NEW_SESSION.into()]));
    assert_eq!(count(&bench,"SELECT count(*) FROM chief_evidence"),0);
}
