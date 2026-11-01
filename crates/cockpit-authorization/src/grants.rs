use cockpit_turso::sql::{Connection, Row, Value};
use cockpit_turso::{Migration, MigrationSet, WriteTransaction};

use crate::{
    Actor, Audit, AuditEntry, AuditedAction, Basis, Decision, Error, Grant, GrantId, Outcome,
    Permission, Refusal, Revision, Scope, SessionId, StartAllowance, StartTerms, TaskId,
};

/// The window a `start-sessions` grant's cap counts over.
const HOUR_SECONDS: i64 = 60 * 60;
/// The columns `grant_from_row` reads, in its order.
const GRANT_COLUMNS: &str =
    "id, holder, permission, scope_kind, scope_id, granted_at, starts_per_hour, starts_unconfirmed";
/// The columns `audit_from_row` reads, in its order.
const AUDIT_COLUMNS: &str = "id, actor, permission, grant_id, revision, action, target, detail, at";

/// The authorization tables. Revoked grants stay, with the revision that revoked them, so a
/// decision's revision can be traced to the grants it read. The unique index keeps one active
/// copy of a grant whoever writes the table: a second copy would survive revoking the first.
/// Ids are never empty, so `coalesce(scope_id, '')` cannot make an owner scope collide.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "authorization",
    migrations: &[
        Migration {
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
        },
        // #410: a `start-sessions` grant's terms, and the audit of what coordinator grants
        // were used for. Audit rows are never updated or deleted.
        Migration {
            version: 2,
            sql: "ALTER TABLE authorization_grants ADD COLUMN starts_per_hour INTEGER
                CHECK (starts_per_hour >= 1);
            ALTER TABLE authorization_grants ADD COLUMN starts_unconfirmed INTEGER NOT NULL
                DEFAULT 0 CHECK (starts_unconfirmed IN (0, 1));
            CREATE TABLE authorization_audit (
                id INTEGER PRIMARY KEY,
                actor TEXT NOT NULL CHECK (actor <> ''),
                permission TEXT NOT NULL,
                grant_id INTEGER NOT NULL,
                revision INTEGER NOT NULL,
                action TEXT NOT NULL CHECK (action IN ('message', 'start', 'started')),
                target TEXT CHECK (target <> ''),
                detail TEXT,
                at INTEGER NOT NULL
            );
            CREATE INDEX authorization_audit_by_actor
                ON authorization_audit (actor, action, at);
            CREATE INDEX authorization_audit_by_target
                ON authorization_audit (target, action, id);",
        },
    ],
};

/// Gives `holder` `permission` over `scope`. Granting an active grant again returns it
/// unchanged, without a new revision. A `start-sessions` grant given here has
/// [`StartTerms::DEFAULT`]; [`grant_start_sessions`] gives other terms.
pub async fn grant(
    write: &WriteTransaction<'_>,
    holder: &SessionId,
    permission: Permission,
    scope: &Scope,
) -> Result<Grant, Error> {
    if permission.is_owner_only() {
        return Err(Error::OwnerOnly { permission });
    }
    if let Some(existing) = active_grant(write, holder, permission, scope).await? {
        return Ok(existing);
    }
    let start_terms = (permission == Permission::StartSessions).then_some(StartTerms::DEFAULT);
    insert_grant(write, holder, permission, scope, start_terms).await
}

/// Gives `holder` `start-sessions` over the owner's scope with `terms`. The same active grant
/// is returned unchanged; one with other terms is revoked and replaced, so the terms a start
/// was allowed under are always the grant's own.
pub async fn grant_start_sessions(
    write: &WriteTransaction<'_>,
    holder: &SessionId,
    terms: StartTerms,
) -> Result<Grant, Error> {
    if terms.per_hour == 0 {
        return Err(Error::InvalidStartTerms {
            reason: "the cap per hour must be at least 1",
        });
    }
    let permission = Permission::StartSessions;
    if let Some(existing) = active_grant(write, holder, permission, &Scope::Owner).await? {
        if existing.start_terms == Some(terms) {
            return Ok(existing);
        }
        revoke(write, existing.id).await?;
    }
    insert_grant(write, holder, permission, &Scope::Owner, Some(terms)).await
}

