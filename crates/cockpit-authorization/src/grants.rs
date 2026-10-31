use cockpit_turso::sql::{Connection, Row, Value};
use cockpit_turso::{Migration, MigrationSet, WriteTransaction};

use crate::{
    Actor, Basis, Decision, Error, Grant, GrantId, Outcome, Permission, Refusal, Revision, Scope,
    SessionId, TaskId,
};

/// The authorization tables. Revoked grants stay, with the revision that revoked them, so a
/// decision's revision can be traced to the grants it read. The unique index keeps one active
/// copy of a grant whoever writes the table: a second copy would survive revoking the first.
/// Ids are never empty, so `coalesce(scope_id, '')` cannot make an owner scope collide.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "authorization",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE authorization_revision (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                revision INTEGER NOT NULL
            );
            INSERT INTO authorization_revision (singleton, revision) VALUES (1, 0);
            CREATE TABLE authorization_grants (
                id INTEGER PRIMARY KEY,
                holder TEXT NOT NULL CHECK (holder <> ''),
                permission TEXT NOT NULL CHECK (permission <> 'host-actions'),
                scope_kind TEXT NOT NULL CHECK (scope_kind IN ('owner', 'session', 'task')),
                scope_id TEXT CHECK (scope_id <> '')
                    CHECK ((scope_kind = 'owner') = (scope_id IS NULL)),
                granted_at INTEGER NOT NULL,
                revoked_at INTEGER
            );
            CREATE UNIQUE INDEX authorization_grants_one_active
                ON authorization_grants (holder, permission, scope_kind, coalesce(scope_id, ''))
                WHERE revoked_at IS NULL;",
    }],
};

const OWNER_SCOPE: &str = "owner";
const SESSION_SCOPE: &str = "session";
const TASK_SCOPE: &str = "task";

/// Gives `holder` `permission` over `scope`. Granting an active grant again returns it
/// unchanged, without a new revision. On error, roll the transaction back.
pub async fn grant(
    write: &WriteTransaction<'_>,
    holder: &SessionId,
    permission: Permission,
    scope: &Scope,
) -> Result<Grant, Error> {
    if permission.is_owner_only() {
        return Err(Error::OwnerOnly { permission });
    }
    let (scope_kind, scope_id) = scope_columns(scope);
    let mut existing = write
        .query(
            "SELECT id, holder, permission, scope_kind, scope_id, granted_at
             FROM authorization_grants
             WHERE holder = ?1 AND permission = ?2 AND scope_kind = ?3 AND scope_id IS ?4
               AND revoked_at IS NULL",
            (
                holder.as_str(),
                permission.as_str(),
                scope_kind,
                scope_id.clone(),
            ),
        )
        .await?;
    if let Some(row) = existing.next().await? {
        return grant_from_row(&row);
    }
    let granted_at = advance_revision(write).await?;
    write
        .execute(
            "INSERT INTO authorization_grants
                (holder, permission, scope_kind, scope_id, granted_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                holder.as_str(),
                permission.as_str(),
                scope_kind,
                scope_id,
                granted_at.0,
            ),
        )
        .await?;
    Ok(Grant {
        id: GrantId(write.last_insert_rowid()),
        holder: holder.clone(),
        permission,
        scope: scope.clone(),
        granted_at,
    })
}

/// Revokes an active grant and returns the revision that revoked it. Every evaluation that
/// reads the committed revocation refuses what the grant allowed. On error, roll the
/// transaction back.
pub async fn revoke(write: &WriteTransaction<'_>, grant: GrantId) -> Result<Revision, Error> {
    let revoked_at = advance_revision(write).await?;
    let revoked = write
        .execute(
            "UPDATE authorization_grants SET revoked_at = ?2
             WHERE id = ?1 AND revoked_at IS NULL",
            (grant.0, revoked_at.0),
        )
        .await?;
    if revoked == 0 {
        return Err(Error::GrantNotActive { grant });
    }
    Ok(revoked_at)
}

/// Answers whether `actor` may use `permission` over `scope`. Evaluate in the transaction the
/// action runs in, so the action cannot outlive a revocation it did not see. Nothing is
/// cached: a revocation applies from the next evaluation that reads it.
pub async fn evaluate(
    connection: &Connection,
    actor: &Actor,
    permission: Permission,
    scope: &Scope,
) -> Result<Decision, Error> {
    let holder = match actor {
        Actor::Owner => {
            return Ok(Decision {
                revision: current_revision(connection).await?,
                outcome: Outcome::Allowed(Basis::Owner),
            });
        }
        Actor::Session(_) if permission.is_owner_only() => {
            return Ok(Decision {
                revision: current_revision(connection).await?,
                outcome: Outcome::Refused(Refusal::OwnerOnly { permission }),
            });
        }
        Actor::Session(holder) => holder,
    };
    let (revision, covering_grant) = covering_grant(connection, holder, permission, scope).await?;
    let outcome = match covering_grant {
        Some(grant) => Outcome::Allowed(Basis::Grant(grant)),
        None => Outcome::Refused(Refusal::NoGrant {
            permission,
            scope: scope.clone(),
        }),
    };
    Ok(Decision { revision, outcome })
}

