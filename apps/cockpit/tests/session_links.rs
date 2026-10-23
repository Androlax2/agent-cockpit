mod common;
use common::sandbox::Sandbox;
use cockpit::{teams::{self, TaskKey, links, store}, todo};
use rusqlite::Connection;
use serde_json::json;
use std::{fs, path::Path};

fn checkout(path: &Path) -> String {
    fs::create_dir_all(path.join(".git")).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn shared_work_requires_an_existing_approved_typed_todo() {
    if std::env::var_os("SESSION_LINKS_LOOKUP_CHILD").is_none() {
        let sandbox = Sandbox::new();
        let output = sandbox.command(std::env::current_exe().unwrap(), &[("SESSION_LINKS_LOOKUP_CHILD", "1")])
            .args(["--exact", "shared_work_requires_an_existing_approved_typed_todo", "--nocapture"]).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let home = cockpit::paths::home();
    let primary = checkout(&home.join("IdeaProjects/project"));
    let other = checkout(&home.join("other/project"));
    let clone = checkout(&cockpit::paths::clones_dir().join("project/feature"));
    let task = TaskKey { project: format!("path:{primary}"), task_id: "1234abcd".into() };
    let item = json!({"id": task.task_id, "project": "project", "cwd": primary, "text": "Joint work", "status": "doing"});
    let path = cockpit::paths::todo();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut cases = vec![("unknown", json!({"items": []})), ("duplicate", json!({"items": [item.clone(), item.clone()]})),
        ("wrong document", json!({"items": {}}))];
    for (name, field, value) in [
        ("proposed", "status", json!("proposed")), ("done", "status", json!("done")),
        ("invalid status", "status", json!("unknown")), ("missing status", "status", json!(null)),
        ("owner mismatch", "project", json!("elsewhere")), ("same basename", "cwd", json!(other)),
        ("missing checkout", "cwd", json!("/nonexistent/shared-work-checkout")), ("empty title", "text", json!("")),
    ] {
        let mut changed = item.clone();
        changed[field] = value;
        cases.push((name, json!({"items": [changed]})));
    }
    for (name, document) in cases {
        fs::write(&path, document.to_string()).unwrap();
        assert!(todo::read_shared_work(&task).is_err(), "accepted {name}");
    }
    fs::write(&path, "{broken").unwrap();
    assert!(todo::read_shared_work(&task).is_err());
    fs::remove_file(&path).unwrap();
    assert!(todo::read_shared_work(&task).is_err());
    fs::create_dir(&path).unwrap();
    assert!(todo::read_shared_work(&task).is_err());
    fs::remove_dir(&path).unwrap();
    for cwd in [primary.as_str(), clone.as_str()] {
        let mut valid = item.clone();
        valid["cwd"] = json!(cwd);
        fs::write(&path, json!({"items": [valid]}).to_string()).unwrap();
        let work = todo::read_shared_work(&task).unwrap();
        assert_eq!(work.task, task);
        assert_eq!(work.status, "doing");
        assert_eq!(work.title, "Joint work");
    }
    let issue = TaskKey { task_id: "owner/repo#12".into(), ..task.clone() };
    assert!(todo::read_shared_work(&issue).is_err());
    let basename = TaskKey { project: "project".into(), ..task };
    assert!(todo::read_shared_work(&basename).is_err());
    assert_eq!(teams::canonical_project(&clone).unwrap(), format!("path:{primary}"));
    assert_ne!(teams::canonical_project(&other).unwrap(), format!("path:{primary}"));
}

fn version_two() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("PRAGMA user_version=2;
        CREATE TABLE teams (task_key TEXT PRIMARY KEY, project TEXT NOT NULL, task_id TEXT NOT NULL, revision INTEGER NOT NULL);
        CREATE TABLE proposals (id TEXT PRIMARY KEY, sender TEXT NOT NULL, submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
            task_key TEXT NOT NULL, project TEXT NOT NULL, task_id TEXT NOT NULL, mapping TEXT NOT NULL, mapping_hash TEXT NOT NULL,
            reason TEXT NOT NULL, revision INTEGER NOT NULL, status TEXT NOT NULL, created_at INTEGER NOT NULL,
            parent_id TEXT, created_by TEXT NOT NULL DEFAULT 'agent', recruits TEXT NOT NULL DEFAULT '[]', spawn_agent TEXT NOT NULL DEFAULT '', UNIQUE(sender,submission_key));
        CREATE TABLE members (task_key TEXT NOT NULL, session_id TEXT NOT NULL UNIQUE, role TEXT NOT NULL, agent TEXT NOT NULL,
            runtime TEXT NOT NULL, epoch TEXT NOT NULL, last_idle_turn TEXT, PRIMARY KEY(task_key,role));
        CREATE TABLE messages (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, task_key TEXT NOT NULL,
            sender TEXT NOT NULL, recipient TEXT NOT NULL, sender_epoch TEXT NOT NULL, recipient_epoch TEXT NOT NULL,
            submission_key TEXT NOT NULL, fingerprint TEXT NOT NULL, envelope TEXT NOT NULL, state TEXT NOT NULL,
            attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, created_at INTEGER NOT NULL,
            last_error TEXT NOT NULL DEFAULT '', idle_turn TEXT, recipient_runtime TEXT NOT NULL, UNIQUE(sender,submission_key));
        CREATE TABLE attempts (id TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages(id), number INTEGER NOT NULL,
            state TEXT NOT NULL, native_id TEXT, finished_at INTEGER, started_at INTEGER NOT NULL DEFAULT 0, UNIQUE(message_id,number));
        INSERT INTO teams VALUES ('task','path:/project','1234abcd',7);
        INSERT INTO proposals (id,sender,submission_key,fingerprint,task_key,project,task_id,mapping,mapping_hash,reason,revision,status,created_at)
            VALUES ('proposal','sender','request','fingerprint','task','path:/project','1234abcd','[]','hash','work',7,'approved',10);
        INSERT INTO members VALUES ('task','sender','developer','claude','runtime','epoch',NULL);
        INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,attempts,next_at,created_at,recipient_runtime)
            VALUES ('message','task','sender','recipient','epoch','peer-epoch','submission','fingerprint','{}','received',1,20,10,'peer-runtime');
        INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime)
            VALUES ('pending','task','sender','recipient','epoch','peer-epoch','queued-submission','pending-fingerprint','{\"body\":\"queued\"}','queued',30,25,'peer-runtime');
        INSERT INTO attempts VALUES ('attempt','message',1,'received','native-receipt',20,15);").unwrap();
    connection
}

