use cockpit::{chief::store::migrate, teams::store};
use rusqlite::Connection;
use cockpit::chief::{ChiefAgentAction, ChiefHostAction, WorkerState};
use cockpit::teams::{ErrorCode, VerifiedSession};
mod common;

fn start_chief(connection: &mut Connection, request: &str, previous: Option<&str>) -> Result<serde_json::Value, cockpit::teams::TeamError> {
    let action: ChiefHostAction = serde_json::from_value(serde_json::json!({"action":"start","request_id":request,"previous_attempt":previous})).expect("Start protocol");
    cockpit::chief::store::host(connection, &action, &[], 100).unwrap().result
}

fn settle(connection: &mut Connection, request: &str, attempt: &str, revision: i64, stopped: bool) -> Result<serde_json::Value, cockpit::teams::TeamError> {
    let action: ChiefHostAction = serde_json::from_value(serde_json::json!({"action":"settle_startup","request_id":request,"attempt":attempt,"expected_revision":revision,
        "evidence":{"kind":"safe_stop","note":"No tmux session or process for the reserved id","confirmed_stopped":stopped}})).expect("SettleStartup protocol");
    cockpit::chief::store::host(connection, &action, &[], 200).unwrap().result
}

#[test]
fn a_host_confirmed_stop_settles_a_stuck_check_so_retry_is_offered() {
    let mut connection = migrated();
    let first = start_chief(&mut connection, "first", None).unwrap();
    connection.execute("UPDATE chief_startup SET state='checking',revision=revision+1", []).unwrap();
    let id = first["id"].as_str().unwrap();
    assert_eq!(settle(&mut connection, "unconfirmed", id, 2, false).unwrap_err().code, ErrorCode::Conflict);
    assert_eq!(settle(&mut connection, "stale", id, 1, true).unwrap_err().code, ErrorCode::Conflict);
    let settled = settle(&mut connection, "settle", id, 2, true).unwrap();
    assert_eq!(settled["state"], "failed");
    assert_eq!(settled["can_retry"], true);
    assert!(start_chief(&mut connection, "retry", Some(id)).is_ok());
}

#[test]
fn an_attempt_whose_chief_was_disabled_is_ended_and_start_needs_no_retry_reference() {
    let mut connection = migrated();
    let first = start_chief(&mut connection, "first", None).unwrap();
    connection.execute("UPDATE chief_startup SET state='active',revision=revision+1", []).unwrap();
    let ended = cockpit::chief::store::startup(&connection).unwrap().unwrap();
    assert_eq!(ended["state"], "ended");
    assert_eq!(ended["can_retry"], false);
    assert!(settle(&mut connection, "settle-ended", first["id"].as_str().unwrap(), 2, true).is_err());
    let second = start_chief(&mut connection, "second", None).unwrap();
    assert_eq!(second["state"], "starting");
    assert_ne!(second["id"], first["id"]);
}

#[test]
fn repeated_and_distinct_start_requests_reserve_only_one_chief() {
    let mut connection = migrated();
    let first = start_chief(&mut connection, "first", None).unwrap();
    assert_eq!(first["state"], "starting");
    assert_eq!(first["can_retry"], false);
    assert_eq!(start_chief(&mut connection, "first", None).unwrap(), first);
    assert_eq!(start_chief(&mut connection, "second", None).unwrap(), first);
    assert_eq!(cockpit::chief::store::snapshot(&connection, &[], 0, 50).unwrap()["startup"], first);
    assert!(cockpit::chief::store::chief_session(&connection).unwrap().is_none());
}

#[test]
fn startup_survives_reopening_without_becoming_retryable() {
    let sandbox = common::sandbox::Sandbox::new();
    let path = sandbox.home.join("protected/startup.db");
    let mut connection = store::open(&path).unwrap();
    let first = start_chief(&mut connection, "first", None).unwrap();
    drop(connection);
    let mut connection = store::open(&path).unwrap();
    assert_eq!(start_chief(&mut connection, "after-reopen", None).unwrap(), first);
}

#[test]
fn startup_cannot_replace_an_enabled_chief() {
    let mut connection = migrated();
    let session = chief_session("chief");
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::Enable { request_id: "enable".into(), session: session.clone() }, &[live(&session)], 10).unwrap().result.unwrap();
    assert_eq!(start_chief(&mut connection, "start", None).unwrap_err().code, ErrorCode::Conflict);
}

#[test]
fn a_retry_cannot_name_an_unknown_attempt() {
    let mut connection = migrated();
    assert_eq!(start_chief(&mut connection, "retry", Some("missing")).unwrap_err().code, ErrorCode::Conflict);
}

#[test]
fn retry_requires_the_current_failed_attempt_and_preserves_its_error() {
    let mut connection = migrated();
    let first = start_chief(&mut connection, "first", None).unwrap();
    connection.execute("UPDATE chief_startup SET state='failed',error='launcher missing',revision=revision+1", []).unwrap();
    let failed = cockpit::chief::store::snapshot(&connection, &[], 0, 50).unwrap()["startup"].clone();
    assert_eq!(failed["error"], "launcher missing");
    assert_eq!(failed["can_retry"], true);
    assert!(start_chief(&mut connection, "late-start", None).is_err());
    let second = start_chief(&mut connection, "retry", first["id"].as_str()).unwrap();
    assert_ne!(second["session_id"], first["session_id"]);
    connection.execute("UPDATE chief_startup SET state='failed'", []).unwrap();
    assert!(start_chief(&mut connection, "stale-retry", first["id"].as_str()).is_err());
}

#[test]
fn startup_changes_invalidate_host_snapshot_pagination() {
    let mut connection = migrated();
    let before = cockpit::chief::store::snapshot_page(&connection, &[], 0, 1, None, 100).unwrap();
    start_chief(&mut connection, "first", None).unwrap();
    let after = cockpit::chief::store::snapshot_page(&connection, &[], 1, 1, before["generation"].as_str(), 100).unwrap();
    assert_eq!(after["invalidated"], true);
}

#[test]
fn concurrent_clients_share_one_startup_reservation() {
    let sandbox = common::sandbox::Sandbox::new();
    let path = sandbox.home.join("protected/concurrent-startup.db");
    drop(store::open(&path).unwrap());
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let clients: Vec<_> = ["client-a", "client-b"].into_iter().map(|request| {
        let mut connection = store::open(&path).unwrap();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            start_chief(&mut connection, request, None).unwrap()
        })
    }).collect();
    let outcomes: Vec<_> = clients.into_iter().map(|client|client.join().unwrap()).collect();
    assert_eq!(outcomes[0], outcomes[1]);
}

#[test]
fn chief_migration_keeps_sleep_continuations_available_on_every_reopen() {
    let sandbox = common::sandbox::Sandbox::new();
    let path = sandbox.home.join("protected/teams.db");
    let mut connection = store::open(&path).unwrap();
    assert!(store::sleep::prepared_records(&connection).unwrap().is_empty());
    connection.execute_batch("DROP TABLE sleep_permits;").unwrap();
    store::migrate(&mut connection).unwrap();
    assert!(store::sleep::prepared_records(&connection).unwrap().is_empty());
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::RequestSummary { request_id: "after-migration".into() }, &[], 10).unwrap().result.unwrap();
    drop(connection);
    let connection = store::open(&path).unwrap();
    assert!(store::sleep::prepared_records(&connection).unwrap().is_empty());
    assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 17);
}

