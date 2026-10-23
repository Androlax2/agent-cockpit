use cockpit::chief::{store, ChiefHostAction, Evidence, GateKind, GateRequirement, WorkBrief, WorkRef};
use cockpit::teams::{Role, TaskKey};
use rusqlite::Connection;
use serde_json::json;
mod common;
use common::sandbox::Sandbox;
use std::{fs,process::Command};

const COMMIT: &str = "1111111111111111111111111111111111111111";

fn brief() -> WorkBrief {
    WorkBrief { work: WorkRef { task: TaskKey { project: "path:/project".into(), task_id: "aaaaaaaa".into() }, revision: 1 },
        brief: "Approved scope".into(), role: Role::Developer, prerequisites: vec![], gates: vec![GateRequirement {
            kind: GateKind::Tests, source: "host_attestation".into(), expected: json!({"commit":COMMIT}) }] }
}

fn setup() -> Connection {
    let mut connection = Connection::open_in_memory().unwrap();
    store::migrate(&mut connection).unwrap();
    let brief = brief();
    store::host(&mut connection, &ChiefHostAction::ApproveWork { request_id:"approve".into(), approval_hash:brief.approval_hash().unwrap(),
        brief, expected_revision:0 }, &[], 1).unwrap().result.unwrap();
    connection
}

fn evidence() -> Evidence {
    Evidence { id:"attest-1".into(), work:brief().work, kind:GateKind::Tests, commit:COMMIT.into(), source:"host_attestation".into(),
        result:json!({"passed":true,"note":"Theo verified the exact revision and tests"}) }
}

#[test]
fn a_host_attestation_completes_the_exact_revision_once_without_todo_or_receipt() {
    let mut connection = setup();
    let reply = store::host(&mut connection, &ChiefHostAction::AttestEvidence { request_id:"attest".into(), evidence:evidence() }, &[], 2).unwrap();
    assert_eq!(reply.result.unwrap()["completed"],json!([brief().work]));
    let state: String = connection.query_row("SELECT state FROM chief_work_revisions", [], |row|row.get(0)).unwrap();
    assert_eq!(state, "completed");
    assert!(store::complete_ready(&mut connection, 3).unwrap().is_empty());
}

#[test]
fn missing_failed_wrong_source_commit_and_revision_never_complete() {
    for mutation in ["missing", "failed", "source", "commit", "revision"] {
        let mut connection = setup();
        let mut result = evidence();
        match mutation {
            "failed" => result.result["passed"] = json!(false),
            "source" => result.source = "qa".into(),
            "commit" => result.commit = "2".repeat(40),
            "revision" => result.work.revision = 2,
            _ => {},
        }
        if mutation != "missing" {
            let _ = store::host(&mut connection, &ChiefHostAction::AttestEvidence { request_id:"attest".into(), evidence:result }, &[], 2);
        }
        assert!(store::complete_ready(&mut connection, 3).unwrap().is_empty(), "{mutation}");
        assert_eq!(connection.query_row("SELECT state FROM chief_work_revisions", [], |row|row.get::<_,String>(0)).unwrap(), "approved", "{mutation}");
    }
}

#[test]
fn caller_supplied_source_ids_cannot_invent_host_results() {
    let mut connection = setup();
    let reply = store::host(&mut connection, &ChiefHostAction::ImportEvidence { request_id:"import".into(), work:brief().work,
        gate:GateKind::Tests, source_id:"forged".into() }, &[], 2).unwrap();
    assert!(reply.result.is_err());
    assert!(store::complete_ready(&mut connection, 3).unwrap().is_empty());
}

#[test]
fn new_approvals_accept_only_the_exact_current_completed_prerequisite() {
    let mut connection = setup();
    store::host(&mut connection, &ChiefHostAction::AttestEvidence { request_id:"attest".into(), evidence:evidence() }, &[], 2).unwrap().result.unwrap();
    let mut dependent = brief();
    dependent.work.task.task_id = "bbbbbbbb".into();
    dependent.prerequisites = vec![brief().work];
    store::host(&mut connection, &ChiefHostAction::ApproveWork { request_id:"dependent".into(), approval_hash:dependent.approval_hash().unwrap(),
        brief:dependent, expected_revision:0 }, &[], 3).unwrap().result.unwrap();
    store::host(&mut connection,&ChiefHostAction::RevokeWork {request_id:"revoke".into(),work:brief().work},&[],4).unwrap().result.unwrap();
    let mut next = brief(); next.work.task.task_id = "cccccccc".into(); next.prerequisites = vec![brief().work];
    assert!(store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"historical".into(),approval_hash:next.approval_hash().unwrap(),brief:next,expected_revision:0},&[],5).unwrap().result.is_err());
}