/// The grants `holder` holds now, oldest first.
pub async fn active_grants(
    connection: &Connection,
    holder: &SessionId,
) -> Result<Vec<Grant>, Error> {
    let mut rows = connection
        .query(
            "SELECT id, holder, permission, scope_kind, scope_id, granted_at
             FROM authorization_grants
             WHERE holder = ?1 AND revoked_at IS NULL
             ORDER BY id",
            [holder.as_str()],
        )
        .await?;
    let mut grants = Vec::new();
    while let Some(row) = rows.next().await? {
        grants.push(grant_from_row(&row)?);
    }
    Ok(grants)
}

/// The authorization state's current revision.
pub async fn current_revision(connection: &Connection) -> Result<Revision, Error> {
    let row = connection
        .prepare("SELECT revision FROM authorization_revision WHERE singleton = 1")
        .await?
        .query_row(())
        .await?;
    Ok(Revision(row.get(0)?))
}

async fn advance_revision(write: &WriteTransaction<'_>) -> Result<Revision, Error> {
    write
        .execute(
            "UPDATE authorization_revision SET revision = revision + 1 WHERE singleton = 1",
            (),
        )
        .await?;
    current_revision(write).await
}

/// The current revision and the oldest active grant to `holder` of `permission` whose scope
/// covers `scope`, read by one statement so both come from the same commit whatever
/// connection the caller passes. Ids match with `=` only: no pattern, case or prefix match
/// can stretch a grant.
async fn covering_grant(
    connection: &Connection,
    holder: &SessionId,
    permission: Permission,
    scope: &Scope,
) -> Result<(Revision, Option<GrantId>), Error> {
    let (scope_kind, scope_id) = scope_columns(scope);
    let row = connection
        .prepare(
            "SELECT
                (SELECT revision FROM authorization_revision WHERE singleton = 1),
                (SELECT id FROM authorization_grants
                 WHERE holder = ?1 AND permission = ?2 AND revoked_at IS NULL
                   AND (scope_kind = 'owner' OR (scope_kind = ?3 AND scope_id = ?4))
                 ORDER BY id
                 LIMIT 1)",
        )
        .await?
        .query_row((holder.as_str(), permission.as_str(), scope_kind, scope_id))
        .await?;
    let revision = Revision(row.get(0)?);
    let grant = row.get::<Option<i64>>(1)?.map(GrantId);
    Ok((revision, grant))
}

fn scope_columns(scope: &Scope) -> (&'static str, Value) {
    match scope {
        Scope::Owner => (OWNER_SCOPE, Value::Null),
        Scope::Session(session) => (SESSION_SCOPE, Value::Text(session.as_str().to_string())),
        Scope::Task(task) => (TASK_SCOPE, Value::Text(task.as_str().to_string())),
    }
}

/// Reads a row of `id, holder, permission, scope_kind, scope_id, granted_at`, refusing values
/// this crate never writes instead of reading them as a narrower or wider grant.
fn grant_from_row(row: &Row) -> Result<Grant, Error> {
    let id = GrantId(row.get(0)?);
    let invalid = |reason: String| Error::InvalidStoredGrant { grant: id, reason };
    let holder =
        SessionId::new(row.get::<String>(1)?).map_err(|error| invalid(error.to_string()))?;
    let permission: Permission = row
        .get::<String>(2)?
        .parse()
        .map_err(|error: Error| invalid(error.to_string()))?;
    if permission.is_owner_only() {
        return Err(invalid(format!("{permission} is held by a session")));
    }
    let scope_kind: String = row.get(3)?;
    let scope_id: Option<String> = row.get(4)?;
    let scope = match (scope_kind.as_str(), scope_id) {
        (OWNER_SCOPE, None) => Scope::Owner,
        (SESSION_SCOPE, Some(id)) => {
            Scope::Session(SessionId::new(id).map_err(|error| invalid(error.to_string()))?)
        }
        (TASK_SCOPE, Some(id)) => {
            Scope::Task(TaskId::new(id).map_err(|error| invalid(error.to_string()))?)
        }
        (kind, id) => return Err(invalid(format!("scope {kind:?} with id {id:?}"))),
    };
    Ok(Grant {
        id,
        holder,
        permission,
        scope,
        granted_at: Revision(row.get(5)?),
    })
}
