//! The sessions tables and every query on them: current state, launches and the transition
//! history (report 04: current state + transition history, no event sourcing).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cockpit_config::{FileState, RecordedFile, StartFileSnapshot};

use cockpit_processes::ExecutionId;
use cockpit_projects::CheckoutId;
use cockpit_provider_api::{ConversationId, ProviderName};
use cockpit_turso::sql::{Connection, Row, Value};
use cockpit_turso::{Migration, MigrationSet, WriteTransaction};

use crate::model::LaunchPurpose;
use crate::{Error, Launch, LaunchKind, Session, SessionId, SessionState};

/// A session row is never deleted, so an id is never reused and an old reference reads as
/// ended. `stopping_to` is where a stop under way leads.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "sessions",
    migrations: &[
        Migration {
            version: 1,
            sql: "CREATE TABLE sessions (
                id TEXT PRIMARY KEY CHECK (id <> ''),
                provider TEXT NOT NULL CHECK (provider <> ''),
                checkout_id INTEGER NOT NULL,
                holds_checkout INTEGER NOT NULL CHECK (holds_checkout IN (0, 1)),
                working_dir TEXT NOT NULL CHECK (working_dir <> ''),
                conversation TEXT CHECK (conversation <> ''),
                design_link TEXT CHECK (design_link <> ''),
                state TEXT NOT NULL CHECK (state IN
                    ('launching', 'running', 'stopping', 'asleep', 'ended', 'failed')),
                stopping_to TEXT CHECK (stopping_to IN ('asleep', 'ended', 'restart')),
                reason TEXT,
                revision INTEGER NOT NULL CHECK (revision >= 1),
                created_at INTEGER NOT NULL,
                changed_at INTEGER NOT NULL,
                CHECK ((state = 'stopping') = (stopping_to IS NOT NULL))
            );
            CREATE TABLE session_launches (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                kind TEXT NOT NULL
                    CHECK (kind IN ('start', 'restart', 'wake', 'resume', 'restore')),
                execution_id INTEGER NOT NULL,
                launched_at INTEGER NOT NULL
            );
            CREATE INDEX session_launches_by_session ON session_launches (session_id, id);
            CREATE TABLE session_transitions (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                from_state TEXT,
                to_state TEXT NOT NULL,
                reason TEXT,
                at INTEGER NOT NULL
            );",
        },
        // #146: agents' reports move here from the agent channel, rows kept, with the
        // launch they were sent from (unknown for the copied rows); each launch
        // records the start files its agent read; a restart may wait for the agent's turn.
        // The copy reads the channel's version 1 table, so the agent channel's migrations
        // run first (the daemon's order).
        Migration {
            version: 2,
            sql: "CREATE TABLE session_reports (
                session TEXT PRIMARY KEY CHECK (session <> ''),
                state TEXT CHECK (state IN ('working', 'waiting')),
                label TEXT,
                doing TEXT,
                needs_input TEXT,
                reported_at INTEGER NOT NULL,
                launch_id INTEGER
            );
            INSERT INTO session_reports (session, state, label, doing, needs_input, reported_at)
                SELECT session, state, label, doing, needs_input, reported_at
                FROM agent_channel_reports;
            DROP TABLE agent_channel_reports;
            CREATE TABLE session_start_files (
                launch_id INTEGER NOT NULL,
                label TEXT NOT NULL CHECK (label <> ''),
                path TEXT NOT NULL CHECK (path <> ''),
                file_state TEXT NOT NULL CHECK (file_state IN ('missing', 'present', 'unreadable')),
                modified_ns INTEGER,
                changed_s INTEGER,
                changed_ns INTEGER,
                inode INTEGER,
                len INTEGER,
                error TEXT,
                PRIMARY KEY (launch_id, label),
                CHECK ((file_state = 'present') = (modified_ns IS NOT NULL)),
                CHECK ((modified_ns IS NULL) = (changed_s IS NULL)
                    AND (changed_s IS NULL) = (changed_ns IS NULL)
                    AND (changed_ns IS NULL) = (inode IS NULL)
                    AND (inode IS NULL) = (len IS NULL)),
                CHECK ((file_state = 'unreadable') = (error IS NOT NULL))
            );
            CREATE TABLE session_restart_requests (
                session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
                requested_at INTEGER NOT NULL
            );",
        },
        // #183: how the agent's latest turn ended; only a failure carries a reason.
        Migration {
            version: 3,
            sql: "ALTER TABLE session_reports ADD COLUMN outcome TEXT
                CHECK (outcome IN ('completed', 'failed', 'quota-stopped'));
            ALTER TABLE session_reports ADD COLUMN outcome_reason TEXT
                CHECK (outcome_reason IS NULL OR outcome = 'failed');",
        },
        // #182 (decided in #200): an open question records where it came from. A question
        // recorded before is taken as the agent's own, the source that stays visible.
        Migration {
            version: 4,
            sql: "CREATE TABLE session_reports_with_sources (
                session TEXT PRIMARY KEY CHECK (session <> ''),
                state TEXT CHECK (state IN ('working', 'waiting')),
                label TEXT,
                doing TEXT,
                needs_input TEXT,
                reported_at INTEGER NOT NULL,
                launch_id INTEGER,
                outcome TEXT CHECK (outcome IN ('completed', 'failed', 'quota-stopped')),
                outcome_reason TEXT CHECK (outcome_reason IS NULL OR outcome = 'failed'),
                question_source TEXT CHECK (question_source IN ('agent', 'dialog')),
                CHECK ((question_source IS NULL) = (needs_input IS NULL))
            );
            INSERT INTO session_reports_with_sources
                (session, state, label, doing, needs_input, reported_at, launch_id, outcome,
                 outcome_reason, question_source)
                SELECT session, state, label, doing, needs_input, reported_at, launch_id,
                    outcome, outcome_reason, CASE WHEN needs_input IS NULL THEN NULL ELSE 'agent' END
                FROM session_reports;
            DROP TABLE session_reports;
            ALTER TABLE session_reports_with_sources RENAME TO session_reports;",
        },
        // #214: a dialog stacks on the agent's own question instead of replacing it; the
        // agent's question waits beside the dialog and shows again once it is answered.
        Migration {
            version: 5,
            sql: "ALTER TABLE session_reports ADD COLUMN covered_question TEXT
                CHECK (covered_question IS NULL OR question_source = 'dialog');",
        },
        // #219: a session's conversation that announced its end (`/clear`, an in-session
        // `/resume`) in the launch it names; the session follows the next one reported.
        Migration {
            version: 6,
            sql: "CREATE TABLE session_conversation_ends (
                session_id TEXT PRIMARY KEY CHECK (session_id <> ''),
                conversation TEXT NOT NULL CHECK (conversation <> ''),
                launch_id INTEGER
            );",
        },
        // #269 (decided in #312) and #273: where a launch made to give the session work
        // lies among the commits, which the finished label compares a finish with; `NULL`
        // for a launch that keeps the label. Earlier launches count as working ones, except
        // restores, ordered before every finish that has an order.
        Migration {
            version: 7,
            sql: "ALTER TABLE session_launches ADD COLUMN working_order INTEGER;
            UPDATE session_launches SET working_order = 0 WHERE kind <> 'restore';
            CREATE INDEX session_launches_by_working_order
                ON session_launches (session_id, working_order);",
        },
    ],
};