fn chief_session(id: &str) -> VerifiedSession {
    if id=="chief" {return VerifiedSession {session_id:"ses_chief000000000000000000000".into(),agent:"opencode".into(),project:"path:/project-a".into(),runtime:format!("runtime-{id}")}}
    VerifiedSession { session_id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".into(), agent: "claude".into(), project: "path:/project-a".into(), runtime: format!("runtime-{id}") }
}

fn live(session: &VerifiedSession) -> WorkerState {
    WorkerState { session: session.clone(), role: None, ready: true, blocked_by: vec![], observed_at_ms: 10 }
}

#[test]
fn an_overlapping_team_membership_cannot_authorize_chief_actions_or_resume() {
    let mut connection = migrated();
    let session = chief_session("chief");
    let workers = [live(&session)];
    let epoch = cockpit::chief::store::host(&mut connection, &ChiefHostAction::Enable { request_id: "enable".into(), session: session.clone() }, &workers, 10).unwrap().result.unwrap()["epoch"].as_str().unwrap().to_string();
    // Simulate a legacy overlapping record; authorization must not repair or trust it.
    connection.execute("INSERT INTO members VALUES ('task-a',?1,'qa','claude',?2,'member-epoch',NULL)", rusqlite::params![session.session_id, session.runtime]).unwrap();
    for action in [ChiefAgentAction::Context { after: 0, limit: 50 }, ChiefAgentAction::Status { summary: "Report, not evidence".into() }] {
        assert_eq!(cockpit::chief::store::agent(&mut connection, &session, &action, &workers, 20).unwrap().result.unwrap_err().code, ErrorCode::Unauthorized);
    }
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::Pause { request_id: "pause".into(), expected_epoch: epoch.clone() }, &workers, 20).unwrap().result.unwrap();
    assert_eq!(cockpit::chief::store::host(&mut connection, &ChiefHostAction::Resume { request_id: "resume".into(), expected_epoch: epoch }, &workers, 30).unwrap().result.unwrap_err().code, ErrorCode::Unauthorized);
    assert_eq!(connection.query_row("SELECT state FROM chief_grants", [], |row| row.get::<_, String>(0)).unwrap(), "paused");
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
    let columns = if table == "messages" {
        "seq,id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,attempts,next_at,created_at,last_error,idle_turn,recipient_runtime"
    } else { "*" };
    let mut statement = connection.prepare(&format!("SELECT {columns} FROM {table} ORDER BY 1")).unwrap();
    let count = statement.column_count();
    statement.query_map([], |row| (0..count).map(|index| row.get(index)).collect()).unwrap()
        .collect::<Result<_, _>>().unwrap()
}

#[test]
fn migration_preserves_v2_links_messages_and_spawns() {
    let mut connection = v2();
    let before: Vec<_> = ["teams", "members", "messages", "spawns"].map(|table| rows(&connection, table)).into();
    migrate(&mut connection).unwrap();
    assert_eq!(version(&connection), 17);
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
    assert_eq!(version(&connection), 17);
}

#[test]
fn future_schema_is_refused() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA user_version=18;").unwrap();
    assert!(migrate(&mut connection).is_err());
    assert_eq!(version(&connection), 18);
    assert!(rows(&connection, "sqlite_master").is_empty());
}

fn migrated() -> Connection {
    let mut connection = v2();
    migrate(&mut connection).unwrap();
    connection
}

use common::chief::remove_link_schema;

#[test]
fn version_ten_upgrade_preserves_chief_and_team_messages_and_receipts() {
    let mut connection = migrated();
    let session = chief_session("chief");
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::Enable {
        request_id: "enable-v10".into(), session: session.clone(),
    }, &[live(&session)], 10).unwrap().result.unwrap();
    remove_link_schema(&connection);
    connection.execute_batch("INSERT INTO attempts VALUES ('receipt-a','message-a',2,'received','native-a',120,110);").unwrap();
    let tables = connection.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap()
        .query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    let before: Vec<_> = tables.iter().map(|table| rows(&connection, table)).collect();
    migrate(&mut connection).unwrap();
    assert_eq!(version(&connection), 17);
    for (table, expected) in tables.iter().zip(before) {
        assert_eq!(rows(&connection, table), expected, "changed {table}");
    }
    let scope: (String, Option<String>) = connection.query_row("SELECT scope_kind,link_id FROM messages WHERE id='message-a'", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(scope, ("team".into(), None));
    assert_eq!(connection.query_row("SELECT last_idle_turn FROM delivery_recipients WHERE session_id='session-a'", [], |row| row.get::<_, String>(0)).unwrap(), "turn-a");
    migrate(&mut connection).unwrap();
    assert_eq!(version(&connection), 17);
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
    // chief_grants fails the first step; chief_inbox_v13 fails the version 13 step after every earlier step succeeded.
    for injected in ["chief_grants", "chief_inbox_v13"] {
        let mut connection = v2();
        connection.execute_batch(&format!("CREATE TABLE {injected} (injected_failure TEXT);")).unwrap();
        let before = rows(&connection, "sqlite_master");
        assert!(migrate(&mut connection).is_err(), "{injected}");
        assert_eq!(version(&connection), 2, "{injected}");
        assert_eq!(rows(&connection, "sqlite_master"), before, "{injected}");
        assert!(connection.is_autocommit());
    }
}

const DROPPED_DISPATCH_TABLES: [&str; 10] = ["chief_evidence", "chief_assignments", "chief_work_revisions", "chief_work_proposals", "chief_sources",
    "chief_handoffs", "chief_apply_reviews", "chief_qa_results", "chief_submissions", "chief_worker_bindings"];

fn table_exists(connection: &Connection, table: &str) -> bool {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)", [table], |row| row.get(0)).unwrap()
}