/// The active grant of exactly `permission` over exactly `scope` to `holder`, if any.
pub async fn active_grant(
    connection: &Connection,
    holder: &SessionId,
    permission: Permission,
    scope: &Scope,
) -> Result<Option<Grant>, Error> {
    let (scope_kind, scope_id) = scope_columns(scope);
    let mut existing = connection
        .query(
            &format!(
                "SELECT {GRANT_COLUMNS} FROM authorization_grants
                 WHERE holder = ?1 AND permission = ?2 AND scope_kind = ?3 AND scope_id IS ?4
                   AND revoked_at IS NULL"
            ),
            (holder.as_str(), permission.as_str(), scope_kind, scope_id),
        )
        .await?;
    match existing.next().await? {
        Some(row) => Ok(Some(grant_from_row(&row)?)),
        None => Ok(None),
    }
}

async fn insert_grant(
    write: &WriteTransaction<'_>,
    holder: &SessionId,
    permission: Permission,
    scope: &Scope,
    start_terms: Option<StartTerms>,
) -> Result<Grant, Error> {
    let (scope_kind, scope_id) = scope_columns(scope);
    let granted_at = advance_revision(write).await?;
    write
        .execute(
            "INSERT INTO authorization_grants
                (holder, permission, scope_kind, scope_id, granted_at, starts_per_hour,
                 starts_unconfirmed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                holder.as_str(),
                permission.as_str(),
                scope_kind,
                scope_id,
                granted_at.0,
                start_terms.map(|terms| i64::from(terms.per_hour)),
                start_terms.is_some_and(|terms| !terms.needs_confirmation),
            ),
        )
        .await?;
    Ok(Grant {
        id: GrantId(write.last_insert_rowid()),
        holder: holder.clone(),
        permission,
        scope: scope.clone(),
        granted_at,
        start_terms,
    })
}

/// Revokes an active grant and returns the revision that revoked it. Every evaluation that
/// reads the committed revocation refuses what the grant allowed. Refusing an inactive grant
/// changes nothing.
pub async fn revoke(write: &WriteTransaction<'_>, grant: GrantId) -> Result<Revision, Error> {
    let mut active = write
        .query(
            "SELECT 1 FROM authorization_grants WHERE id = ?1 AND revoked_at IS NULL",
            [grant.0],
        )
        .await?;
    if active.next().await?.is_none() {
        return Err(Error::GrantNotActive { grant });
    }
    let revoked_at = advance_revision(write).await?;
    write
        .execute(
            "UPDATE authorization_grants SET revoked_at = ?2 WHERE id = ?1",
            (grant.0, revoked_at.0),
        )
        .await?;
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
    let (revision, outcome) = match actor {
        Actor::Owner => (
            current_revision(connection).await?,
            Outcome::Allowed(Basis::Owner),
        ),
        Actor::Session(_) if permission.is_owner_only() => (
            current_revision(connection).await?,
            Outcome::Refused(Refusal::OwnerOnly { permission }),
        ),
        Actor::Session(holder) => {
            let (revision, covering_grant) =
                covering_grant(connection, holder, permission, scope).await?;
            let outcome = match covering_grant {
                Some(grant) => Outcome::Allowed(Basis::Grant(grant)),
                None => Outcome::Refused(Refusal::NoGrant {
                    permission,
                    scope: scope.clone(),
                }),
            };
            (revision, outcome)
        }
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
            &format!(
                "SELECT {GRANT_COLUMNS} FROM authorization_grants
                 WHERE holder = ?1 AND revoked_at IS NULL
                 ORDER BY id"
            ),
            [holder.as_str()],
        )
        .await?;
    let mut grants = Vec::new();
    while let Some(row) = rows.next().await? {
        grants.push(grant_from_row(&row)?);
    }
    Ok(grants)
}

/// Every session's active coordinator grants (#410), by holder then oldest first: what the
/// session cards show.
pub async fn active_coordinator_grants(connection: &Connection) -> Result<Vec<Grant>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {GRANT_COLUMNS} FROM authorization_grants
                 WHERE revoked_at IS NULL AND permission IN (?1, ?2, ?3)
                 ORDER BY holder, id"
            ),
            (
                Permission::ObserveSessions.as_str(),
                Permission::MessageSessions.as_str(),
                Permission::StartSessions.as_str(),
            ),
        )
        .await?;
    let mut grants = Vec::new();
    while let Some(row) = rows.next().await? {
        grants.push(grant_from_row(&row)?);
    }
    Ok(grants)
}

