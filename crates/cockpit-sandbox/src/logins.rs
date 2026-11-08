//! Login grants (#59, #858): the owner lets one project's sessions use a login account, the
//! sign-in `torlek account sign-in` keeps. Like a network grant (#674), each records when and
//! through which owner client it was granted and revoked, and applies from the project's next
//! launch; only the owner decides, so who is the owner.
//!
//! The caller owns the transaction and authorization; these functions keep the rules of the
//! rows: one active grant per project, provider and account, rows never deleted.

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row, Value};

use crate::network::{OwnerClient, OwnerDecision};

/// The table, created by the `sandbox` migration set's version 3.
pub(crate) const MIGRATION_SQL: &str = "CREATE TABLE sandbox_login_grants (
        id INTEGER PRIMARY KEY,
        project TEXT NOT NULL CHECK (project <> ''),
        provider TEXT NOT NULL CHECK (provider <> ''),
        account TEXT NOT NULL CHECK (account <> ''),
        granted_at INTEGER NOT NULL,
        granted_through TEXT CHECK (granted_through IN ('tui', 'cli')),
        revoked_at INTEGER,
        revoked_through TEXT CHECK (revoked_through IN ('tui', 'cli')),
        CHECK (revoked_at IS NOT NULL OR revoked_through IS NULL)
    );
    CREATE UNIQUE INDEX sandbox_login_grants_one_active
        ON sandbox_login_grants (project, provider, account) WHERE revoked_at IS NULL;";

/// One project's use of a provider's login account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginGrant {
    pub id: i64,
    pub project: String,
    pub provider: String,
    pub account: String,
    pub granted: OwnerDecision,
    pub revoked: Option<OwnerDecision>,
}

/// Why a grant was not changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginGrantRefusal {
    NotFound {
        id: i64,
    },
    /// The project holds this account through `grant` already.
    AlreadyGranted {
        grant: i64,
    },
    AlreadyRevoked,
}

impl std::fmt::Display for LoginGrantRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginGrantRefusal::NotFound { id } => write!(formatter, "no login grant {id}"),
            LoginGrantRefusal::AlreadyGranted { grant } => {
                write!(
                    formatter,
                    "the project holds that login through grant {grant}"
                )
            }
            LoginGrantRefusal::AlreadyRevoked => {
                formatter.write_str("the login grant was revoked already")
            }
        }
    }
}

#[derive(Debug)]
pub enum LoginGrantError {
    Refused(LoginGrantRefusal),
    InvalidRow(String),
    Sql(cockpit_turso::sql::Error),
}

impl std::fmt::Display for LoginGrantError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginGrantError::Refused(refusal) => refusal.fmt(formatter),
            LoginGrantError::InvalidRow(reason) => {
                write!(formatter, "a stored login grant is invalid: {reason}")
            }
            LoginGrantError::Sql(error) => write!(formatter, "login grants database: {error}"),
        }
    }
}

impl std::error::Error for LoginGrantError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoginGrantError::Sql(error) => Some(error),
            _ => None,
        }
    }
}

impl From<cockpit_turso::sql::Error> for LoginGrantError {
    fn from(error: cockpit_turso::sql::Error) -> Self {
        LoginGrantError::Sql(error)
    }
}

impl From<LoginGrantRefusal> for LoginGrantError {
    fn from(refusal: LoginGrantRefusal) -> Self {
        LoginGrantError::Refused(refusal)
    }
}

const COLUMNS: &str = "id, project, provider, account, granted_at, granted_through, revoked_at,
    revoked_through";

/// The owner grants `project` the use of `account` of `provider`; refused while the project
/// holds it already. The caller checks the names.
pub async fn grant_login(
    write: &WriteTransaction<'_>,
    project: &str,
    provider: &str,
    account: &str,
    through: Option<OwnerClient>,
    now: i64,
) -> Result<LoginGrant, LoginGrantError> {
    let mut rows = write
        .query(
            "SELECT id FROM sandbox_login_grants
             WHERE project = ?1 AND provider = ?2 AND account = ?3 AND revoked_at IS NULL",
            (project, provider, account),
        )
        .await?;
    if let Some(row) = rows.next().await? {
        return Err(LoginGrantRefusal::AlreadyGranted { grant: row.get(0)? }.into());
    }
    drop(rows);
    let row = write
        .prepare(&format!(
            "INSERT INTO sandbox_login_grants
                (project, provider, account, granted_at, granted_through)
             VALUES (?1, ?2, ?3, ?4, ?5) RETURNING {COLUMNS}"
        ))
        .await?
        .query_row((
            project,
            provider,
            account,
            now,
            through.map(OwnerClient::as_str),
        ))
        .await?;
    grant_from(&row)
}

/// The active grants, of `project` or of every project, oldest first.
pub async fn login_grants(
    connection: &Connection,
    project: Option<&str>,
) -> Result<Vec<LoginGrant>, LoginGrantError> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {COLUMNS} FROM sandbox_login_grants
                 WHERE revoked_at IS NULL AND (?1 IS NULL OR project = ?1) ORDER BY id"
            ),
            [project.map_or(Value::Null, |name| Value::Text(name.to_owned()))],
        )
        .await?;
    let mut grants = Vec::new();
    while let Some(row) = rows.next().await? {
        grants.push(grant_from(&row)?);
    }
    Ok(grants)
}

/// The owner revokes grant `id`; the row stays as the record of who allowed what.
pub async fn revoke_login_grant(
    write: &WriteTransaction<'_>,
    id: i64,
    through: Option<OwnerClient>,
    now: i64,
) -> Result<LoginGrant, LoginGrantError> {
    let mut rows = write
        .query(
            &format!("SELECT {COLUMNS} FROM sandbox_login_grants WHERE id = ?1"),
            [id],
        )
        .await?;
    let grant = match rows.next().await? {
        Some(row) => grant_from(&row)?,
        None => return Err(LoginGrantRefusal::NotFound { id }.into()),
    };
    drop(rows);
    if grant.revoked.is_some() {
        return Err(LoginGrantRefusal::AlreadyRevoked.into());
    }
    let row = write
        .prepare(&format!(
            "UPDATE sandbox_login_grants SET revoked_at = ?2, revoked_through = ?3
             WHERE id = ?1 RETURNING {COLUMNS}"
        ))
        .await?
        .query_row((id, now, through.map(OwnerClient::as_str)))
        .await?;
    grant_from(&row)
}

fn grant_from(row: &Row) -> Result<LoginGrant, LoginGrantError> {
    Ok(LoginGrant {
        id: row.get(0)?,
        project: row.get(1)?,
        provider: row.get(2)?,
        account: row.get(3)?,
        granted: decision_from(row, 4)?
            .ok_or_else(|| LoginGrantError::InvalidRow("a grant without its time".to_owned()))?,
        revoked: decision_from(row, 6)?,
    })
}

/// The decision stored in `at, through` from `first`; `None` while `at` is null.
fn decision_from(row: &Row, first: usize) -> Result<Option<OwnerDecision>, LoginGrantError> {
    let Some(at) = row.get::<Option<i64>>(first)? else {
        return Ok(None);
    };
    let through = match row.get::<Option<String>>(first + 1)? {
        Some(text) => Some(
            OwnerClient::parse(&text)
                .ok_or_else(|| LoginGrantError::InvalidRow(format!("decided through {text:?}")))?,
        ),
        None => None,
    };
    Ok(Some(OwnerDecision { at, through }))
}