fn columns(connection: &Connection, table: &str) -> Vec<String> {
    connection.prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid").unwrap()
        .query_map([table], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
}

fn schema_shape(connection: &Connection) -> Vec<(String, String, Vec<String>)> {
    let objects: Vec<(String, String)> = connection.prepare("SELECT type,name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name").unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    objects.into_iter().map(|(kind, name)| { let found = if kind == "table" { columns(connection, &name) } else { Vec::new() }; (kind, name, found) }).collect()
}

#[test]
fn version_12_with_dispatch_leftovers_upgrades_to_13_keeping_wakes_and_decisions() {
    let mut connection = migrated();
    common::chief::restore_version_12_dispatch_schema(&connection);
    connection.execute_batch("PRAGMA foreign_keys=ON;
        INSERT INTO chief_work_revisions VALUES ('path:/project-a','aaaaaaaa',1,'Brief','hash','approved',0,1);
        INSERT INTO chief_assignments VALUES ('assignment-a','path:/project-a','aaaaaaaa',1,'worker-a','runtime','epoch-w','grant','epoch-a','queued',1,'key','print',1,NULL);
        INSERT INTO chief_evidence VALUES ('evidence-a','path:/project-a','aaaaaaaa',1,'tests','commit','host_run','source','result',1);
        INSERT INTO chief_sources VALUES ('source-a','evidence','hash',0,1);
        INSERT INTO chief_inbox(id,assignment,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at)
            VALUES ('inbox-assignment','assignment-a','epoch-a','session-a','{}','epoch-a','null','{\"kind\":\"chief_assignment\"}','queued',1);
        INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at)
            VALUES ('inbox-wake','epoch-a','session-b','{}','epoch-a','null','{\"kind\":\"chief_wake\"}','queued',1);
        INSERT INTO chief_delivery_attempts (id,assignment,recipient,recipient_runtime,recipient_epoch,kind,envelope,idle_turn,state,started_at)
            VALUES ('attempt-wake','assignment-a','session-b','runtime','epoch-a','chief_wake','{\"kind\":\"chief_wake\"}','idle-1','received',1);
        INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at)
            VALUES ('decision-open','global','question','What next?','Wait','open',1,1);").unwrap();
    assert_eq!(version(&connection), 12);

    migrate(&mut connection).unwrap();

    assert_eq!(version(&connection), 17);
    for table in DROPPED_DISPATCH_TABLES { assert!(!table_exists(&connection, table), "{table} survived"); }
    for table in ["chief_stops", "chief_answers"] { assert!(table_exists(&connection, table), "{table} missing"); }
    for table in ["chief_inbox", "chief_delivery_attempts"] { assert!(!columns(&connection, table).contains(&"assignment".to_string()), "{table} kept assignment"); }
    for column in ["session_id", "stop_id", "category", "suggested_answer"] {
        assert!(columns(&connection, "chief_decisions").contains(&column.to_string()), "decision column {column} missing");
    }
    let text = |sql: &str| connection.query_row(sql, [], |row| row.get::<_, String>(0)).unwrap();
    assert_eq!(text("SELECT state FROM chief_inbox WHERE id='inbox-wake'"), "queued");
    assert_eq!(text("SELECT state FROM chief_inbox WHERE id='inbox-assignment'"), "cancelled");
    assert_eq!(text("SELECT state FROM chief_delivery_attempts WHERE id='attempt-wake'"), "received");
    assert_eq!(text("SELECT state FROM chief_decisions WHERE id='decision-open'"), "open");
    let names: Vec<String> = connection.prepare("SELECT name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'").unwrap()
        .query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    for name in ["chief_inbox_inflight", "chief_one_queued_wake", "chief_one_inflight_recipient", "review_chief_inbox_INSERT", "review_chief_delivery_attempts_DELETE"] {
        assert!(names.contains(&name.to_string()), "{name} missing");
    }
    assert!(names.iter().all(|name| !DROPPED_DISPATCH_TABLES.iter().any(|table| name.contains(table))), "a dropped table left an object behind");
    assert_eq!(connection.prepare("PRAGMA foreign_key_check").unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap().count(), 0);

    let snapshot = |connection: &Connection| (schema_shape(connection), rows(connection, "chief_inbox"), rows(connection, "chief_delivery_attempts"), rows(connection, "chief_decisions"));
    let before = snapshot(&connection);
    migrate(&mut connection).unwrap();
    assert_eq!(before, snapshot(&connection));
    assert_eq!(version(&connection), 17);
}

#[test]
fn a_fresh_database_is_created_at_version_17() {
    let mut fresh = Connection::open_in_memory().unwrap();
    fresh.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    migrate(&mut fresh).unwrap();
    assert_eq!(version(&fresh), 17);
    for table in DROPPED_DISPATCH_TABLES { assert!(!table_exists(&fresh, table), "{table} exists in a fresh database"); }
    let dangling: i64 = fresh.query_row("SELECT count(*) FROM sqlite_master WHERE sql LIKE '%chief_assignments%' OR sql LIKE '%chief_work_revisions%'", [], |row| row.get(0)).unwrap();
    assert_eq!(dangling, 0);

    let mut upgraded = migrated();
    common::chief::restore_version_12_dispatch_schema(&upgraded);
    upgraded.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    migrate(&mut upgraded).unwrap();
    assert_eq!(schema_shape(&fresh), schema_shape(&upgraded));
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
    assert_eq!(version(&connection), 17);
    drop(connection);
    assert_eq!(version(&store::open(&database).unwrap()), 17);
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

#[test]
fn upgrading_clears_dispatch_leftovers() {
    let mut connection = migrated();
    let task = cockpit::teams::TaskKey { project: "path:/project-a".into(), task_id: "aaaaaaaa".into() };
    connection.execute_batch("INSERT INTO scheduling_reservations VALUES ('path:/project-a','aaaaaaaa','worker-a','token-a','chief');
        INSERT INTO chief_decisions(id,scope_key,decision_key,work,question,recommendation,state,revision,created_at)
            VALUES ('decision-work','global','missing_evidence','work-a','What next?','Wait','open',1,1);
        INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at)
            VALUES ('decision-host','host-assignment-a','reconcile','What next?','Wait','open',1,1);
        INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at)
            VALUES ('decision-kept','global','question','What next?','Wait','open',1,1);
        INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at)
            VALUES ('inbox-assignment','epoch-a','session-a','{}','epoch-a','null','{\"kind\":\"chief_assignment\"}','queued',1);
        INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at)
            VALUES ('inbox-wake','epoch-a','session-b','{}','epoch-a','null','{\"kind\":\"chief_wake\"}','queued',1);
        INSERT INTO chief_delivery_attempts (id,recipient,recipient_runtime,recipient_epoch,kind,envelope,idle_turn,state,started_at) VALUES
            ('attempt-submitting','session-a','runtime','epoch-a','chief_assignment','{\"kind\":\"chief_assignment\"}','idle-1','submitting',1),
            ('attempt-uncertain','session-c','runtime','epoch-a','chief_assignment','{\"kind\":\"chief_assignment\"}','idle-1','uncertain',1),
            ('attempt-received','session-d','runtime','epoch-a','chief_assignment','{\"kind\":\"chief_assignment\"}','idle-1','received',1),
            ('attempt-wake','session-b','runtime','epoch-a','chief_wake','{\"kind\":\"chief_wake\"}','idle-1','submitting',1);").unwrap();
    assert!(cockpit::chief::store::reserve_legacy(&mut connection, &task, None, 5).is_err());

    migrate(&mut connection).unwrap();

    let count = |sql: &str| connection.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
    let text = |sql: &str| connection.query_row(sql, [], |row| row.get::<_, String>(0)).unwrap();
    assert_eq!(count("SELECT count(*) FROM scheduling_reservations WHERE owner='chief'"), 0);
    assert_eq!(text("SELECT state FROM chief_decisions WHERE id='decision-work'"), "superseded");
    assert_eq!(text("SELECT state FROM chief_decisions WHERE id='decision-host'"), "superseded");
    assert_eq!(text("SELECT state FROM chief_decisions WHERE id='decision-kept'"), "open");
    assert_eq!(text("SELECT state FROM chief_inbox WHERE id='inbox-assignment'"), "cancelled");
    assert_eq!(text("SELECT state FROM chief_inbox WHERE id='inbox-wake'"), "queued");
    assert_eq!(text("SELECT state FROM chief_delivery_attempts WHERE id='attempt-submitting'"), "cancelled");
    assert_eq!(text("SELECT state FROM chief_delivery_attempts WHERE id='attempt-uncertain'"), "cancelled");
    assert_eq!(text("SELECT state FROM chief_delivery_attempts WHERE id='attempt-received'"), "received");
    assert_eq!(text("SELECT state FROM chief_delivery_attempts WHERE id='attempt-wake'"), "submitting");
    cockpit::chief::store::reserve_legacy(&mut connection, &task, None, 6).unwrap();

    let snapshot = |connection: &Connection| (rows(connection, "scheduling_reservations"), rows(connection, "chief_decisions"),
        rows(connection, "chief_inbox"), rows(connection, "chief_delivery_attempts"));
    let before = snapshot(&connection);
    migrate(&mut connection).unwrap();
    assert_eq!(before, snapshot(&connection));
}

