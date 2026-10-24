use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn concurrent_open_reads_schema_version_after_the_migration_writer_commits() {
    static WAITING: AtomicBool = AtomicBool::new(false);
    let directory = std::env::temp_dir().join(format!("cockpit-migration-{}", crate::util::token_hex(6)));
    let path = directory.join("teams.sqlite");
    let mut connection = open(&path).unwrap();
    connection.execute_batch("PRAGMA user_version=1;").unwrap();
    let mut other = Connection::open(&path).unwrap();
    other.busy_handler(Some(|_| {
        WAITING.store(true, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1));
        true
    })).unwrap();
    // The migrator has completed its DDL but has not yet published the version.
    let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
    transaction.execute_batch("PRAGMA user_version=12;").unwrap();
    let reader = std::thread::spawn(move || {
        migrate(&mut other)?;
        Ok::<_, TeamError>(other)
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !WAITING.load(Ordering::SeqCst) && !reader.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let waited = WAITING.load(Ordering::SeqCst);
    transaction.commit().unwrap();
    let opened = reader.join().unwrap().unwrap();
    assert!(waited, "initialization did not contend with the schema writer");
    assert_eq!(opened.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0)).unwrap(), 12);
    drop(opened);
    drop(connection);
    fs::remove_file(path).unwrap();
    fs::remove_dir(directory).unwrap();
}