pub(crate) fn now() -> i64 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs();
    i64::try_from(seconds).expect("seconds since 1970 fit in an i64")
}

pub(crate) fn path_text(path: &Path) -> Result<&str, Error> {
    path.to_str().ok_or_else(|| Error::InvalidStoredRow {
        reason: format!("{} is not valid UTF-8", path.display()),
    })
}

fn optional_text(text: Option<&str>) -> Value {
    text.map_or(Value::Null, |text| Value::Text(text.to_owned()))
}

const SESSION_COLUMNS: &str = "s.id, s.provider, s.checkout_id, s.holds_checkout, s.working_dir,
    s.conversation, s.design_link, s.state, s.stopping_to, s.reason, s.revision, s.created_at,
    s.changed_at, l.id, l.kind, l.execution_id, l.launched_at,
    EXISTS (SELECT 1 FROM session_restart_requests r WHERE r.session_id = s.id),
    (SELECT max(w.working_order) FROM session_launches w WHERE w.session_id = s.id)";
const SESSION_FROM: &str = "sessions s LEFT JOIN session_launches l ON l.id =
    (SELECT max(id) FROM session_launches WHERE session_id = s.id)";

async fn query_sessions(
    connection: &Connection,
    filter: &str,
    params: impl cockpit_turso::sql::IntoParams,
) -> Result<Vec<Session>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {SESSION_COLUMNS} FROM {SESSION_FROM} WHERE {filter}
                 ORDER BY s.created_at, s.id"
            ),
            params,
        )
        .await?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next().await? {
        sessions.push(session_from_row(&row)?);
    }
    Ok(sessions)
}

/// Every session, or only those not ended, by creation.
pub(crate) async fn sessions(
    connection: &Connection,
    include_ended: bool,
) -> Result<Vec<Session>, Error> {
    let filter = if include_ended {
        "1"
    } else {
        "s.state <> 'ended'"
    };
    query_sessions(connection, filter, ()).await
}

