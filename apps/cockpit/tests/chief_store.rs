use cockpit::{chief::store::migrate, teams::store};
use rusqlite::Connection;
use cockpit::chief::{ChiefAgentAction, ChiefHostAction, WorkBrief, WorkRef, WorkerState};
use cockpit::teams::{ErrorCode, Role, TaskKey, VerifiedSession};
mod common;

#[test]
fn chief_migration_keeps_sleep_continuations_available_on_every_reopen() {
    let sandbox = common::sandbox::Sandbox::new();
    let path = sandbox.home.join("protected/teams.db");
    let mut connection = store::open(&path).unwrap();
    assert!(store::sleep::prepared_records(&connection).unwrap().is_empty());
    connection.execute_batch("DROP TABLE sleep_permits;").unwrap();
    store::migrate(&mut connection).unwrap();
    assert!(store::sleep::prepared_records(&connection).unwrap().is_empty());
    approve(&mut connection, brief("aaaaaaaa", 1), "after-migration").result.unwrap();
    drop(connection);
    let connection = store::open(&path).unwrap();
    assert!(store::sleep::prepared_records(&connection).unwrap().is_empty());
    assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 10);
}

fn chief_session(id: &str) -> VerifiedSession {
    if id=="chief" {return VerifiedSession {session_id:"ses_chief000000000000000000000".into(),agent:"opencode".into(),project:"path:/project-a".into(),runtime:format!("runtime-{id}")}}
    VerifiedSession { session_id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into(), agent: "claude".into(), project: "path:/project-a".into(), runtime: format!("runtime-{id}") }
}

fn live(session: &VerifiedSession) -> WorkerState {
    WorkerState { session: session.clone(), role: None, ready: false, blocked_by: vec!["host_role_unbound".into()], observed_at_ms: 10 }
}

fn brief(task: &str, revision: i64) -> WorkBrief {
    WorkBrief { work: WorkRef { task: TaskKey { project: "path:/project-a".into(), task_id: task.into() }, revision },
        brief: "Implement only the approved scope".into(), role: Role::Developer, prerequisites: vec![], gates: vec![] }
}

fn approve(connection: &mut Connection, brief: WorkBrief, request: &str) -> cockpit::teams::TeamReply {
    let approval_hash = brief.approval_hash().unwrap();
    cockpit::chief::store::host(connection, &ChiefHostAction::ApproveWork { request_id: request.into(), expected_revision: brief.work.revision - 1,
        brief, approval_hash }, &[], 10).unwrap()
}

#[test]
fn host_approval_is_revision_bound_and_preserves_briefs_and_dependency_revisions() {
    let mut connection = migrated();
    let first = brief("aaaaaaaa", 1);
    approve(&mut connection, first.clone(), "first").result.unwrap();
    let mut dependent = brief("bbbbbbbb", 1);
    dependent.prerequisites.push(first.work.clone());
    approve(&mut connection, dependent.clone(), "dependent").result.unwrap();
    assert_eq!(approve(&mut connection, first.clone(), "stale").result.unwrap_err().code, ErrorCode::Conflict);
    let mut missing = brief("cccccccc", 1);
    missing.prerequisites.push(brief("dddddddd", 1).work);
    assert_eq!(approve(&mut connection, missing, "missing").result.unwrap_err().code, ErrorCode::Conflict);
    let mut cycle = brief("aaaaaaaa", 2);
    cycle.prerequisites.push(dependent.work);
    assert_eq!(approve(&mut connection, cycle, "cycle").result.unwrap_err().code, ErrorCode::Conflict);
    let mut edited = brief("aaaaaaaa", 2);
    edited.brief = "Narrow revised scope".into();
    approve(&mut connection, edited, "edit").result.unwrap();
    let old: String = connection.query_row("SELECT brief FROM chief_work_revisions WHERE task_id='aaaaaaaa' AND revision=1", [], |row| row.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<WorkBrief>(&old).unwrap().brief, "Implement only the approved scope");
    let reply = cockpit::chief::store::host(&mut connection, &ChiefHostAction::RevokeWork { request_id: "stale-revoke".into(), work: first.work }, &[], 10).unwrap();
    assert_eq!(reply.result.unwrap_err().code, ErrorCode::Conflict);
}

