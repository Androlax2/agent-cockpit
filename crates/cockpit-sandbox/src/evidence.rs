//! What one launch got, stored in the launch's own transaction so a launch the database
//! names always has its evidence: enforced or unsandboxed, the engine and platform, each rule
//! with what enforced it, and the host capabilities observed when the engine was checked.

use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_turso::sql::Connection;
use cockpit_turso::{Migration, MigrationSet, WriteTransaction};

/// Evidence rows are kept with their launch; nothing deletes them, so `session show` on an
/// ended session still says how its last agent ran.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "sandbox",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE sandbox_launches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                execution_id INTEGER NOT NULL UNIQUE,
                session_id TEXT NOT NULL CHECK (session_id <> ''),
                enforcement TEXT NOT NULL CHECK (enforcement IN ('sandboxed', 'unsandboxed')),
                backend TEXT CHECK (backend <> ''),
                engine TEXT CHECK (engine <> ''),
                client_identity TEXT CHECK (client_identity <> ''),
                reason TEXT CHECK (reason <> ''),
                platform TEXT NOT NULL CHECK (platform <> ''),
                recorded_at INTEGER NOT NULL,
                CHECK ((enforcement = 'sandboxed') = (backend IS NOT NULL AND engine IS NOT NULL
                    AND client_identity IS NOT NULL AND reason IS NULL)),
                CHECK ((enforcement = 'unsandboxed') = (reason IS NOT NULL))
            );
            CREATE INDEX sandbox_launches_by_session ON sandbox_launches (session_id, id);
            CREATE TABLE sandbox_launch_lines (
                launch INTEGER NOT NULL REFERENCES sandbox_launches (id),
                position INTEGER NOT NULL CHECK (position >= 0),
                kind TEXT NOT NULL CHECK (kind IN ('rule', 'observed')),
                text TEXT NOT NULL CHECK (text <> ''),
                enforced_by TEXT CHECK (enforced_by <> ''),
                CHECK ((kind = 'rule') = (enforced_by IS NOT NULL)),
                PRIMARY KEY (launch, position)
            );",
    }],
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enforcement {
    Sandboxed {
        backend: String,
        engine: String,
        client_identity: String,
    },
    /// Only ever by the owner's explicit setting, which `reason` names.
    Unsandboxed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedRule {
    pub rule: String,
    pub enforced_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub enforcement: Enforcement,
    pub platform: String,
    pub rules: Vec<AppliedRule>,
    /// The host capabilities the engine reported when it was checked.
    pub observed: Vec<String>,
}

/// Records `evidence` for the launch of `execution` in `session`, in the caller's
/// transaction (the one that records the launch).
pub async fn record(
    write: &WriteTransaction<'_>,
    execution: i64,
    session: &str,
    evidence: &Evidence,
) -> Result<(), cockpit_turso::Error> {
    let (enforcement, backend, engine, client_identity, reason) = match &evidence.enforcement {
        Enforcement::Sandboxed {
            backend,
            engine,
            client_identity,
        } => (
            "sandboxed",
            Some(backend.as_str()),
            Some(engine.as_str()),
            Some(client_identity.as_str()),
            None,
        ),
        Enforcement::Unsandboxed { reason } => {
            ("unsandboxed", None, None, None, Some(reason.as_str()))
        }
    };
    write
        .execute(
            "INSERT INTO sandbox_launches
                (execution_id, session_id, enforcement, backend, engine, client_identity,
                 reason, platform, recorded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (
                execution,
                session,
                enforcement,
                backend,
                engine,
                client_identity,
                reason,
                evidence.platform.as_str(),
                unix_now(),
            ),
        )
        .await?;
    let launch = write.last_insert_rowid();
    let rules = evidence
        .rules
        .iter()
        .map(|rule| ("rule", rule.rule.as_str(), Some(rule.enforced_by.as_str())));
    let observed = evidence
        .observed
        .iter()
        .map(|observation| ("observed", observation.as_str(), None));
    for (position, (kind, text, enforced_by)) in rules.chain(observed).enumerate() {
        write
            .execute(
                "INSERT INTO sandbox_launch_lines (launch, position, kind, text, enforced_by)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                (launch, position as i64, kind, text, enforced_by),
            )
            .await?;
    }
    Ok(())
}

/// The evidence of `session`'s latest recorded launch, if it has one.
pub async fn latest_for_session(
    connection: &Connection,
    session: &str,
) -> Result<Option<Evidence>, cockpit_turso::Error> {
    let mut rows = connection
        .query(
            "SELECT id, enforcement, backend, engine, client_identity, reason, platform
             FROM sandbox_launches WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
            [session],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let launch: i64 = row.get(0)?;
    let enforcement: String = row.get(1)?;
    let enforcement = if enforcement == "sandboxed" {
        Enforcement::Sandboxed {
            backend: row.get(2)?,
            engine: row.get(3)?,
            client_identity: row.get(4)?,
        }
    } else {
        Enforcement::Unsandboxed {
            reason: row.get(5)?,
        }
    };
    let platform: String = row.get(6)?;
    drop(rows);
    let mut lines = connection
        .query(
            "SELECT kind, text, enforced_by FROM sandbox_launch_lines
             WHERE launch = ?1 ORDER BY position",
            [launch],
        )
        .await?;
    let mut rules = Vec::new();
    let mut observed = Vec::new();
    while let Some(line) = lines.next().await? {
        let kind: String = line.get(0)?;
        let text: String = line.get(1)?;
        if kind == "rule" {
            rules.push(AppliedRule {
                rule: text,
                enforced_by: line.get(2)?,
            });
        } else {
            observed.push(text);
        }
    }
    Ok(Some(Evidence {
        enforcement,
        platform,
        rules,
        observed,
    }))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}