/// Whether `holder` may start a session at `now` (seconds since 1970): its `start-sessions`
/// grant over the owner's scope, and fewer [`AuditedAction::Start`] entries in the past hour
/// than the grant's cap. Evaluate it in the write transaction that records the start, so two
/// starts cannot both take the last place.
pub async fn start_allowance(
    connection: &Connection,
    holder: &SessionId,
    now: i64,
) -> Result<StartAllowance, Error> {
    let actor = Actor::Session(holder.clone());
    let decision = evaluate(connection, &actor, Permission::StartSessions, &Scope::Owner).await?;
    let grant = match decision.outcome {
        Outcome::Allowed(Basis::Grant(grant)) => grant,
        Outcome::Allowed(Basis::Owner) => unreachable!("a session is never allowed as the owner"),
        Outcome::Refused(refusal) => return Ok(StartAllowance::Refused(refusal)),
    };
    let terms = grant_by_id(connection, grant)
        .await?
        .start_terms
        .ok_or_else(|| Error::InvalidStoredGrant {
            grant,
            reason: "a start-sessions grant without terms".to_owned(),
        })?;
    let window_start = now - HOUR_SECONDS;
    let mut rows = connection
        .query(
            "SELECT at FROM authorization_audit
             WHERE actor = ?1 AND action = 'start' AND at > ?2
             ORDER BY at DESC
             LIMIT 1 OFFSET ?3",
            (holder.as_str(), window_start, i64::from(terms.per_hour) - 1),
        )
        .await?;
    if let Some(row) = rows.next().await? {
        let oldest_counted: i64 = row.get(0)?;
        return Ok(StartAllowance::Capped {
            per_hour: terms.per_hour,
            retry_at: oldest_counted + HOUR_SECONDS,
        });
    }
    Ok(StartAllowance::Allowed {
        grant,
        revision: decision.revision,
        needs_confirmation: terms.needs_confirmation,
    })
}

/// The grant stored as `id`, active or revoked, when another crate kept only its number:
/// the grant a request was asked under (#285, #410).
pub async fn stored_grant(connection: &Connection, id: i64) -> Result<Option<Grant>, Error> {
    match grant_by_id(connection, GrantId(id)).await {
        Ok(grant) => Ok(Some(grant)),
        Err(Error::GrantNotActive { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn grant_by_id(connection: &Connection, grant: GrantId) -> Result<Grant, Error> {
    let mut rows = connection
        .query(
            &format!("SELECT {GRANT_COLUMNS} FROM authorization_grants WHERE id = ?1"),
            [grant.0],
        )
        .await?;
    match rows.next().await? {
        Some(row) => grant_from_row(&row),
        None => Err(Error::GrantNotActive { grant }),
    }
}

/// Records one use of a coordinator grant, in the caller's transaction, so the audit commits
/// with the effect or not at all. Returns the entry's id.
pub async fn record_audit(write: &WriteTransaction<'_>, audit: &Audit) -> Result<i64, Error> {
    if !audit.permission.is_coordinator() {
        return Err(Error::NotAudited {
            permission: audit.permission,
        });
    }
    write
        .execute(
            "INSERT INTO authorization_audit
                (actor, permission, grant_id, revision, action, target, detail, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            (
                audit.actor.as_str(),
                audit.permission.as_str(),
                audit.grant.0,
                audit.revision.0,
                audit.action.as_str(),
                audit
                    .target
                    .as_ref()
                    .map(|target| target.as_str().to_owned()),
                audit.detail.clone(),
                audit.at,
            ),
        )
        .await?;
    Ok(write.last_insert_rowid())
}

/// The audit entries `session` made or was the target of, newest first, at most `limit`.
pub async fn audit_of(
    connection: &Connection,
    session: &SessionId,
    limit: u32,
) -> Result<Vec<AuditEntry>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {AUDIT_COLUMNS} FROM authorization_audit
                 WHERE actor = ?1 OR target = ?1
                 ORDER BY id DESC
                 LIMIT ?2"
            ),
            (session.as_str(), i64::from(limit)),
        )
        .await?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next().await? {
        entries.push(audit_from_row(&row)?);
    }
    Ok(entries)
}

/// The newest entry of `action` whose target is `target`, if any.
pub async fn latest_audit_on(
    connection: &Connection,
    target: &SessionId,
    action: AuditedAction,
) -> Result<Option<AuditEntry>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {AUDIT_COLUMNS} FROM authorization_audit
                 WHERE target = ?1 AND action = ?2
                 ORDER BY id DESC
                 LIMIT 1"
            ),
            (target.as_str(), action.as_str()),
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(audit_from_row(&row)?)),
        None => Ok(None),
    }
}