#[test]
fn host_bound_worker_rebind_preserves_immutable_identity_and_history() {
    let mut connection = migrated();
    let session = chief_session("worker");
    let reply = cockpit::chief::store::host(&mut connection, &ChiefHostAction::BindWorker {
        request_id: "bind".into(), session: session.clone(), role: Role::Developer, expected_epoch: None,
    }, &[live(&session)], 10).unwrap().result.unwrap();
    let epoch = reply["epoch"].as_str().unwrap().to_string();
    let mut replacement = session.clone();
    replacement.runtime = "replacement".into();
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::BindWorker { request_id: "rebind".into(), session: replacement.clone(),
        role: Role::Qa, expected_epoch: Some(epoch.clone()) }, &[live(&replacement)], 20).unwrap().result.unwrap();
    assert_eq!(connection.query_row("SELECT runtime FROM chief_worker_bindings WHERE epoch=?1", [&epoch], |row| row.get::<_, String>(0)).unwrap(), session.runtime);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_worker_bindings WHERE session_id=?1", [&session.session_id], |row| row.get::<_, i64>(0)).unwrap(), 2);
    assert!(connection.execute("UPDATE chief_worker_bindings SET runtime='forged' WHERE epoch=?1", [&epoch]).is_err());
}

#[test]
fn chief_reprioritizes_only_approved_unclaimed_work_and_cannot_accept_worker_assignments() {
    let mut connection = migrated();
    let session = chief_session("chief");
    let workers = [live(&session)];
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::Enable { request_id: "enable".into(), session: session.clone() }, &workers, 10).unwrap().result.unwrap();
    let first = brief("aaaaaaaa", 1);
    let second = brief("bbbbbbbb", 1);
    approve(&mut connection, first.clone(), "first").result.unwrap();
    approve(&mut connection, second.clone(), "second").result.unwrap();
    common::chief::admit(&mut connection,&workers,&session,10);
    let order = ChiefAgentAction::Reprioritize { submission_key: "order".into(), ordered: vec![second.work.clone(), first.work.clone()] };
    cockpit::chief::store::agent(&mut connection, &session, &order, &workers, 10).unwrap().result.unwrap();
    let tasks: Vec<String> = connection.prepare("SELECT task_id FROM chief_work_revisions ORDER BY priority").unwrap().query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    assert_eq!(tasks, ["bbbbbbbb", "aaaaaaaa"]);
    connection.execute("INSERT INTO chief_assignments (id,project,task_id,work_revision,worker,worker_runtime,worker_epoch,chief_grant,chief_epoch,state,revision,submission_key,fingerprint,created_at) VALUES ('claim','path:/project-a','aaaaaaaa',1,'worker','runtime','epoch','chief','epoch','working',1,'claim','hash',10)", []).unwrap();
    let reply = cockpit::chief::store::agent(&mut connection, &session, &ChiefAgentAction::Reprioritize { submission_key: "busy".into(), ordered: vec![first.work] }, &workers, 10).unwrap();
    assert_eq!(reply.result.unwrap_err().code, ErrorCode::Conflict);
    assert_eq!(cockpit::chief::store::agent(&mut connection, &session, &ChiefAgentAction::Accept { assignment: "claim".into() }, &workers, 10).unwrap().result.unwrap_err().code, ErrorCode::Unauthorized);
    cockpit::chief::store::agent(&mut connection, &session, &ChiefAgentAction::Status { assignment: None, summary: "Report, not evidence".into() }, &workers, 10).unwrap().result.unwrap();
}