fn queue_leftover_assignment(connection: &Connection, session: &VerifiedSession) {
    connection.execute("INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at)
        VALUES ('inbox-assignment','epoch-a',?1,?2,'epoch-a','null','{\"kind\":\"chief_assignment\"}','queued',1)",
        rusqlite::params![session.session_id, serde_json::to_string(session).unwrap()]).unwrap();
}

#[test]
fn a_queued_leftover_inbox_row_is_cancelled_instead_of_delivered() {
    let mut connection = migrated();
    let session = chief_session("chief");
    queue_leftover_assignment(&connection, &session);
    assert!(cockpit::chief::store::claim(&mut connection, &session, "idle-1", 10).unwrap().is_none());
    let state: String = connection.query_row("SELECT state FROM chief_inbox WHERE id='inbox-assignment'", [], |row| row.get(0)).unwrap();
    assert_eq!(state, "cancelled");
}

#[test]
fn a_receipt_for_a_leftover_delivery_never_opens_a_chief_turn() {
    use cockpit::teams::TransportOutcome;
    let mut connection = migrated();
    let session = chief_session("chief");
    queue_leftover_assignment(&connection, &session);
    connection.execute_batch("UPDATE chief_inbox SET state='submitting';").unwrap();
    connection.execute("INSERT INTO chief_delivery_attempts (id,recipient,recipient_runtime,recipient_epoch,kind,envelope,idle_turn,state,started_at)
        VALUES ('attempt-a',?1,?2,'epoch-a','chief_assignment','{\"kind\":\"chief_assignment\",\"message_id\":\"inbox-assignment\"}','idle-1','submitting',1)",
        rusqlite::params![session.session_id, session.runtime]).unwrap();
    cockpit::chief::store::finish_for(&mut connection, &session, "attempt-a", &TransportOutcome::Received { native_id: "native-a".into() }, 10).unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM chief_turns", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(connection.query_row("SELECT state FROM chief_delivery_attempts WHERE id='attempt-a'", [], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
    assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE id='inbox-assignment'", [], |row| row.get::<_, String>(0)).unwrap(), "cancelled");
}

fn enabled_chief_with_queued_wake() -> (Connection, VerifiedSession) {
    let mut connection = migrated();
    let chief = chief_session("chief");
    let workers = [live(&chief)];
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::Enable { request_id: "enable".into(), session: chief.clone() }, &workers, 10).unwrap().result.unwrap();
    cockpit::chief::store::verify_admission(&mut connection, &chief, "opencode-queued-v1", "Controlled test fixture, not a live support claim", 10).unwrap();
    cockpit::chief::store::observe(&mut connection, &workers, 10).unwrap();
    (connection, chief)
}

fn add_team_member_with_queued_message(connection: &Connection, member: &VerifiedSession) {
    connection.execute_batch("INSERT INTO teams VALUES ('task-x','path:/project-a','bbbbbbbb',1);
        INSERT INTO members VALUES ('task-x','sender-session','designer','claude','runtime-sender','epoch-sender',NULL);").unwrap();
    connection.execute("INSERT INTO members VALUES ('task-x',?1,'developer',?2,?3,'epoch-member',NULL)",
        rusqlite::params![member.session_id, member.agent, member.runtime]).unwrap();
    connection.execute("INSERT INTO messages(id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime)
        VALUES ('message-x','task-x','sender-session',?1,'epoch-sender','epoch-member','key-x','fingerprint-x','{\"body\":\"Note\"}','queued',1,1,?2)",
        rusqlite::params![member.session_id, member.runtime]).unwrap();
}

#[test]
fn a_chief_wake_in_flight_blocks_the_team_inbox_of_the_same_session() {
    use cockpit::teams::TransportOutcome;
    let (mut connection, chief) = enabled_chief_with_queued_wake();
    let wake = cockpit::chief::store::claim(&mut connection, &chief, "idle-1", 10).unwrap().expect("queued wake is delivered");
    assert_eq!(wake.envelope["kind"], "chief_wake");
    add_team_member_with_queued_message(&connection, &chief);
    assert!(store::claim(&mut connection, &chief, "idle-2", std::slice::from_ref(&chief), 10).unwrap().is_none(), "a chief wake is in flight");
    cockpit::chief::store::finish_for(&mut connection, &chief, &wake.id, &TransportOutcome::Received { native_id: "native-wake".into() }, 10).unwrap();
    assert!(store::claim(&mut connection, &chief, "idle-1", std::slice::from_ref(&chief), 10).unwrap().is_none(), "the idle turn was consumed by the wake");
    assert!(store::claim(&mut connection, &chief, "idle-2", std::slice::from_ref(&chief), 10).unwrap().is_some());
}

#[test]
fn a_team_message_in_flight_blocks_chief_delivery_to_the_same_session() {
    use cockpit::teams::TransportOutcome;
    let (mut connection, chief) = enabled_chief_with_queued_wake();
    add_team_member_with_queued_message(&connection, &chief);
    let message = store::claim(&mut connection, &chief, "idle-1", std::slice::from_ref(&chief), 10).unwrap().expect("queued team message is delivered");
    connection.execute("DELETE FROM members WHERE session_id=?1", [&chief.session_id]).unwrap();
    assert!(cockpit::chief::store::claim(&mut connection, &chief, "idle-2", 10).unwrap().is_none(), "a team message is in flight");
    store::finish_for(&mut connection, &chief, &message.id, &TransportOutcome::Received { native_id: "native-message".into() }, 10).unwrap();
    assert!(cockpit::chief::store::claim(&mut connection, &chief, "idle-1", 10).unwrap().is_none(), "the idle turn was consumed by the team message");
    assert!(cockpit::chief::store::claim(&mut connection, &chief, "idle-2", 10).unwrap().is_some());
}

#[test]
fn upgrading_supersedes_delivery_decisions_of_cleared_leftovers_only() {
    let mut connection = migrated();
    connection.execute_batch("INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at)
            VALUES ('inbox-assignment','epoch-a','session-a','{}','epoch-a','null','{\"kind\":\"chief_assignment\"}','uncertain',1);
        INSERT INTO chief_inbox(id,chief_epoch,recipient,recipient_identity,recipient_epoch,work,envelope,state,next_at)
            VALUES ('inbox-wake','epoch-a','session-b','{}','epoch-a','null','{\"kind\":\"chief_wake\"}','uncertain',1);
        INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at)
            VALUES ('decision-assignment','host-delivery-inbox-assignment','delivery','Needs recovery','Inspect','open',1,1);
        INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at)
            VALUES ('decision-wake','host-delivery-inbox-wake','delivery','Needs recovery','Inspect','open',1,1);").unwrap();

    migrate(&mut connection).unwrap();

    let state = |id: &str| connection.query_row("SELECT state FROM chief_decisions WHERE id=?1", [id], |row| row.get::<_, String>(0)).unwrap();
    assert_eq!(state("decision-assignment"), "superseded");
    assert_eq!(state("decision-wake"), "open");
    assert_eq!(connection.query_row("SELECT state FROM chief_inbox WHERE id='inbox-wake'", [], |row| row.get::<_, String>(0)).unwrap(), "uncertain");

    let before = rows(&connection, "chief_decisions");
    migrate(&mut connection).unwrap();
    assert_eq!(before, rows(&connection, "chief_decisions"));
}