/// For each session some entry of `action` targets, the newest such entry.
pub async fn latest_audits_by_target(
    connection: &Connection,
    action: AuditedAction,
) -> Result<Vec<AuditEntry>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {AUDIT_COLUMNS} FROM authorization_audit
                 WHERE id IN (SELECT MAX(id) FROM authorization_audit
                              WHERE action = ?1 AND target IS NOT NULL
                              GROUP BY target)
                 ORDER BY target"
            ),
            [action.as_str()],
        )
        .await?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next().await? {
        entries.push(audit_from_row(&row)?);
    }
    Ok(entries)
}

/// Reads a row of [`AUDIT_COLUMNS`], refusing values this crate never writes.
fn audit_from_row(row: &Row) -> Result<AuditEntry, Error> {
    let id: i64 = row.get(0)?;
    let invalid = |reason: String| Error::InvalidStoredAudit { entry: id, reason };
    let actor =
        SessionId::new(row.get::<String>(1)?).map_err(|error| invalid(error.to_string()))?;
    let permission: Permission = row
        .get::<String>(2)?
        .parse()
        .map_err(|error: Error| invalid(error.to_string()))?;
    let action_name: String = row.get(5)?;
    let action = AuditedAction::parse(&action_name)
        .ok_or_else(|| invalid(format!("unknown action {action_name:?}")))?;
    let target = row
        .get::<Option<String>>(6)?
        .map(SessionId::new)
        .transpose()
        .map_err(|error| invalid(error.to_string()))?;
    Ok(AuditEntry {
        id,
        audit: Audit {
            actor,
            permission,
            grant: GrantId(row.get(3)?),
            revision: Revision(row.get(4)?),
            action,
            target,
            detail: row.get(7)?,
            at: row.get(8)?,
        },
    })
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
    let row = write
        .prepare(
            "UPDATE authorization_revision SET revision = revision + 1 WHERE singleton = 1
             RETURNING revision",
        )
        .await?
        .query_row(())
        .await?;
    Ok(Revision(row.get(0)?))
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
        Scope::Owner => ("owner", Value::Null),
        Scope::Session(session) => ("session", Value::Text(session.as_str().to_string())),
        Scope::Task(task) => ("task", Value::Text(task.as_str().to_string())),
    }
}

/// Reads a row of [`GRANT_COLUMNS`], refusing values this crate never writes instead of
/// reading them as a narrower or wider grant.
fn grant_from_row(row: &Row) -> Result<Grant, Error> {
    let id = GrantId(row.get(0)?);
    let invalid = |reason: &dyn std::fmt::Display| Error::InvalidStoredGrant {
        grant: id,
        reason: reason.to_string(),
    };
    let holder = SessionId::new(row.get::<String>(1)?).map_err(|error| invalid(&error))?;
    let permission: Permission = row
        .get::<String>(2)?
        .parse()
        .map_err(|error: Error| invalid(&error))?;
    if permission.is_owner_only() {
        return Err(invalid(&format_args!("{permission} is held by a session")));
    }
    let scope_kind: String = row.get(3)?;
    let scope_id: Option<String> = row.get(4)?;
    let scope = match (scope_kind.as_str(), scope_id) {
        ("owner", None) => Scope::Owner,
        ("session", Some(id)) => {
            Scope::Session(SessionId::new(id).map_err(|error| invalid(&error))?)
        }
        ("task", Some(id)) => Scope::Task(TaskId::new(id).map_err(|error| invalid(&error))?),
        (kind, id) => return Err(invalid(&format_args!("scope {kind:?} with id {id:?}"))),
    };
    let per_hour: Option<i64> = row.get(6)?;
    let is_unconfirmed: bool = row.get::<i64>(7)? == 1;
    let start_terms = match (permission, per_hour) {
        (Permission::StartSessions, Some(per_hour)) => Some(StartTerms {
            per_hour: u32::try_from(per_hour)
                .ok()
                .filter(|per_hour| *per_hour >= 1)
                .ok_or_else(|| invalid(&format_args!("a cap of {per_hour} starts an hour")))?,
            needs_confirmation: !is_unconfirmed,
        }),
        (Permission::StartSessions, None) => {
            return Err(invalid(&"a start-sessions grant without terms"));
        }
        (_, None) if !is_unconfirmed => None,
        (other, _) => return Err(invalid(&format_args!("{other} with start terms"))),
    };
    Ok(Grant {
        id,
        holder,
        permission,
        scope,
        granted_at: Revision(row.get(5)?),
        start_terms,
    })
}