#[test]
fn chief_work_edits_invalidate_dependents_without_rewriting_their_approved_payloads() {
    let mut connection = migrated();
    let first = brief("aaaaaaaa", 1);
    approve(&mut connection, first.clone(), "first").result.unwrap();
    let mut dependent = brief("bbbbbbbb", 1);
    dependent.prerequisites.push(first.work);
    approve(&mut connection, dependent, "dependent").result.unwrap();
    approve(&mut connection, brief("aaaaaaaa", 2), "edited").result.unwrap();
    let state: String = connection.query_row("SELECT state FROM chief_work_revisions WHERE task_id='bbbbbbbb'", [], |row| row.get(0)).unwrap();
    assert_eq!(state, "superseded");
    let encoded: String = connection.query_row("SELECT brief FROM chief_work_revisions WHERE task_id='bbbbbbbb'", [], |row| row.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<WorkBrief>(&encoded).unwrap().prerequisites[0].revision, 1);
}

#[test]
fn chief_priority_moves_selected_unclaimed_work_before_the_remaining_queue() {
    let mut connection = migrated();
    let session = chief_session("chief");
    let workers = [live(&session)];
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::Enable { request_id: "enable".into(), session: session.clone() }, &workers, 10).unwrap().result.unwrap();
    for task in ["aaaaaaaa", "bbbbbbbb", "cccccccc"] { approve(&mut connection, brief(task, 1), task).result.unwrap(); }
    common::chief::admit(&mut connection,&workers,&session,10);
    cockpit::chief::store::agent(&mut connection, &session, &ChiefAgentAction::Reprioritize { submission_key: "front".into(), ordered: vec![brief("cccccccc", 1).work] }, &workers, 10).unwrap().result.unwrap();
    let first: String = connection.query_row("SELECT task_id FROM chief_work_revisions ORDER BY priority,task_id LIMIT 1", [], |row| row.get(0)).unwrap();
    assert_eq!(first, "cccccccc");
}

#[test]
fn chief_host_request_replay_cannot_change_the_approved_role_or_identity() {
    let mut connection = migrated();
    let session = chief_session("worker");
    let mut action = ChiefHostAction::BindWorker { request_id: "bind".into(), session: session.clone(), role: Role::Developer, expected_epoch: None };
    let first = cockpit::chief::store::host(&mut connection, &action, &[live(&session)], 10).unwrap().result.unwrap();
    assert_eq!(cockpit::chief::store::host(&mut connection, &action, &[live(&session)], 20).unwrap().result.unwrap(), first);
    if let ChiefHostAction::BindWorker { role, .. } = &mut action { *role = Role::Qa; }
    assert_eq!(cockpit::chief::store::host(&mut connection, &action, &[live(&session)], 20).unwrap().result.unwrap_err().code, ErrorCode::Conflict);
}

#[test]
fn chief_context_contains_only_approved_bounded_briefs_and_exact_dependencies() {
    let mut connection = migrated();
    let first = brief("aaaaaaaa", 1);
    approve(&mut connection, first.clone(), "first").result.unwrap();
    let mut dependent = brief("bbbbbbbb", 1);
    dependent.prerequisites.push(first.work);
    approve(&mut connection, dependent, "dependent").result.unwrap();
    let session = chief_session("context");
    let workers = [live(&session)];
    cockpit::chief::store::host(&mut connection,&ChiefHostAction::Enable {request_id:"enable-context".into(),session:session.clone()},&workers,10).unwrap().result.unwrap();
    let first = cockpit::chief::store::agent(&mut connection,&session,&ChiefAgentAction::ContextSnapshot {after:0,limit:1,generation:None},&workers,10).unwrap().result.unwrap();
    let generation = first["generation"].as_str().unwrap().to_owned();
    let mut context = |after,limit| cockpit::chief::store::agent(&mut connection,&session,&ChiefAgentAction::ContextSnapshot {after,limit,generation:Some(generation.clone())},&workers,10).unwrap().result;
    let snapshot = context(2,1).unwrap();
    assert_eq!(snapshot["records"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["records"][0]["brief"], "Implement only the approved scope");
    assert_eq!(snapshot["next"], 3);
    assert_eq!(snapshot["has_more"], true);
    let snapshot = context(3,1).unwrap();
    assert_eq!(snapshot["records"][0]["prerequisites"][0]["revision"], 1);
    assert_eq!(snapshot["has_more"], false);
    assert!(!snapshot.to_string().contains("approval_hash"));
    assert!(context(0,51).is_err());
}

#[test]
fn an_approved_standalone_worker_cannot_acquire_a_chief_grant() {
    let mut connection = migrated();
    let session = chief_session("worker");
    let workers = [live(&session)];
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::BindWorker { request_id: "bind".into(), session: session.clone(),
        role: Role::Developer, expected_epoch: None }, &workers, 10).unwrap().result.unwrap();
    let reply = cockpit::chief::store::host(&mut connection, &ChiefHostAction::Enable { request_id: "enable".into(), session }, &workers, 20).unwrap();
    assert_eq!(reply.result.unwrap_err().code, ErrorCode::Unauthorized);
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_grants", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(connection.query_row("SELECT state FROM chief_worker_bindings", [], |row| row.get::<_, String>(0)).unwrap(), "bound");
}