pub(crate) async fn sessions_in(
    connection: &Connection,
    states: &[&str],
) -> Result<Vec<Session>, Error> {
    let placeholders: Vec<String> = (1..=states.len()).map(|n| format!("?{n}")).collect();
    let values: Vec<Value> = states
        .iter()
        .map(|state| Value::Text((*state).to_owned()))
        .collect();
    query_sessions(
        connection,
        &format!("s.state IN ({})", placeholders.join(", ")),
        cockpit_turso::sql::params_from_iter(values),
    )
    .await
}

pub(crate) async fn session(connection: &Connection, id: &SessionId) -> Result<Session, Error> {
    query_sessions(connection, "s.id = ?1", [id.as_str()])
        .await?
        .pop()
        .ok_or_else(|| Error::UnknownSession {
            session: id.clone(),
        })
}

/// The sessions other than `except` that hold `provider`'s conversation `conversation`.
pub(crate) async fn conversation_holders(
    connection: &Connection,
    provider: &ProviderName,
    conversation: &ConversationId,
    except: &SessionId,
) -> Result<Vec<Session>, Error> {
    query_sessions(
        connection,
        "s.provider = ?1 AND s.conversation = ?2 AND s.id <> ?3",
        (provider.as_str(), conversation.as_str(), except.as_str()),
    )
    .await
}

pub(crate) struct NewSession<'values> {
    pub(crate) id: &'values SessionId,
    pub(crate) provider: &'values ProviderName,
    pub(crate) checkout: CheckoutId,
    pub(crate) holds_checkout: bool,
    pub(crate) working_dir: &'values Path,
}

/// Records a new session, `launching` its first launch.
pub(crate) async fn insert_session(
    write: &WriteTransaction<'_>,
    new: &NewSession<'_>,
) -> Result<(), Error> {
    let at = now();
    write
        .execute(
            "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir,
                 state, revision, created_at, changed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'launching', 1, ?6, ?6)",
            (
                new.id.as_str(),
                new.provider.as_str(),
                new.checkout.0,
                i64::from(new.holds_checkout),
                path_text(new.working_dir)?,
                at,
            ),
        )
        .await?;
    record_transition(write, new.id, None, SessionState::Launching, None).await
}

async fn record_transition(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    from: Option<SessionState>,
    to: SessionState,
    reason: Option<&str>,
) -> Result<(), Error> {
    write
        .execute(
            "INSERT INTO session_transitions (session_id, from_state, to_state, reason, at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                id.as_str(),
                optional_text(from.map(|state| state.to_string()).as_deref()),
                to.to_string(),
                optional_text(reason),
                now(),
            ),
        )
        .await?;
    Ok(())
}

/// Moves the session from one of `from` to `to`, counting a revision and recording the
/// transition. A session in another state is refused for `operation`, unchanged.
pub(crate) async fn transition(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    from: &[SessionState],
    to: SessionState,
    reason: Option<&str>,
    operation: &'static str,
) -> Result<Session, Error> {
    let current = session(write, id).await?;
    if !from.contains(&current.state) {
        return Err(Error::WrongState {
            session: id.clone(),
            state: current.state,
            operation,
        });
    }
    write
        .execute(
            "UPDATE sessions SET state = ?2, stopping_to = ?3, reason = ?4,
                 revision = revision + 1, changed_at = ?5
             WHERE id = ?1",
            (
                id.as_str(),
                to.column(),
                optional_text(to.stop_column()),
                optional_text(reason),
                now(),
            ),
        )
        .await?;
    record_transition(write, id, Some(current.state), to, reason).await?;
    session(write, id).await
}

/// Whether the session's latest recorded transition went from `from` to `to`.
pub(crate) async fn latest_transition_is(
    connection: &Connection,
    id: &SessionId,
    from: SessionState,
    to: SessionState,
) -> Result<bool, Error> {
    let mut rows = connection
        .query(
            "SELECT from_state, to_state FROM session_transitions
             WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
            [id.as_str()],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(false);
    };
    // Compared as `record_transition` writes them.
    let latest_from: Option<String> = row.get(0)?;
    let latest_to: String = row.get(1)?;
    Ok(latest_from == Some(from.to_string()) && latest_to == to.to_string())
}