#[test]
fn gates_cannot_be_changed_to_not_applicable_after_approval() {
    let mut connection = setup();
    let mut changed = brief(); changed.gates.clear();
    let action = ChiefHostAction::ApproveWork {request_id:"waive".into(),approval_hash:changed.approval_hash().unwrap(),brief:changed,expected_revision:0};
    assert!(store::host(&mut connection,&action,&[],2).unwrap().result.is_err());
    assert!(store::complete_ready(&mut connection,3).unwrap().is_empty());
}

#[test]
fn protected_sources_replay_after_restart_and_conflicting_ids_are_refused() {
    let mut connection = setup();
    let mut result = evidence();
    result.id = "source-1".into();
    store::publish_source(&mut connection, &result, 2).unwrap();
    store::recover(&mut connection, 3).unwrap();
    store::replay_sources(&mut connection, 3).unwrap();
    store::replay_sources(&mut connection, 4).unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_evidence", [], |row|row.get::<_,i64>(0)).unwrap(), 1);
    result.result["passed"] = json!(false);
    assert!(store::publish_source(&mut connection, &result, 5).is_err());
}

#[test]
fn failed_evidence_invalidates_completed_prerequisites_and_unsent_dependents() {
    let mut connection = setup();
    store::host(&mut connection, &ChiefHostAction::AttestEvidence { request_id:"attest".into(), evidence:evidence() }, &[], 2).unwrap().result.unwrap();
    let mut dependent = brief();
    dependent.work.task.task_id = "bbbbbbbb".into();
    dependent.prerequisites = vec![brief().work];
    store::host(&mut connection, &ChiefHostAction::ApproveWork { request_id:"dependent".into(), approval_hash:dependent.approval_hash().unwrap(), brief:dependent, expected_revision:0 }, &[], 3).unwrap().result.unwrap();
    let mut failed = evidence();
    failed.id = "attest-failure".into();
    failed.result["passed"] = json!(false);
    store::host(&mut connection, &ChiefHostAction::AttestEvidence { request_id:"invalidate".into(), evidence:failed }, &[], 4).unwrap().result.unwrap();
    assert!(store::complete_ready(&mut connection, 5).unwrap().is_empty());
    assert_eq!(connection.query_row("SELECT state FROM chief_work_revisions WHERE task_id='aaaaaaaa'", [], |row|row.get::<_,String>(0)).unwrap(), "approved");
    assert_eq!(connection.query_row("SELECT state FROM chief_work_revisions WHERE task_id='bbbbbbbb'", [], |row|row.get::<_,String>(0)).unwrap(), "superseded");
}

#[test]
fn run_identity_rejects_a_sandbox_checkout_before_invoking_git() {
    let connection = setup();
    let binding = cockpit::chief::evidence::RunBinding { assignment:"missing".into(), gate:GateKind::Tests };
    assert!(cockpit::chief::evidence::prepare_run(&connection,&binding,"make test","/home/theo/IdeaProjects/agent-cockpit").is_err());
}

#[test]
fn explicit_host_fallback_cannot_substitute_for_the_original_exact_gate() {
    for (kind,source) in [(GateKind::Tests,"host_run"),(GateKind::Qa,"qa"),(GateKind::Apply,"reviewed_apply")] {
        let mut connection = Connection::open_in_memory().unwrap();
        store::migrate(&mut connection).unwrap();
        let mut approved = brief();
        approved.gates[0].kind = kind;
        approved.gates[0].source = source.into();
        approved.gates[0].expected = if source=="host_run" {json!({"commit":COMMIT,"command_fingerprint":cockpit::audit::sha256_hex(b"make test"),"cwd":"/protected/reviewed"})} else {json!({"commit":COMMIT})};
        store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"approve".into(),approval_hash:approved.approval_hash().unwrap(),brief:approved.clone(),expected_revision:0},&[],1).unwrap().result.unwrap();
        let mut attestation = evidence();
        attestation.kind = kind;
        attestation.result["attested_source"] = json!(source);
        attestation.result["expected"] = approved.gates[0].expected.clone();
        assert!(store::host(&mut connection,&ChiefHostAction::AttestEvidence {request_id:"fallback".into(),evidence:attestation},&[],2).unwrap().result.is_err());
        assert_eq!(connection.query_row("SELECT state FROM chief_work_revisions",[],|row|row.get::<_,String>(0)).unwrap(),"approved");
    }
}

