//! The ordered migration runner: applies each capability's migrations once and refuses schemas
//! it does not know.
mod common;

use cockpit_turso::{Database, Error, Migration, MigrationSet};
use common::{TASK_MIGRATIONS, insert_task, scratch_dir, task_titles};

const TASK_MIGRATIONS_V2: MigrationSet = MigrationSet {
    owner: "tasks",
    migrations: &[
        TASK_MIGRATIONS.migrations[0],
        Migration {
            version: 2,
            sql: "ALTER TABLE tasks ADD COLUMN priority INTEGER NOT NULL DEFAULT 0",
        },
    ],
};

#[tokio::test]
async fn a_new_migration_applies_on_the_next_open_and_keeps_existing_rows() {
    let path = scratch_dir("migration-upgrade").join("state.db");
    let first = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();
    insert_task(&first, "older row").await;
    drop(first);

    let upgraded = Database::open(&path, &[TASK_MIGRATIONS_V2]).await.unwrap();

    let read = upgraded.read().await.unwrap();
    let mut rows = read
        .query("SELECT title, priority FROM tasks", ())
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    assert_eq!(row.get::<String>(0).unwrap(), "older row");
    assert_eq!(row.get::<i64>(1).unwrap(), 0);
}

#[tokio::test]
async fn reopening_with_the_same_migrations_applies_nothing_twice() {
    let path = scratch_dir("migration-idempotent").join("state.db");
    drop(Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap());

    let reopened = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();

    insert_task(&reopened, "still one table").await;
    assert_eq!(task_titles(&reopened).await, ["still one table"]);
}

#[tokio::test]
async fn a_schema_newer_than_the_binary_is_refused() {
    let path = scratch_dir("migration-newer").join("state.db");
    drop(Database::open(&path, &[TASK_MIGRATIONS_V2]).await.unwrap());

    let older_binary = Database::open(&path, &[TASK_MIGRATIONS]).await;

    assert!(
        matches!(
            &older_binary,
            Err(Error::SchemaNewerThanBinary { owner, applied_version: 2, known_version: 1 })
                if owner == "tasks"
        ),
        "{older_binary:?}"
    );
}

#[tokio::test]
async fn a_schema_from_an_unknown_capability_is_refused() {
    let path = scratch_dir("migration-unknown-owner").join("state.db");
    drop(Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap());

    let without_tasks = Database::open(&path, &[]).await;

    assert!(
        matches!(
            &without_tasks,
            Err(Error::SchemaNewerThanBinary { owner, applied_version: 1, known_version: 0 })
                if owner == "tasks"
        ),
        "{without_tasks:?}"
    );
}

#[tokio::test]
async fn an_applied_migration_whose_sql_changed_is_refused() {
    let path = scratch_dir("migration-edited").join("state.db");
    drop(Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap());
    let edited = MigrationSet {
        owner: "tasks",
        migrations: &[Migration {
            version: 1,
            sql: "CREATE TABLE tasks (id INTEGER PRIMARY KEY, name TEXT)",
        }],
    };

    let reopened = Database::open(&path, &[edited]).await;

    assert!(
        matches!(&reopened, Err(Error::MigrationEdited { owner, version: 1 }) if owner == "tasks"),
        "{reopened:?}"
    );
}

#[tokio::test]
async fn a_failing_migration_leaves_no_partial_schema() {
    let path = scratch_dir("migration-failing").join("state.db");
    let broken = MigrationSet {
        owner: "tasks",
        migrations: &[Migration {
            version: 1,
            sql: "CREATE TABLE tasks (id INTEGER PRIMARY KEY, title TEXT);\
                  CREATE TABLE broken (",
        }],
    };
    assert!(Database::open(&path, &[broken]).await.is_err());

    let fixed = Database::open(&path, &[TASK_MIGRATIONS]).await.unwrap();

    insert_task(&fixed, "created by the fixed migration").await;
    assert_eq!(
        task_titles(&fixed).await,
        ["created by the fixed migration"]
    );
}

#[tokio::test]
async fn later_sets_see_the_tables_of_earlier_sets() {
    let path = scratch_dir("migration-order").join("state.db");
    let notes_referencing_tasks = MigrationSet {
        owner: "notes",
        migrations: &[Migration {
            version: 1,
            sql: "CREATE VIEW task_titles AS SELECT title FROM tasks",
        }],
    };

    let opened = Database::open(&path, &[TASK_MIGRATIONS, notes_referencing_tasks]).await;

    assert!(opened.is_ok(), "{opened:?}");
}

#[test]
fn migration_versions_must_count_up_from_one() {
    let skipping = MigrationSet {
        owner: "tasks",
        migrations: &[Migration {
            version: 2,
            sql: "SELECT 1",
        }],
    };

    let validation = cockpit_turso::validate_migrations(&[skipping]);

    assert!(
        matches!(&validation, Err(Error::InvalidMigrations { owner, .. }) if owner == "tasks"),
        "{validation:?}"
    );
}

#[test]
fn an_owner_may_appear_only_once() {
    let validation = cockpit_turso::validate_migrations(&[TASK_MIGRATIONS, TASK_MIGRATIONS]);

    assert!(
        matches!(&validation, Err(Error::InvalidMigrations { owner, .. }) if owner == "tasks"),
        "{validation:?}"
    );
}