/// When the session last moved to asleep, if it is asleep now.
pub(crate) async fn entered_asleep_at(
    connection: &Connection,
    id: &SessionId,
) -> Result<Option<i64>, Error> {
    if session(connection, id).await?.state != SessionState::Asleep {
        return Ok(None);
    }
    let mut rows = connection
        .query(
            "SELECT at FROM session_transitions
             WHERE session_id = ?1 AND to_state = 'asleep'
             ORDER BY id DESC LIMIT 1",
            [id.as_str()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Err(Error::InvalidStoredRow {
            reason: format!("session {id} is asleep with no transition to asleep"),
        }),
    }
}

/// Records a launch and the start files its agent reads, as they were just before it ran.
pub(crate) async fn insert_launch(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    kind: LaunchKind,
    purpose: LaunchPurpose,
    execution: ExecutionId,
    start_files: &StartFileSnapshot,
) -> Result<(), Error> {
    let working_order = match purpose {
        LaunchPurpose::Work => Value::Integer(write.commit_order().await?),
        LaunchPurpose::Upkeep => Value::Null,
    };
    write
        .execute(
            "INSERT INTO session_launches
                (session_id, kind, execution_id, launched_at, working_order)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                id.as_str(),
                kind.column(),
                execution.0,
                now(),
                working_order,
            ),
        )
        .await?;
    let launch = write.last_insert_rowid();
    for file in &start_files.files {
        insert_start_file(write, launch, file).await?;
    }
    Ok(())
}

async fn insert_start_file(
    write: &WriteTransaction<'_>,
    launch: i64,
    file: &RecordedFile,
) -> Result<(), Error> {
    let (file_state, present, error) = match &file.state {
        FileState::Missing => ("missing", None, None),
        FileState::Present {
            modified,
            changed_at,
            inode,
            len,
        } => match nanos_since_epoch(*modified) {
            Some(modified) => ("present", Some((modified, *changed_at, *inode, *len)), None),
            // Kept as unreadable rather than failing the launch: the file then reads as
            // changed on every look, which the owner sees.
            None => (
                "unreadable",
                None,
                Some(format!(
                    "its modification time {modified:?} is out of the recorded range"
                )),
            ),
        },
        FileState::Unreadable { error } => ("unreadable", None, Some(error.clone())),
    };
    // Stored bit for bit: SQLite integers are signed.
    let (modified_ns, changed_s, changed_ns, inode, len) = match present {
        Some((modified, (seconds, nanos), inode, len)) => (
            Some(modified),
            Some(seconds),
            Some(nanos),
            Some(inode.cast_signed()),
            Some(len.cast_signed()),
        ),
        None => (None, None, None, None, None),
    };
    write
        .execute(
            "INSERT INTO session_start_files (launch_id, label, path, file_state, modified_ns,
                 changed_s, changed_ns, inode, len, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            cockpit_turso::sql::params_from_iter([
                Value::Integer(launch),
                Value::Text(file.label.clone()),
                Value::Text(path_text(&file.path)?.to_owned()),
                Value::Text(file_state.to_owned()),
                integer(modified_ns),
                integer(changed_s),
                integer(changed_ns),
                integer(inode),
                integer(len),
                optional_text(error.as_deref()),
            ]),
        )
        .await?;
    Ok(())
}

/// The start files the launch numbered `launch` recorded for its `provider`. A launch from
/// before they were recorded has none, so every file its provider reads counts as changed.
pub(crate) async fn start_files(
    connection: &Connection,
    launch: i64,
    provider: &str,
) -> Result<StartFileSnapshot, Error> {
    let mut rows = connection
        .query(
            "SELECT label, path, file_state, modified_ns, changed_s, changed_ns, inode, len, error
             FROM session_start_files WHERE launch_id = ?1 ORDER BY label",
            [launch],
        )
        .await?;
    let mut files = Vec::new();
    while let Some(row) = rows.next().await? {
        let label: String = row.get(0)?;
        let file_state: String = row.get(2)?;
        let state = match file_state.as_str() {
            "missing" => FileState::Missing,
            "present" => FileState::Present {
                modified: system_time(row.get(3)?),
                changed_at: (row.get(4)?, row.get(5)?),
                inode: row.get::<i64>(6)?.cast_unsigned(),
                len: row.get::<i64>(7)?.cast_unsigned(),
            },
            "unreadable" => FileState::Unreadable { error: row.get(8)? },
            other => {
                return Err(Error::InvalidStoredRow {
                    reason: format!("start file {label:?} of launch {launch}: state {other:?}"),
                });
            }
        };
        files.push(RecordedFile {
            label,
            path: PathBuf::from(row.get::<String>(1)?),
            state,
        });
    }
    Ok(StartFileSnapshot {
        provider: provider.to_owned(),
        files,
    })
}