#[test]
fn reviewed_apply_producer_records_install_failure_and_rehashed_patch_correspondence() {
    for case in ["success","install-failed","changed-patch"] {
        let installed = case != "install-failed";
        let sandbox = Sandbox::new();
        let live = sandbox.home.join("agent-cockpit");
        fs::create_dir_all(&live).unwrap();
        let git = |path: &std::path::Path,args: &[&str]| {
            let output = Command::new("git").arg("-C").arg(path).args(args).env("GIT_AUTHOR_NAME","Test").env("GIT_AUTHOR_EMAIL","test@example.com")
                .env("GIT_COMMITTER_NAME","Test").env("GIT_COMMITTER_EMAIL","test@example.com").output().unwrap();
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
            String::from_utf8(output.stdout).unwrap()
        };
        git(&live,&["init","-q"]);
        fs::write(live.join("file"),"base\n").unwrap();
        git(&live,&["add","file"]); git(&live,&["commit","-qm","base"]);
        fs::write(live.join("file"),"changed\n").unwrap();
        git(&live,&["add","file"]); git(&live,&["commit","-qm","reviewed patch"]);
        let commit = git(&live,&["rev-parse","HEAD"]).trim().to_string();
        let patch = git(&live,&["format-patch","-1","--stdout"]);
        git(&live,&["reset","--hard","HEAD~1"]);
        let copies = sandbox.runtime.join("copies"); fs::create_dir(&copies).unwrap();
        let patch_path = copies.join("0001.patch"); fs::write(&patch_path,patch).unwrap();
        let protected = sandbox.root.join("protected"); fs::create_dir(&protected).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&protected,fs::Permissions::from_mode(0o700)).unwrap();
        let mut connection = cockpit::teams::store::open(&protected.join("teams.sqlite")).unwrap();
        let mut approved = brief();
        approved.gates = vec![GateRequirement {kind:GateKind::Apply,source:"reviewed_apply".into(),expected:json!({"commit":commit})}];
        if case == "success" {
            approved.gates.push(GateRequirement {kind:GateKind::Tests,source:"host_run".into(),expected:json!({"commit":commit,"command_fingerprint":cockpit::audit::sha256_hex(b"true"),"cwd":live})});
        }
        store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"approve".into(),approval_hash:approved.approval_hash().unwrap(),brief:approved,expected_revision:0},&[],1).unwrap().result.unwrap();
        connection.execute("INSERT INTO chief_assignments (id,project,task_id,work_revision,worker,worker_runtime,worker_epoch,chief_grant,chief_epoch,state,revision,submission_key,fingerprint,created_at) VALUES ('assignment','path:/project','aaaaaaaa',1,'worker','runtime','epoch','grant','epoch','working',1,'dispatch','hash',1)",[]).unwrap();
        let producer = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_cockpit-team")).args(args).env("HOME",&sandbox.home).env("CLAUDE_AUDIT_DIR",&protected).output().unwrap();
        let prepared = producer(&["evidence-apply-prepare","assignment",live.to_str().unwrap(),copies.to_str().unwrap()]);
        assert!(prepared.status.success(),"{}",String::from_utf8_lossy(&prepared.stderr));
        assert!(String::from_utf8_lossy(&prepared.stderr).contains(&commit));
        let token = String::from_utf8(prepared.stdout).unwrap();
        if case == "changed-patch" {
            let patch = fs::read_to_string(&patch_path).unwrap().replace("+changed","+forged");
            fs::write(&patch_path,patch).unwrap();
        }
        let applied = Command::new("git").arg("-C").arg(&live).args(["am","--quiet",patch_path.to_str().unwrap()])
            .env("GIT_COMMITTER_NAME","Test").env("GIT_COMMITTER_EMAIL","test@example.com").env("GIT_COMMITTER_DATE","2000-01-01T00:00:00Z").output().unwrap();
        assert!(applied.status.success(),"{}",String::from_utf8_lossy(&applied.stderr));
        assert_ne!(git(&live,&["rev-parse","HEAD"]).trim(),commit,"fixture must exercise genuinely rehashed commits");
        let finished = producer(&["evidence-apply-finish",token.trim(),if installed {"true"} else {"false"}]);
        if case == "changed-patch" {
            assert!(!finished.status.success(),"a different patch must not satisfy reviewed correspondence");
            continue;
        }
        assert!(finished.status.success(),"{}",String::from_utf8_lossy(&finished.stderr));
        store::replay_sources(&mut connection,2).unwrap();
        if case == "success" {
            for (id,command,expected_count) in [("0123456789ab","true ",0),("0123456789ac","true",1)] {
            fs::create_dir_all(sandbox.runtime.join("claude-requests")).unwrap();
            let request = json!({"id":id,"command":command,"why":"Run the exact reviewed Tests gate","cwd":live,"started_in":live,"project":"agent-cockpit",
                "answers":"project","requested_at":cockpit::util::now_secs(),"evidence":{"assignment":"assignment","gate":"tests"}});
            fs::write(sandbox.runtime.join(format!("claude-requests/{id}.request")),request.to_string()).unwrap();
            let answers = sandbox.root.join("answers");
            let mut terminal = common::Terminal::start(std::path::Path::new(env!("CARGO_BIN_EXE_cockpit-run-approve")),&[id],
                &[("HOME",&sandbox.home),("CLAUDE_AUDIT_DIR",&protected),("XDG_RUNTIME_DIR",&sandbox.runtime),("CLAUDE_ANSWERS_DIR",&answers),("CLAUDE_AUDIT_JOURNAL",std::path::Path::new("off"))]);
            terminal.pump(10.0,Some(b"Run it?"));
            assert_eq!(common::contains(&terminal.screen,b"Evidence unavailable"),expected_count == 0,"{}",String::from_utf8_lossy(&terminal.screen));
            fs::write(sandbox.runtime.join(format!("cockpit-runs/{id}.remote")),"y").unwrap();
            terminal.wait_for_exit(20.0);
            assert_eq!(connection.query_row("SELECT count(*) FROM chief_sources WHERE json_extract(evidence,'$.source')='host_run'",[],|row|row.get::<_,i64>(0)).unwrap(),expected_count,"{}",String::from_utf8_lossy(&terminal.screen));
            store::replay_sources(&mut connection,3).unwrap();
            }
        }
        assert_eq!(connection.query_row("SELECT state FROM chief_work_revisions",[],|row|row.get::<_,String>(0)).unwrap(),if installed {"completed"} else {"approved"});
        let source: String = connection.query_row("SELECT evidence FROM chief_sources WHERE json_extract(evidence,'$.source')='reviewed_apply'",[],|row|row.get(0)).unwrap();
        let source: serde_json::Value = serde_json::from_str(&source).unwrap();
        assert_eq!(source["result"]["patches"][0]["source_commit"],commit);
        assert_eq!(source["result"]["patches"][0]["live_commit"],git(&live,&["rev-parse","HEAD"]).trim());
    }
}