fn team_state(connection: &Connection) -> Vec<Vec<String>> {
    ["teams", "proposals", "members", "attempts", "messages"].into_iter().map(|table| {
        let columns = if table == "messages" {
            "seq,id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,attempts,next_at,created_at,last_error,idle_turn,recipient_runtime"
        } else { "*" };
        let mut statement = connection.prepare(&format!("SELECT {columns} FROM {table}")).unwrap();
        let columns = statement.column_count();
        statement.query_map([], |row| Ok((0..columns).map(|column| format!("{:?}", row.get_ref(column).unwrap())).collect::<Vec<_>>().join("|")))
            .unwrap().collect::<Result<Vec<_>, _>>().unwrap()
    }).collect()
}

#[test]
fn link_schema_upgrade_preserves_team_state_and_receipts() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    fs::create_dir(path.parent().unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    let fixture = version_two();
    fixture.execute("VACUUM INTO ?1", [path.to_str().unwrap()]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let team_before = team_state(&fixture);
    let connection = store::open(&path).unwrap();
    let team_after = team_state(&connection);
    assert_eq!(team_before, team_after);
    drop(connection);
    let connection = store::open(&path).unwrap();
    assert_eq!(team_before, team_state(&connection));
    let receipt: (String, String, Option<String>) = connection.query_row(
        "SELECT state,scope_kind,link_id FROM messages WHERE id='message'", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
    assert_eq!(receipt, ("received".into(), "team".into(), None));
    assert!(connection.execute("INSERT INTO messages (id,task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime)
        SELECT 'new-id',task_key,sender,recipient,sender_epoch,recipient_epoch,submission_key,fingerprint,envelope,state,next_at,created_at,recipient_runtime FROM messages WHERE id='message'", []).is_err());
}

#[test]
fn failed_or_future_link_migrations_do_not_partially_upgrade() {
    let mut connection = version_two();
    connection.execute_batch("CREATE TABLE links (wrong TEXT);").unwrap();
    assert!(store::migrate(&mut connection).is_err());
    assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(connection.query_row("SELECT count(*) FROM pragma_table_info('messages') WHERE name='scope_kind'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    let connection = version_two();
    connection.execute_batch("PRAGMA user_version=999;").unwrap();
    assert!(links::migrate(&connection).is_err());
    assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 999);
}

fn approved_link_fixture(connection: &Connection) {
    connection.execute_batch("INSERT INTO link_work VALUES ('shared','path:/project','abcdef12',4,'open',10);
        INSERT INTO link_proposals (id,sender,submission_key,fingerprint,task_key,purpose,revision,mapping_hash,work_generation,status,created_at)
            VALUES ('p1','a','request-1','fingerprint','shared','Joint work',1,'hash-1',4,'approved',10);
        INSERT INTO link_participants VALUES ('p1',0,'a','claude','path:/project','runtime-a');
        INSERT INTO link_participants VALUES ('p1',1,'b','opencode','path:/other','runtime-b');
        INSERT INTO links VALUES ('link-1','p1',1,'hash-1','shared',4,'a','b','epoch-1','active',NULL,20,NULL);").unwrap();
}

#[test]
fn link_schema_binds_exact_immutable_approval_and_one_active_unordered_pair() {
    let mut connection = version_two();
    store::migrate(&mut connection).unwrap();
    approved_link_fixture(&connection);
    for statement in [
        "UPDATE link_proposals SET mapping_hash='changed' WHERE id='p1'",
        "UPDATE link_proposals SET revision=2 WHERE id='p1'",
        "UPDATE link_participants SET runtime='replacement' WHERE proposal_id='p1'",
        "DELETE FROM link_participants WHERE proposal_id='p1'",
        "UPDATE links SET revision=2 WHERE id='link-1'",
        "DELETE FROM link_work WHERE task_key='shared'",
        "UPDATE link_work SET generation=3 WHERE task_key='shared'",
    ] {
        assert!(connection.execute(statement, []).is_err(), "accepted {statement}");
    }
    connection.execute_batch("INSERT INTO link_proposals (id,sender,submission_key,fingerprint,task_key,purpose,revision,mapping_hash,work_generation,status,created_at)
        VALUES ('p2','b','request-2','fingerprint','shared','Joint work',2,'hash-2',4,'approved',30);
        INSERT INTO link_participants VALUES ('p2',0,'b','opencode','path:/other','runtime-b');
        INSERT INTO link_participants VALUES ('p2',1,'a','claude','path:/project','runtime-a');").unwrap();
    for (low, high, hash) in [("b", "a", "hash-2"), ("a", "b", "hash-1"), ("a", "c", "hash-2"), ("a", "b", "hash-2")] {
        assert!(connection.execute("INSERT INTO links VALUES ('link-2','p2',2,?1,'shared',4,?2,?3,'epoch-2','active',NULL,30,NULL)",
            (hash, low, high)).is_err());
    }
    connection.execute("UPDATE links SET state='closed',closure_reason='done',closed_at=40 WHERE id='link-1'", []).unwrap();
    connection.execute("INSERT INTO links VALUES ('link-2','p2',2,'hash-2','shared',4,'a','b','epoch-2','active',NULL,50,NULL)", []).unwrap();
    assert_eq!(connection.query_row("SELECT count(*) FROM teams", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}

#[test]
fn store_open_rolls_back_legacy_and_link_changes_on_migration_failure() {
    let sandbox = Sandbox::new();
    let path = sandbox.root.join("protected/teams.sqlite");
    fs::create_dir(path.parent().unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    let fixture = version_two();
    fixture.execute_batch("CREATE TABLE links (wrong TEXT); PRAGMA user_version=1; ALTER TABLE proposals DROP COLUMN spawn_agent;").unwrap();
    fixture.execute("VACUUM INTO ?1", [path.to_str().unwrap()]).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(store::open(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    assert_eq!(connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    assert_eq!(connection.query_row("SELECT count(*) FROM pragma_table_info('proposals') WHERE name='spawn_agent'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(connection.query_row("SELECT count(*) FROM sqlite_master WHERE name='link_work'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}