fn integer(value: Option<i64>) -> Value {
    value.map_or(Value::Null, Value::Integer)
}

/// `None` past about the year 2262 either side of 1970, beyond an `i64` of nanoseconds.
fn nanos_since_epoch(time: SystemTime) -> Option<i64> {
    let nanos = match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i128::try_from(after.as_nanos()),
        Err(before) => i128::try_from(before.duration().as_nanos()).map(|nanos| -nanos),
    };
    nanos.ok().and_then(|nanos| i64::try_from(nanos).ok())
}

fn system_time(nanos: i64) -> SystemTime {
    let magnitude = Duration::from_nanos(nanos.unsigned_abs());
    if nanos >= 0 {
        UNIX_EPOCH + magnitude
    } else {
        UNIX_EPOCH - magnitude
    }
}

/// Asks for the session to restart once its agent waits at its prompt; asking twice keeps
/// the first request.
pub(crate) async fn request_restart(
    write: &WriteTransaction<'_>,
    id: &SessionId,
) -> Result<(), Error> {
    write
        .execute(
            "INSERT OR IGNORE INTO session_restart_requests (session_id, requested_at)
             VALUES (?1, ?2)",
            (id.as_str(), now()),
        )
        .await?;
    Ok(())
}

/// A launch gives the agent the start files as they are, so it answers any restart request;
/// a session that ends or fails has nothing left to restart.
pub(crate) async fn clear_restart_request(
    write: &WriteTransaction<'_>,
    id: &SessionId,
) -> Result<(), Error> {
    write
        .execute(
            "DELETE FROM session_restart_requests WHERE session_id = ?1",
            [id.as_str()],
        )
        .await?;
    Ok(())
}

pub(crate) async fn set_conversation(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    conversation: &ConversationId,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE sessions SET conversation = ?2 WHERE id = ?1",
            (id.as_str(), conversation.as_str()),
        )
        .await?;
    Ok(())
}

/// Records that the session's agent now runs `conversation`, as a change clients see; unlike
/// [`set_conversation`], whose launch transaction records its own transition.
pub(crate) async fn change_conversation(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    conversation: &ConversationId,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE sessions SET conversation = ?2, revision = revision + 1, changed_at = ?3
             WHERE id = ?1",
            (id.as_str(), conversation.as_str(), now()),
        )
        .await?;
    Ok(())
}

pub(crate) async fn set_design_link(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    link: &str,
) -> Result<Session, Error> {
    session(write, id).await?;
    write
        .execute(
            "UPDATE sessions SET design_link = ?2, revision = revision + 1, changed_at = ?3
             WHERE id = ?1",
            (id.as_str(), link, now()),
        )
        .await?;
    session(write, id).await
}

fn session_from_row(row: &Row) -> Result<Session, Error> {
    let id_text: String = row.get(0)?;
    let invalid = |reason: String| Error::InvalidStoredRow {
        reason: format!("session {id_text}: {reason}"),
    };
    let id = SessionId::parse(&id_text).map_err(|error| invalid(error.to_string()))?;
    let provider =
        ProviderName::new(row.get::<String>(1)?).map_err(|error| invalid(error.to_string()))?;
    let conversation = row
        .get::<Option<String>>(5)?
        .map(ConversationId::new)
        .transpose()
        .map_err(|error| invalid(error.to_string()))?;
    let state_text: String = row.get(7)?;
    let stopping_to: Option<String> = row.get(8)?;
    let state = SessionState::from_columns(&state_text, stopping_to.as_deref())
        .ok_or_else(|| invalid(format!("state {state_text:?} to {stopping_to:?}")))?;
    let launch = match row.get::<Option<i64>>(13)? {
        None => None,
        Some(number) => {
            let kind_text: String = row.get(14)?;
            Some(Launch {
                number,
                kind: LaunchKind::from_column(&kind_text)
                    .ok_or_else(|| invalid(format!("launch kind {kind_text:?}")))?,
                execution: ExecutionId(row.get(15)?),
                launched_at: row.get(16)?,
            })
        }
    };
    let restart_pending = row.get::<i64>(17)? == 1;
    let working_launch_order = row.get(18)?;
    Ok(Session {
        id,
        provider,
        checkout: CheckoutId(row.get(2)?),
        holds_checkout: row.get::<i64>(3)? == 1,
        working_dir: PathBuf::from(row.get::<String>(4)?),
        conversation,
        design_link: row.get(6)?,
        state,
        reason: row.get(9)?,
        revision: row.get(10)?,
        created_at: row.get(11)?,
        changed_at: row.get(12)?,
        launch,
        working_launch_order,
        restart_pending,
    })
}