fn insert_stop(connection: &Connection, id: &str, session_id: &str, task_id: &str, created_at: i64) {
    connection.execute("INSERT INTO chief_stops(id,session_id,runtime,boundary,project,task_id,bundle,state,created_at) VALUES (?1,?2,'runtime-a',?1,'path:/project-a',?3,'{}','answered',?4)",
        rusqlite::params![id, session_id, task_id, created_at]).unwrap();
}

fn insert_answer(connection: &Connection, id: &str, stop_id: &str, session_id: &str, source: &str, delivery: &str, created_at: i64) {
    connection.execute("INSERT INTO chief_answers(id,stop_id,session_id,question,answer,reason,source,delivery,created_at) VALUES (?1,?2,?3,?4,?5,'Because.',?6,?7,?8)",
        rusqlite::params![id, stop_id, session_id, format!("Question {id}?"), format!("Answer {id}."), source, delivery, created_at]).unwrap();
}

#[test]
fn todays_chief_counts_include_midnight_and_resolved_escalations_beyond_the_answer_cap() {
    use chrono::TimeZone;
    let connection = migrated();
    let midnight = chrono::Local.with_ymd_and_hms(2026, 10, 26, 0, 0, 0).single().unwrap().timestamp_millis();
    insert_stop(&connection, "stop-today", "session-a", "task-a", midnight);
    for index in 0..205 {
        insert_answer(&connection, &format!("today-{index}"), "stop-today", "session-a", "chief", "received", midnight + index);
    }
    for (id, source, delivery, created) in [("yesterday", "chief", "received", midnight - 1),
        ("cancelled", "chief", "cancelled", midnight + 1), ("theo", "theo", "received", midnight + 2)] {
        insert_answer(&connection, id, "stop-today", "session-a", source, delivery, created);
    }
    for (id, category, state, created) in [("resolved", Some("scope"), "resolved", midnight),
        ("old", Some("security"), "open", midnight - 1), ("ordinary", None, "open", midnight + 1)] {
        connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at,category)
            VALUES (?1,?1,?1,'Which?','Pick one.',?2,1,?3,?4)", rusqlite::params![id, state, created, category]).unwrap();
    }
    let page = cockpit::chief::store::snapshot_page(&connection, &[], 0, 50, None, midnight + 3600_000).unwrap();
    assert_eq!(page["today"], serde_json::json!({"since_ms":midnight,"answered":205,"escalated":1}));
    assert_eq!(page["chief_counts"], serde_json::json!([{"session_id":"session-a","count":206}]));
}

fn answer_records(connection: &Connection) -> Vec<serde_json::Value> {
    let mut records = Vec::new();
    let mut after = 0;
    loop {
        let page = cockpit::chief::store::snapshot(connection, &[], after, 50).unwrap();
        records.extend(page["records"].as_array().unwrap().iter().filter(|record| record["kind"] == "answer").cloned());
        if page["has_more"] != true { return records; }
        after = page["next"].as_i64().unwrap();
    }
}

#[test]
fn version_13_upgrades_to_14_with_an_empty_overrule_log() {
    let mut connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    insert_answer(&connection, "answer-a", "stop-a", "session-a", "chief", "received", 20);
    connection.execute_batch("DROP INDEX chief_answers_by_session; DROP INDEX chief_answers_by_created_at; DROP TABLE chief_overrules; PRAGMA user_version=13;").unwrap();
    assert!(!index_exists(&connection, "chief_answers_by_created_at"));
    assert!(!table_exists(&connection, "chief_overrules"));

    migrate(&mut connection).unwrap();

    assert_eq!(version(&connection), 17);
    assert!(table_exists(&connection, "chief_overrules"));
    for index in ["chief_answers_by_session", "chief_answers_by_created_at"] {
        assert!(index_exists(&connection, index), "{index} missing after the upgrade");
    }
    assert_eq!(rows(&connection, "chief_overrules").len(), 0);
    assert_eq!(rows(&connection, "chief_answers").len(), 1);
    let before = (schema_shape(&connection), rows(&connection, "chief_answers"));
    migrate(&mut connection).unwrap();
    assert_eq!(before, (schema_shape(&connection), rows(&connection, "chief_answers")));
    assert_eq!(version(&connection), 17);
}

#[test]
fn an_overrule_needs_an_existing_answer_and_is_one_per_answer() {
    let connection = migrated();
    connection.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    let overrule = |id: &str, answer: &str| connection.execute("INSERT INTO chief_overrules(id,answer_id,session_id,runtime,correction,delivery,created_at) VALUES (?1,?2,'session-a','runtime-a','Do B.','queued',30)", [id, answer]);
    assert!(overrule("overrule-a", "missing").is_err());
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    insert_answer(&connection, "answer-a", "stop-a", "session-a", "chief", "received", 20);
    overrule("overrule-a", "answer-a").unwrap();
    assert!(overrule("overrule-b", "answer-a").is_err());
}

#[test]
fn the_review_lists_chief_answers_newest_first_and_bounded() {
    let connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    for index in 0..250 {
        let delivery = ["queued", "received", "uncertain", "cancelled"][index % 4];
        let source = if index % 5 == 0 { "theo" } else { "chief" };
        insert_answer(&connection, &format!("answer-{index:03}"), "stop-a", "session-a", source, delivery, 100 + index as i64);
    }
    let records = answer_records(&connection);
    assert_eq!(records.len(), 200);
    let ids: Vec<&str> = records.iter().map(|record| record["id"].as_str().unwrap()).collect();
    let expected: Vec<String> = (50..250).rev().map(|index| format!("answer-{index:03}")).collect();
    assert_eq!(ids, expected.iter().map(String::as_str).collect::<Vec<_>>());
    let newest = &records[0];
    assert_eq!(newest["stop_id"], "stop-a");
    assert_eq!(newest["session_id"], "session-a");
    assert_eq!(newest["project"], "path:/project-a");
    assert_eq!(newest["task_id"], "task-a");
    assert_eq!(newest["question"], "Question answer-249?");
    assert_eq!(newest["answer"], "Answer answer-249.");
    assert_eq!(newest["reason"], "Because.");
    assert_eq!(newest["source"], "chief");
    assert_eq!(newest["delivery"], "received");
    assert_eq!(newest["created_at_ms"], 349);
    assert!(newest["overrule"].is_null());
    let deliveries: std::collections::BTreeSet<&str> = records.iter().map(|record| record["delivery"].as_str().unwrap()).collect();
    assert_eq!(deliveries.len(), 4, "every delivery state is listed");
    assert!(records.iter().any(|record| record["source"] == "theo"));
}

#[test]
fn an_overruled_answer_carries_its_correction() {
    let connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    insert_answer(&connection, "answer-a", "stop-a", "session-a", "chief", "received", 20);
    insert_answer(&connection, "answer-b", "stop-a", "session-a", "chief", "received", 21);
    connection.execute("INSERT INTO chief_overrules(id,answer_id,session_id,runtime,correction,delivery,created_at) VALUES ('overrule-a','answer-a','session-a','runtime-a','Do B instead.','queued',30)", []).unwrap();
    let records = answer_records(&connection);
    assert!(records[0]["overrule"].is_null(), "{}", records[0]);
    assert_eq!(records[1]["id"], "answer-a");
    assert_eq!(records[1]["overrule"], serde_json::json!({"id":"overrule-a","correction":"Do B instead.","delivery":"queued","created_at_ms":30}));
}