#[test]
fn real_bound_apply_requires_a_supported_successful_installer() {
    for (case,makefile) in [
        ("success","after-apply:\n\t@true\n"),
        ("parse-error","after-apply\n"),
        ("include-error","include missing-installer.mk\n"),
        ("removed-target","other-target:\n\t@true\n"),
        ("recipe-failed","after-apply:\n\t@exit 1\n"),
    ] {
        let sandbox = Sandbox::new();
        let live = sandbox.home.join("agent-cockpit");
        fs::create_dir_all(&live).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git").arg("-C").arg(&live).args(args).env("GIT_AUTHOR_NAME","Test").env("GIT_AUTHOR_EMAIL","test@example.com")
                .env("GIT_COMMITTER_NAME","Test").env("GIT_COMMITTER_EMAIL","test@example.com").output().unwrap();
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
            String::from_utf8(output.stdout).unwrap()
        };
        git(&["init","-q"]);
        fs::write(live.join("Makefile"),"after-apply:\n\t@echo original installer\n").unwrap();
        git(&["add","Makefile"]); git(&["commit","-qm","base installer"]);
        fs::write(live.join("Makefile"),makefile).unwrap();
        git(&["add","Makefile"]); git(&["commit","-qm","reviewed installer change"]);
        let commit = git(&["rev-parse","HEAD"]).trim().to_string();
        let patch = git(&["format-patch","-1","--stdout"]);
        git(&["reset","--hard","HEAD~1"]);
        let work = sandbox.home.join("IdeaProjects/agent-cockpit");
        fs::create_dir_all(work.join(".outgoing")).unwrap();
        fs::write(work.join(".outgoing/0001.patch"),patch).unwrap();
        let protected = sandbox.root.join("protected");
        let mut connection = cockpit::teams::store::open(&protected.join("teams.sqlite")).unwrap();
        let mut approved = brief();
        approved.gates = vec![GateRequirement {kind:GateKind::Apply,source:"reviewed_apply".into(),expected:json!({"commit":commit})}];
        store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"approve".into(),approval_hash:approved.approval_hash().unwrap(),brief:approved,expected_revision:0},&[],1).unwrap().result.unwrap();
        let mut dependent = brief();
        dependent.work.task.task_id = "bbbbbbbb".into(); dependent.prerequisites = vec![brief().work];
        store::host(&mut connection,&ChiefHostAction::ApproveWork {request_id:"dependent".into(),approval_hash:dependent.approval_hash().unwrap(),brief:dependent,expected_revision:0},&[],1).unwrap().result.unwrap();
        connection.execute("INSERT INTO chief_assignments (id,project,task_id,work_revision,worker,worker_runtime,worker_epoch,chief_grant,chief_epoch,state,revision,submission_key,fingerprint,created_at) VALUES ('assignment','path:/project','aaaaaaaa',1,'worker','runtime','epoch','grant','epoch','working',1,'dispatch','hash',1)",[]).unwrap();
        sandbox.shim("cockpit-team",&format!("exec '{}' \"$@\"",env!("CARGO_BIN_EXE_cockpit-team")));
        let output = sandbox.run_with_input(common::sandbox::bin().join("cockpit"),&["apply",work.to_str().unwrap(),"assignment"],
            &[("CLAUDE_AUDIT_DIR",protected.to_str().unwrap()),("GIT_COMMITTER_NAME","Test"),("GIT_COMMITTER_EMAIL","test@example.com")],"y\n");
        assert_eq!(output.status.success(),case=="success","{case}: {}",String::from_utf8_lossy(&output.stderr));
        assert_eq!(git(&["log","-1","--format=%s"]).trim(),"reviewed installer change","patch must actually apply before the installer fails");
        store::replay_sources(&mut connection,2).unwrap();
        let source: String = connection.query_row("SELECT evidence FROM chief_sources",[],|row|row.get(0)).unwrap();
        let source: serde_json::Value = serde_json::from_str(&source).unwrap();
        assert_eq!(source["result"]["installed"],case=="success","{case}");
        assert_eq!(connection.query_row("SELECT state FROM chief_work_revisions WHERE task_id='aaaaaaaa'",[],|row|row.get::<_,String>(0)).unwrap(),if case=="success" {"completed"} else {"approved"});
        let live_workers = ["ses_aaaaaaaa000000000000000000","ses_bbbbbbbb000000000000000000"].into_iter().map(|session_id|cockpit::chief::WorkerState {
            session:cockpit::teams::VerifiedSession {session_id:session_id.into(),agent:"opencode".into(),project:"path:/project".into(),runtime:format!("runtime-{session_id}")},
            role:Some(Role::Developer),ready:true,blocked_by:vec![],observed_at_ms:3,
        }).collect::<Vec<_>>();
        store::host(&mut connection,&ChiefHostAction::Enable {request_id:"enable".into(),session:live_workers[0].session.clone()},&live_workers,3).unwrap().result.unwrap();
        store::host(&mut connection,&ChiefHostAction::BindWorker {request_id:"bind-dependent".into(),session:live_workers[1].session.clone(),role:Role::Developer,expected_epoch:None},&live_workers,3).unwrap().result.unwrap();
        common::chief::admit(&mut connection,&live_workers,&live_workers[0].session,3);
        let mut dependent_work = brief().work; dependent_work.task.task_id = "bbbbbbbb".into();
        let dispatched = store::agent(&mut connection,&live_workers[0].session,&cockpit::chief::ChiefAgentAction::Dispatch {
            submission_key:"dependent-dispatch".into(),work:dependent_work,worker:live_workers[1].session.session_id.clone(),
        },&live_workers,3).unwrap().result;
        assert_eq!(dispatched.is_ok(),case=="success","failed/unsupported installation must not release dependent work: {case}");
    }
}