#[test]
fn overlapping_product_state_cannot_authorize_chief_actions_or_resume() {
    for overlap in ["team", "standalone"] {
        let mut connection = migrated();
        let session = chief_session("chief");
        let workers = [live(&session)];
        let epoch = cockpit::chief::store::host(&mut connection, &ChiefHostAction::Enable { request_id: "enable".into(), session: session.clone() }, &workers, 10).unwrap().result.unwrap()["epoch"].as_str().unwrap().to_string();
        // Simulate a legacy overlapping record; authorization must not repair or trust it.
        if overlap == "team" {
            connection.execute("INSERT INTO members VALUES ('task-a',?1,'qa','claude',?2,'member-epoch',NULL)", rusqlite::params![session.session_id, session.runtime]).unwrap();
        } else {
            connection.execute("INSERT INTO chief_worker_bindings VALUES (?1,'claude','path:/project-a',?2,'worker-epoch','developer',1,'bound','original-hash',10)", rusqlite::params![session.session_id, session.runtime]).unwrap();
        }
        for action in [ChiefAgentAction::Context { after: 0, limit: 50 }, ChiefAgentAction::Reprioritize { submission_key: "order".into(), ordered: vec![brief("aaaaaaaa", 1).work] }] {
            assert_eq!(cockpit::chief::store::agent(&mut connection, &session, &action, &workers, 20).unwrap().result.unwrap_err().code, ErrorCode::Unauthorized, "{overlap}");
        }
        cockpit::chief::store::host(&mut connection, &ChiefHostAction::Pause { request_id: "pause".into(), expected_epoch: epoch.clone() }, &workers, 20).unwrap().result.unwrap();
        assert_eq!(cockpit::chief::store::host(&mut connection, &ChiefHostAction::Resume { request_id: "resume".into(), expected_epoch: epoch }, &workers, 30).unwrap().result.unwrap_err().code, ErrorCode::Unauthorized, "{overlap}");
        assert_eq!(connection.query_row("SELECT state FROM chief_grants", [], |row| row.get::<_, String>(0)).unwrap(), "paused");
    }
}