#[test]
fn the_summary_record_names_the_previous_completed_summary() {
    let connection = migrated();
    let summary = |connection: &Connection| cockpit::chief::store::snapshot(connection, &[], 0, 50).unwrap()["records"].as_array().unwrap().iter()
        .find(|record| record["kind"] == "summary").cloned().unwrap();
    assert!(summary(&connection)["previous_completed_at_ms"].is_null());
    let complete = |id: &str, completed_at: i64| connection.execute("INSERT INTO chief_summary_requests(id,epoch,state,created_at,deadline,completed_at,response) VALUES (?1,'epoch-a','completed',?2,?2,?2,'{}')", rusqlite::params![id, completed_at]).unwrap();
    complete("summary-a", 100);
    assert!(summary(&connection)["previous_completed_at_ms"].is_null());
    complete("summary-b", 200);
    complete("summary-c", 300);
    let record = summary(&connection);
    assert_eq!(record["last_completed_at_ms"], 300);
    assert_eq!(record["previous_completed_at_ms"], 200);
}

fn counts(connection: &Connection) -> Vec<(String, u32, Vec<String>)> {
    cockpit::chief::store::answer_counts(connection).unwrap().into_iter()
        .map(|count| (count.session_id, count.count, count.recent.iter().map(|recent| recent["question"].as_str().unwrap().to_string()).collect())).collect()
}

#[test]
fn answer_counts_count_chief_answers_of_the_current_task_only() {
    let connection = migrated();
    insert_stop(&connection, "stop-old", "session-a", "task-old", 10);
    insert_stop(&connection, "stop-new", "session-a", "task-new", 100);
    insert_answer(&connection, "old-1", "stop-old", "session-a", "chief", "received", 20);
    insert_answer(&connection, "new-1", "stop-new", "session-a", "chief", "queued", 110);
    insert_answer(&connection, "new-2", "stop-new", "session-a", "chief", "received", 120);
    insert_answer(&connection, "new-3", "stop-new", "session-a", "chief", "uncertain", 130);
    insert_answer(&connection, "new-4", "stop-new", "session-a", "chief", "received", 140);
    insert_answer(&connection, "new-theo", "stop-new", "session-a", "theo", "received", 150);
    insert_answer(&connection, "new-cancelled", "stop-new", "session-a", "chief", "cancelled", 160);
    insert_stop(&connection, "stop-b", "session-b", "task-b", 50);
    insert_answer(&connection, "b-1", "stop-b", "session-b", "chief", "received", 60);
    insert_stop(&connection, "stop-c", "session-c", "task-c", 50);
    insert_answer(&connection, "c-theo", "stop-c", "session-c", "theo", "received", 60);

    assert_eq!(counts(&connection), [
        ("session-a".to_string(), 4, vec!["Question new-4?".to_string(), "Question new-3?".to_string(), "Question new-2?".to_string()]),
        ("session-b".to_string(), 1, vec!["Question b-1?".to_string()]),
    ]);
    let recent = &cockpit::chief::store::answer_counts(&connection).unwrap()[0].recent[0];
    assert_eq!(recent, &serde_json::json!({"created_at_ms":140,"question":"Question new-4?","answer":"Answer new-4.","delivery":"received"}));
}

#[test]
fn a_session_that_went_back_to_an_earlier_task_counts_only_that_task() {
    let connection = migrated();
    insert_stop(&connection, "stop-1", "session-a", "task-1", 10);
    insert_stop(&connection, "stop-2", "session-a", "task-2", 20);
    insert_stop(&connection, "stop-3", "session-a", "task-1", 30);
    insert_answer(&connection, "one", "stop-1", "session-a", "chief", "received", 11);
    insert_answer(&connection, "two", "stop-2", "session-a", "chief", "received", 21);
    insert_answer(&connection, "three", "stop-3", "session-a", "chief", "received", 31);
    assert_eq!(counts(&connection), [("session-a".to_string(), 2, vec!["Question three?".to_string(), "Question one?".to_string()])]);
}

fn index_exists(connection: &Connection, index: &str) -> bool {
    connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)", [index], |row| row.get(0)).unwrap()
}

#[test]
fn an_overrule_delivery_must_be_a_known_state() {
    let connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    insert_answer(&connection, "answer-a", "stop-a", "session-a", "chief", "received", 20);
    let overrule = |delivery: &str| connection.execute("INSERT INTO chief_overrules(id,answer_id,session_id,runtime,correction,delivery,created_at) VALUES ('overrule-a','answer-a','session-a','runtime-a','Do B.',?1,30)", [delivery]);
    assert!(overrule("sent").is_err());
    assert!(overrule("received").is_ok());
}

