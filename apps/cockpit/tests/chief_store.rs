use cockpit::{chief::store::migrate, teams::store};
use rusqlite::Connection;
use cockpit::chief::{ChiefAgentAction, ChiefHostAction, WorkerState};
use cockpit::teams::{ErrorCode, VerifiedSession};
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
    cockpit::chief::store::host(&mut connection, &ChiefHostAction::RequestSummary { request_id: "after-migration".into() }, &[], 10).unwrap().result.unwrap();
    drop(connection);
    let connection = store::open(&path).unwrap();
    assert!(store::sleep::prepared_records(&connection).unwrap().is_empty());
    assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 12);
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
    assert_eq!(version(&connection), 12);
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
    assert_eq!(version(&connection), 12);
}

#[test]
fn future_schema_is_refused() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA user_version=13;").unwrap();
    assert!(migrate(&mut connection).is_err());
    assert_eq!(version(&connection), 13);
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
    assert_eq!(version(&connection), 12);
    for (table, expected) in tables.iter().zip(before) {
        assert_eq!(rows(&connection, table), expected, "changed {table}");
    }
    let scope: (String, Option<String>) = connection.query_row("SELECT scope_kind,link_id FROM messages WHERE id='message-a'", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(scope, ("team".into(), None));
    assert_eq!(connection.query_row("SELECT last_idle_turn FROM delivery_recipients WHERE session_id='session-a'", [], |row| row.get::<_, String>(0)).unwrap(), "turn-a");
    migrate(&mut connection).unwrap();
    assert_eq!(version(&connection), 12);
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
    assert_eq!(version(&connection), 12);
    drop(connection);
    assert_eq!(version(&store::open(&database).unwrap()), 12);
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