#[test]
fn task1_v3_worker_approvals_upgrade_without_losing_their_identity_or_history() {
    let mut connection = migrated();
    // Recreate the reviewed Task1 shape, which allowed just one row for each session.
    connection.execute_batch("DROP TABLE scheduling_reservations; DROP TABLE chief_admission_verifications;
        DROP TABLE chief_inbox; DROP TABLE chief_observations; DROP TABLE chief_submissions; DROP TABLE inbox_turns;
        DROP TABLE chief_worker_bindings;
        CREATE TABLE chief_worker_bindings (
            session_id TEXT PRIMARY KEY, agent TEXT NOT NULL, project TEXT NOT NULL, runtime TEXT NOT NULL,
            epoch TEXT NOT NULL UNIQUE, role TEXT NOT NULL CHECK(role IN ('designer','developer','qa')),
            revision INTEGER NOT NULL CHECK(revision>0), state TEXT NOT NULL CHECK(state IN ('bound','revoked')),
            approval_hash TEXT NOT NULL, created_at INTEGER NOT NULL);
        CREATE TRIGGER chief_worker_approval_immutable BEFORE UPDATE OF approval_hash ON chief_worker_bindings
            BEGIN SELECT RAISE(ABORT,'chief worker approval hash is immutable'); END;
        PRAGMA user_version=3;").unwrap();
    let session = chief_session("original");
    connection.execute("INSERT INTO chief_worker_bindings VALUES (?1,'claude','path:/project-a',?2,'old-epoch','developer',1,'bound','approved-original',10)",
        rusqlite::params![session.session_id, session.runtime]).unwrap();
    migrate(&mut connection).unwrap();
    let mut replacement = session.clone();
    replacement.runtime = "replacement".into();
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::BindWorker { request_id: "replace".into(), session: replacement.clone(),
        role: Role::Qa, expected_epoch: Some("old-epoch".into()) }, &[live(&replacement)], 20).unwrap().result.unwrap();
    let original: (String, String, String) = connection.query_row("SELECT runtime,approval_hash,state FROM chief_worker_bindings WHERE epoch='old-epoch'", [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(original, (session.runtime, "approved-original".into(), "revoked".into()));
    assert!(connection.execute("UPDATE chief_worker_bindings SET runtime='forged' WHERE epoch='old-epoch'", []).is_err());
    let before = rows(&connection, "sqlite_master");
    migrate(&mut connection).unwrap();
    assert_eq!(rows(&connection, "sqlite_master"), before);
}

fn version(connection: &Connection) -> i64 {
    connection.query_row("PRAGMA user_version", [], |row| row.get(0)).unwrap()
}

fn v2() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA user_version=2;
        CREATE TABLE teams (task_key TEXT PRIMARY KEY, project TEXT NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL);
        INSERT INTO teams VALUES ('task-a','project-a','1d3fd318',7);
        CREATE TABLE members (task_key TEXT NOT NULL, session_id TEXT NOT NULL UNIQUE, role TEXT NOT NULL,
            agent TEXT NOT NULL, runtime TEXT NOT NULL, epoch TEXT NOT NULL, last_idle_turn TEXT, PRIMARY KEY(task_key,role));
        INSERT INTO members VALUES ('task-a','session-a','developer','claude','runtime-a','epoch-a','turn-a');
        CREATE TABLE messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, task_key TEXT NOT NULL,
            sender TEXT NOT NULL, recipient TEXT NOT NULL, sender_epoch TEXT NOT NULL, recipient_epoch TEXT NOT NULL,
            submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL, envelope TEXT NOT NULL, state TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, created_at INTEGER NOT NULL,
            last_error TEXT NOT NULL DEFAULT '', idle_turn TEXT, recipient_runtime TEXT NOT NULL, UNIQUE(sender,submission_key));
        INSERT INTO messages VALUES (17,'message-a','task-a','sender-a','session-a','epoch-s','epoch-a',
            'submission-a','fingerprint-a','{\"body\":\"Approved message\"}','uncertain',2,123,100,'uncertain','turn-a','runtime-a');
        CREATE TABLE spawns (token TEXT PRIMARY KEY, task_key TEXT NOT NULL, role TEXT NOT NULL, agent TEXT NOT NULL,
            directory TEXT NOT NULL, commit_id TEXT, brief TEXT NOT NULL, handoff_id TEXT, status TEXT NOT NULL,
            session_id TEXT, created_at INTEGER NOT NULL, started_at INTEGER);
        INSERT INTO spawns VALUES ('spawn-a','task-a','qa','claude','/checkout','commit-a','Review only','handoff-a',
            'started','session-qa',101,102);").unwrap();
    connection
}

fn rows(connection: &Connection, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    let mut statement = connection.prepare(&format!("SELECT * FROM {table} ORDER BY 1")).unwrap();
    let count = statement.column_count();
    statement.query_map([], |row| (0..count).map(|index| row.get(index)).collect()).unwrap()
        .collect::<Result<_, _>>().unwrap()
}

#[test]
fn migration_preserves_v2_links_messages_and_spawns() {
    let mut connection = v2();
    let before: Vec<_> = ["teams", "members", "messages", "spawns"].map(|table| rows(&connection, table)).into();
    migrate(&mut connection).unwrap();
    assert_eq!(version(&connection), 10);
    for (table, expected) in ["teams", "members", "messages", "spawns"].into_iter().zip(before) {
        assert_eq!(rows(&connection, table), expected, "changed {table}");
    }
}