#[test]
fn answers_with_the_same_time_keep_their_insertion_order_newest_first() {
    let connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    for id in ["first", "second", "third"] {
        insert_answer(&connection, id, "stop-a", "session-a", "chief", "received", 50);
    }
    let ids: Vec<String> = answer_records(&connection).iter().map(|record| record["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(ids, ["third", "second", "first"]);
    assert_eq!(counts(&connection), [("session-a".to_string(), 3, vec!["Question third?".to_string(), "Question second?".to_string(), "Question first?".to_string()])]);
}

#[test]
fn answers_since_counts_what_the_chief_did_after_the_given_time() {
    let connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    for (id, source, delivery, created_at) in [("before", "chief", "received", 90), ("sent", "chief", "received", 110), ("waiting", "chief", "queued", 120),
        ("cancelled", "chief", "cancelled", 130), ("theo", "theo", "received", 140), ("uncertain", "chief", "uncertain", 150)] {
        insert_answer(&connection, id, "stop-a", "session-a", source, delivery, created_at);
    }
    let escalate = |id: &str, category: Option<&str>, created_at: i64| connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at,category)
        VALUES (?1,?1,?1,'Which?','Pick one.','open',1,?2,?3)", rusqlite::params![id, created_at, category]).unwrap();
    escalate("old-escalation", Some("scope"), 90);
    escalate("escalation", Some("security"), 115);
    escalate("plain-decision", None, 116);
    connection.execute("INSERT INTO chief_overrules(id,answer_id,session_id,runtime,correction,delivery,created_at) VALUES ('overrule-a','sent','session-a','runtime-a','Do B.','queued',160)", []).unwrap();

    let digest = cockpit::chief::store::answers_since(&connection, 100).unwrap();
    assert_eq!((digest.answered, digest.escalated, digest.overruled), (3, 1, 1));
    let questions: Vec<&str> = digest.lines.iter().map(|line| line.question.as_str()).collect();
    assert_eq!(questions, ["Question uncertain?", "Question waiting?", "Question sent?"]);
    assert_eq!((digest.lines[0].session_id.as_str(), digest.lines[0].answer.as_str(), digest.more), ("session-a", "Answer uncertain.", 0));
}

#[test]
fn answers_since_leaves_cancelled_corrections_out_of_the_overruled_count() {
    let connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    for (answer, overrule, delivery) in [("kept", "overrule-kept", "received"), ("dropped", "overrule-dropped", "cancelled")] {
        insert_answer(&connection, answer, "stop-a", "session-a", "chief", "received", 110);
        connection.execute("INSERT INTO chief_overrules(id,answer_id,session_id,runtime,correction,delivery,created_at) VALUES (?1,?2,'session-a','runtime-a','Do B.',?3,160)",
            [overrule, answer, delivery]).unwrap();
    }
    assert_eq!(cockpit::chief::store::answers_since(&connection, 100).unwrap().overruled, 1, "a cancelled correction never reached the agent");
}

#[test]
fn answers_since_lists_the_newest_five_and_counts_the_rest() {
    let connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    for index in 0..8 {
        insert_answer(&connection, &format!("answer-{index}"), "stop-a", "session-a", "chief", "received", 100 + index);
    }
    let digest = cockpit::chief::store::answers_since(&connection, 0).unwrap();
    assert_eq!((digest.answered, digest.lines.len(), digest.more), (8, 5, 3));
    assert_eq!(digest.lines[0].question, "Question answer-7?");
    assert_eq!(digest.lines[4].question, "Question answer-3?");
}

fn insert_habit(connection: &Connection, id: &str, text: &str, project: Option<&str>, origin_project: Option<&str>, created_at: i64) {
    connection.execute("INSERT INTO chief_habits(id,text,project,origin_project,source,source_kind,source_id,state,created_at,changed_at) VALUES (?1,?2,?3,?4,'overrule','overrule',?5,'active',?6,?6)",
        rusqlite::params![id, text, project, origin_project, format!("overrule-{id}"), created_at]).unwrap();
}

fn habit_action(connection: &mut Connection, action: ChiefHostAction, now: i64) -> Result<serde_json::Value, cockpit::teams::TeamError> {
    cockpit::chief::store::host(connection, &action, &[], now).unwrap().result
}

fn habit_state(connection: &Connection, id: &str) -> (Option<String>, String, i64) {
    connection.query_row("SELECT project,state,changed_at FROM chief_habits WHERE id=?1", [id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap()
}

fn habit_records(connection: &Connection) -> Vec<serde_json::Value> {
    let mut records = Vec::new();
    let mut after = 0;
    loop {
        let page = cockpit::chief::store::snapshot(connection, &[], after, 50).unwrap();
        records.extend(page["records"].as_array().unwrap().iter().filter(|record| record["kind"] == "habit").cloned());
        if page["has_more"] != true { return records; }
        after = page["next"].as_i64().unwrap();
    }
}

#[test]
fn answers_since_lists_the_newest_three_active_habits_and_counts_them_all() {
    let connection = migrated();
    insert_habit(&connection, "before", "Seen before.", None, None, 90);
    for (index, project) in [None, Some("path:/shop"), None, Some("path:/shop")].into_iter().enumerate() {
        insert_habit(&connection, &format!("habit-{index}"), &format!("Habit {index}."), project, project, 110 + index as i64);
    }
    insert_habit(&connection, "gone", "Deleted.", None, None, 200);
    connection.execute("UPDATE chief_habits SET state='deleted' WHERE id='gone'", []).unwrap();

    let digest = cockpit::chief::store::answers_since(&connection, 100).unwrap();

    assert_eq!(digest.new_habits, 4, "deleted habits and those before the since-time are not new");
    let shown: Vec<(&str, Option<&str>)> = digest.habits.iter().map(|habit| (habit.text.as_str(), habit.project.as_deref())).collect();
    assert_eq!(shown, [("Habit 3.", Some("path:/shop")), ("Habit 2.", None), ("Habit 1.", Some("path:/shop"))]);
}

#[test]
fn version_14_upgrades_to_15_with_an_empty_habit_list() {
    let mut connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    insert_answer(&connection, "answer-a", "stop-a", "session-a", "chief", "received", 20);
    connection.execute_batch("DROP INDEX chief_habits_by_state; DROP TABLE chief_habits; ALTER TABLE chief_answers DROP COLUMN habits; PRAGMA user_version=14;").unwrap();
    assert!(!table_exists(&connection, "chief_habits"));

    migrate(&mut connection).unwrap();

    assert_eq!(version(&connection), 17);
    assert!(table_exists(&connection, "chief_habits"));
    assert!(index_exists(&connection, "chief_habits_by_state"));
    assert_eq!(rows(&connection, "chief_habits").len(), 0);
    let habits: String = connection.query_row("SELECT habits FROM chief_answers WHERE id='answer-a'", [], |row| row.get(0)).unwrap();
    assert_eq!(habits, "[]");
    let before = (schema_shape(&connection), rows(&connection, "chief_answers"));
    migrate(&mut connection).unwrap();
    assert_eq!(before, (schema_shape(&connection), rows(&connection, "chief_answers")));
    assert_eq!(version(&connection), 17);
}

#[test]
fn deleting_and_restoring_a_habit() {
    let mut connection = migrated();
    insert_habit(&connection, "habit-a", "Never touch the public API.", Some("path:/project-a"), Some("path:/project-a"), 10);
    let deleted = habit_action(&mut connection, ChiefHostAction::DeleteHabit { request_id: "delete-1".into(), habit: "habit-a".into() }, 500).unwrap();
    assert_eq!((deleted["habit"].as_str(), deleted["state"].as_str()), (Some("habit-a"), Some("deleted")));
    assert_eq!(habit_state(&connection, "habit-a"), (Some("path:/project-a".to_string()), "deleted".to_string(), 500));
    habit_action(&mut connection, ChiefHostAction::RestoreHabit { request_id: "restore-1".into(), habit: "habit-a".into() }, 600).unwrap();
    assert_eq!(habit_state(&connection, "habit-a"), (Some("path:/project-a".to_string()), "active".to_string(), 600));
    let missing = habit_action(&mut connection, ChiefHostAction::DeleteHabit { request_id: "delete-2".into(), habit: "no-such-habit".into() }, 700).unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);
}

#[test]
fn rescoping_toggles_between_its_project_and_everywhere() {
    let mut connection = migrated();
    insert_habit(&connection, "habit-a", "Prefer small commits.", Some("path:/project-a"), Some("path:/project-a"), 10);
    insert_habit(&connection, "habit-b", "Keep replies short.", None, None, 11);
    let rescope = |connection: &mut Connection, request: &str, habit: &str, everywhere: bool, now: i64|
        habit_action(connection, ChiefHostAction::RescopeHabit { request_id: request.into(), habit: habit.into(), everywhere }, now);

    rescope(&mut connection, "rescope-1", "habit-a", true, 500).unwrap();
    assert_eq!(habit_state(&connection, "habit-a"), (None, "active".to_string(), 500));
    rescope(&mut connection, "rescope-2", "habit-a", false, 600).unwrap();
    assert_eq!(habit_state(&connection, "habit-a"), (Some("path:/project-a".to_string()), "active".to_string(), 600));
    let error = rescope(&mut connection, "rescope-3", "habit-b", false, 700).unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(error.message.contains("no project to scope to"), "{}", error.message);
    assert_eq!(habit_state(&connection, "habit-b"), (None, "active".to_string(), 11));
}

#[test]
fn the_review_lists_habits_and_the_habits_an_answer_used() {
    let connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    insert_answer(&connection, "answer-a", "stop-a", "session-a", "chief", "received", 20);
    insert_answer(&connection, "answer-b", "stop-a", "session-a", "chief", "received", 21);
    insert_habit(&connection, "habit-a", "Never touch the public API.", Some("path:/project-a"), Some("path:/project-a"), 30);
    insert_habit(&connection, "habit-b", "Keep replies short.", None, None, 40);
    connection.execute("UPDATE chief_habits SET state='deleted', changed_at=50 WHERE id='habit-b'", []).unwrap();
    connection.execute("UPDATE chief_answers SET habits='[\"habit-a\",\"habit-b\",\"gone\"]' WHERE id='answer-a'", []).unwrap();

    let records = habit_records(&connection);
    assert_eq!(records.iter().map(|record| record["id"].as_str().unwrap()).collect::<Vec<_>>(), ["habit-b", "habit-a"], "newest first, deleted ones included");
    assert_eq!(records[1], serde_json::json!({"kind":"habit","id":"habit-a","text":"Never touch the public API.","project":"path:/project-a","origin_project":"path:/project-a",
        "source":"overrule","source_kind":"overrule","reason":null,"source_id":"overrule-habit-a","state":"active","created_at_ms":30,"changed_at_ms":30,
        "session_id":null,"stopped_at_ms":null}), "an overrule the review no longer holds names no session");
    assert_eq!((records[0]["state"].as_str(), records[0]["project"].is_null(), records[0]["changed_at_ms"].as_i64()), (Some("deleted"), true, Some(50)));

    let answers = answer_records(&connection);
    assert_eq!(answers[0]["id"], "answer-b");
    assert_eq!(answers[0]["habits"], serde_json::json!([]));
    assert_eq!(answers[1]["habits"], serde_json::json!([{"id":"habit-a","text":"Never touch the public API."},{"id":"habit-b","text":"Keep replies short."}]),
        "an unknown habit id is left out");
}

#[test]
fn a_habit_names_the_session_its_source_came_from() {
    let connection = migrated();
    insert_stop(&connection, "stop-a", "session-a", "task-a", 10);
    insert_stop(&connection, "stop-b", "session-b", "task-b", 15);
    insert_answer(&connection, "answer-a", "stop-a", "session-a", "theo", "received", 20);
    insert_answer(&connection, "answer-c", "stop-b", "session-b", "chief", "received", 21);
    connection.execute("INSERT INTO chief_overrules(id,answer_id,session_id,runtime,correction,delivery,created_at) VALUES ('overrule-c','answer-c','session-b','runtime-a','Use C.','queued',22)", []).unwrap();
    for (id, kind, source_id, created_at) in [("habit-o", "overrule", "overrule-c", 30), ("habit-a", "theo_answer", "answer-a", 31), ("habit-m", "theo_message", "stop-b", 32)] {
        connection.execute("INSERT INTO chief_habits(id,text,project,origin_project,source,source_kind,source_id,state,created_at,changed_at) VALUES (?1,'A habit.',NULL,NULL,?2,?3,?4,'active',?5,?5)",
            rusqlite::params![id, if kind == "overrule" { "overrule" } else { "theo_answer" }, kind, source_id, created_at]).unwrap();
    }
    let records = habit_records(&connection);
    let named: Vec<_> = records.iter().map(|record| (record["id"].as_str().unwrap(), record["session_id"].as_str(), record["stopped_at_ms"].as_i64())).collect();
    assert_eq!(named, [("habit-m", Some("session-b"), Some(15)), ("habit-a", Some("session-a"), None), ("habit-o", Some("session-b"), None)]);
}

#[test]
fn the_habit_review_is_the_newest_two_hundred() {
    let connection = migrated();
    for index in 0..250 {
        insert_habit(&connection, &format!("habit-{index:03}"), "A habit.", None, None, 100 + index);
    }
    let records = habit_records(&connection);
    assert_eq!(records.len(), 200);
    assert_eq!(records[0]["id"], "habit-249");
    assert_eq!(records[199]["id"], "habit-050");
}

#[test]
fn habit_changes_move_the_review_generation() {
    let mut connection = migrated();
    insert_habit(&connection, "habit-a", "Prefer small commits.", Some("path:/project-a"), Some("path:/project-a"), 10);
    let generation = |connection: &Connection| cockpit::chief::store::audit_page(connection, &[], 0, 50, None, 100).unwrap()["generation"].as_str().unwrap().to_string();
    let mut seen = vec![generation(&connection)];
    let steps = [
        ChiefHostAction::DeleteHabit { request_id: "g-1".into(), habit: "habit-a".into() },
        ChiefHostAction::RestoreHabit { request_id: "g-2".into(), habit: "habit-a".into() },
        ChiefHostAction::RescopeHabit { request_id: "g-3".into(), habit: "habit-a".into(), everywhere: true },
    ];
    for (index, step) in steps.into_iter().enumerate() {
        habit_action(&mut connection, step, 500 + index as i64).unwrap();
        let current = generation(&connection);
        assert!(!seen.contains(&current), "step {index} did not move the generation");
        seen.push(current);
    }
}

fn open_escalation(connection: &Connection, id: &str) {
    insert_stop(connection, &format!("stop-{id}"), "session-a", "task-a", 10);
    connection.execute("INSERT INTO chief_decisions(id,scope_key,decision_key,question,recommendation,state,revision,created_at,session_id,stop_id,category,suggested_answer)
        VALUES (?1,'chief-epoch',?2,'Restart osd?','The guide blocks the restart.','open',1,10,'session-a',?3,'not_sure','Go on.')",
        rusqlite::params![id, format!("stop-{id}"), format!("stop-{id}")]).unwrap();
}

fn later(connection: &mut Connection, decision: &str, revision: i64, now_ms: i64) -> Result<serde_json::Value, cockpit::teams::TeamError> {
    let action: ChiefHostAction = serde_json::from_value(serde_json::json!({"action":"later_decision","request_id":format!("later-{decision}-{revision}-{now_ms}"),"decision":decision,"expected_revision":revision})).expect("LaterDecision protocol");
    cockpit::chief::store::host(connection, &action, &[], now_ms).unwrap().result
}

fn decision_record(connection: &Connection, id: &str) -> serde_json::Value {
    cockpit::chief::store::snapshot(connection, &[], 0, 50).unwrap()["records"].as_array().unwrap().iter()
        .find(|record| record["kind"] == "decision" && record["id"] == id).cloned().expect("decision projected")
}

#[test]
fn later_hides_an_open_escalation_for_twenty_minutes_without_answering_it() {
    let mut connection = migrated();
    open_escalation(&connection, "decision-a");
    assert_eq!(decision_record(&connection, "decision-a")["later_until_ms"], serde_json::Value::Null);
    later(&mut connection, "decision-a", 1, 1_000).unwrap();
    let record = decision_record(&connection, "decision-a");
    assert_eq!(record["later_until_ms"], 1_201_000);
    assert_eq!(record["state"], "open");
    assert_eq!(record["revision"], 1);
}

#[test]
fn later_refuses_a_stale_or_answered_escalation() {
    let mut connection = migrated();
    open_escalation(&connection, "decision-a");
    assert_eq!(later(&mut connection, "decision-a", 2, 1_000).unwrap_err().code, ErrorCode::Conflict);
    connection.execute("UPDATE chief_decisions SET state='resolved',revision=2 WHERE id='decision-a'", []).unwrap();
    assert_eq!(later(&mut connection, "decision-a", 2, 1_000).unwrap_err().code, ErrorCode::Conflict);
}

#[test]
fn a_populated_version_16_database_upgrades_to_17_keeping_its_decisions() {
    let sandbox = common::sandbox::Sandbox::new();
    let path = sandbox.home.join("protected/v16.db");
    let connection = store::open(&path).unwrap();
    open_escalation(&connection, "decision-a");
    connection.execute_batch("ALTER TABLE chief_decisions DROP COLUMN later_until; PRAGMA user_version=16;").unwrap();
    drop(connection);
    let connection = store::open(&path).unwrap();
    assert_eq!(version(&connection), 17);
    assert_eq!(decision_record(&connection, "decision-a")["suggested_answer"], "Go on.");
}