#[test]
fn migration_reopen_is_idempotent() {
    let mut connection = v2();
    migrate(&mut connection).unwrap();
    let schema = rows(&connection, "sqlite_master");
    migrate(&mut connection).unwrap();
    assert_eq!(rows(&connection, "sqlite_master"), schema);
    assert_eq!(version(&connection), 10);
}

#[test]
fn future_schema_is_refused() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA user_version=11;").unwrap();
    assert!(migrate(&mut connection).is_err());
    assert_eq!(version(&connection), 11);
    assert!(rows(&connection, "sqlite_master").is_empty());
}

fn migrated() -> Connection {
    let mut connection = v2();
    migrate(&mut connection).unwrap();
    connection
}

#[test]
fn only_one_global_chief_including_a_paused_grant_can_exist() {
    let connection = migrated();
    connection.execute_batch("INSERT INTO chief_grants VALUES ('chief-a','session-a','claude','project-a','runtime-a','epoch-a',1,'paused','hash-a',1);").unwrap();
    assert!(connection.execute_batch("INSERT INTO chief_grants VALUES ('chief-b','session-b','claude','project-b','runtime-b','epoch-b',1,'active','hash-b',2);").is_err());
    connection.execute_batch("UPDATE chief_grants SET state='disabled' WHERE id='chief-a';
        INSERT INTO chief_grants VALUES ('chief-b','session-b','claude','project-b','runtime-b','epoch-b',1,'active','hash-b',2);").unwrap();
}

#[test]
fn worker_approval_hash_is_immutable_while_revocation_preserves_the_approval() {
    let connection = migrated();
    connection.execute_batch("INSERT INTO chief_worker_bindings VALUES
        ('worker-a','claude','project-a','runtime-a','epoch-a','developer',1,'bound','approved-hash',1);").unwrap();
    assert!(connection.execute("UPDATE chief_worker_bindings SET approval_hash='forged' WHERE session_id=?1", ["worker-a"]).is_err());
    assert_eq!(connection.execute("UPDATE chief_worker_bindings SET state='revoked' WHERE session_id=?1", ["worker-a"]).unwrap(), 1);
    let (state, approval_hash): (String, String) = connection.query_row(
        "SELECT state,approval_hash FROM chief_worker_bindings WHERE session_id=?1", ["worker-a"],
        |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(state, "revoked");
    assert_eq!(approval_hash, "approved-hash");
}

fn insert_work(connection: &Connection, task: &str, revision: i64, state: &str) -> rusqlite::Result<usize> {
    connection.execute("INSERT INTO chief_work_revisions (project,task_id,revision,brief,approval_hash,state,created_at)
        VALUES ('project-a',?1,?2,'approved complete brief','hash',?3,1)", rusqlite::params![task, revision, state])
}

fn insert_assignment(connection: &Connection, id: &str, task: &str, revision: i64, worker: &str) -> rusqlite::Result<usize> {
    connection.execute("INSERT INTO chief_assignments (id,project,task_id,work_revision,worker,worker_runtime,worker_epoch,
        chief_grant,chief_epoch,state,revision,submission_key,fingerprint,created_at)
        VALUES (?1,'project-a',?2,?3,?4,'runtime-a','epoch-a','chief-a','chief-epoch','queued',1,?1,'fingerprint',1)",
        rusqlite::params![id, task, revision, worker])
}

#[test]
fn open_claims_are_unique_across_revisions_and_per_worker() {
    let connection = migrated();
    for (task, revision) in [("task-a", 1), ("task-a", 2), ("task-b", 1)] {
        insert_work(&connection, task, revision, "revoked").unwrap();
    }
    insert_assignment(&connection, "claim-a", "task-a", 1, "worker-a").unwrap();
    assert!(insert_assignment(&connection, "claim-b", "task-a", 2, "worker-b").is_err());
    assert!(insert_assignment(&connection, "claim-c", "task-b", 1, "worker-a").is_err());
    connection.execute_batch("UPDATE chief_assignments SET state='cancelled';").unwrap();
    insert_assignment(&connection, "claim-b", "task-a", 2, "worker-a").unwrap();
}

#[test]
fn uncertain_delivery_blocks_other_chief_traffic_to_the_same_recipient() {
    let connection = migrated();
    let insert = |id: &str, recipient: &str| connection.execute("INSERT INTO chief_delivery_attempts
        (id,recipient,recipient_runtime,recipient_epoch,kind,envelope,state,started_at)
        VALUES (?1,?2,'runtime','epoch','coordinate','{}','submitting',1)", [id, recipient]);
    insert("dispatch-a", "worker-a").unwrap();
    connection.execute_batch("UPDATE chief_delivery_attempts SET state='uncertain';").unwrap();
    assert!(insert("dispatch-b", "worker-a").is_err());
    insert("dispatch-c", "worker-b").unwrap();
    connection.execute_batch("UPDATE chief_delivery_attempts SET state='received';").unwrap();
    insert("dispatch-b", "worker-a").unwrap();
}

#[test]
fn approved_work_is_immutable_and_active_capacity_is_bounded() {
    let connection = migrated();
    for index in 0..256 { insert_work(&connection, &format!("task-{index}"), 1, "approved").unwrap(); }
    assert!(insert_work(&connection, "one-too-many", 1, "approved").is_err());
    insert_work(&connection, "revoked-work", 1, "revoked").unwrap();
    assert!(connection.execute_batch("UPDATE chief_work_revisions SET state='approved' WHERE task_id='revoked-work';").is_err());
    assert!(connection.execute_batch("UPDATE chief_work_revisions SET brief='wider scope' WHERE task_id='task-0';").is_err());
    assert!(connection.execute_batch("UPDATE chief_work_revisions SET approval_hash='forged' WHERE task_id='task-0';").is_err());
}

#[test]
fn open_decisions_are_bounded_and_deduplicated() {
    let connection = migrated();
    let insert = |id: &str, key: &str| connection.execute("INSERT INTO chief_decisions
        (id,scope_key,decision_key,question,recommendation,state,revision,created_at)
        VALUES (?1,'global',?2,'What next?','Wait','open',1,1)", [id, key]);
    insert("decision-0", "key-0").unwrap();
    assert!(insert("duplicate", "key-0").is_err());
    for index in 1..100 { insert(&format!("decision-{index}"), &format!("key-{index}")).unwrap(); }
    assert!(insert("overflow", "overflow").is_err());
    connection.execute_batch("UPDATE chief_decisions SET state='resolved' WHERE id='decision-0';").unwrap();
    insert("replacement", "key-0").unwrap();
    assert!(connection.execute_batch("UPDATE chief_decisions SET state='open' WHERE id='decision-0';").is_err());
}

#[test]
fn failed_migration_rolls_back_all_ddl_and_version_changes() {
    let mut connection = v2();
    connection.execute_batch("CREATE TABLE chief_assignments (injected_failure TEXT);").unwrap();
    let before = rows(&connection, "sqlite_master");
    assert!(migrate(&mut connection).is_err());
    assert_eq!(version(&connection), 2);
    assert_eq!(rows(&connection, "sqlite_master"), before);
    assert!(connection.is_autocommit());
}

#[test]
fn open_uses_current_migration_and_keeps_private_path_checks() {
    // Exercise the real protected open path, rather than only an in-memory migration.
    let path = std::env::temp_dir().join(format!("chief-store-{}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let database = path.join("teams.sqlite");
    let connection = store::open(&database).unwrap();
    assert_eq!(version(&connection), 10);
    drop(connection);
    assert_eq!(version(&store::open(&database).unwrap()), 10);
    std::fs::set_permissions(&database, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(store::open(&database).is_err());
    std::fs::remove_file(&database).unwrap();
    for suffix in ["teams.sqlite-wal", "teams.sqlite-shm"] {
        match std::fs::remove_file(path.join(suffix)) {
            Ok(()) => {}, Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}, Err(error) => panic!("{error}"),
        }
    }
    std::fs::remove_dir(&path).unwrap();
}
