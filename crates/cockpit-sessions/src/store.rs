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
use crate::{
    AgentExit, Error, ExitStatus, Launch, LaunchBundle, LaunchKind, Session, SessionBundle,
    SessionId, SessionState,
};

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
        Migration {
            version: 8,
            sql: "CREATE TABLE session_delivery (
                execution_id INTEGER PRIMARY KEY,
                provider_program BLOB NOT NULL CHECK (length(provider_program) > 0),
                provider_conversation TEXT CHECK (provider_conversation <> ''),
                reach TEXT NOT NULL CHECK (reach IN ('direct', 'mounted', 'unreachable')),
                reason TEXT CHECK (reason <> ''),
                root_boot TEXT CHECK (root_boot <> ''),
                root_pid INTEGER CHECK (root_pid > 0),
                root_start INTEGER CHECK (root_start >= 0),
                CHECK ((reach = 'unreachable') = (reason IS NOT NULL)),
                CHECK ((root_boot IS NULL) = (root_pid IS NULL)
                    AND (root_pid IS NULL) = (root_start IS NULL))
            );
            CREATE TABLE session_delivery_mounts (
                execution_id INTEGER NOT NULL REFERENCES session_delivery (execution_id),
                host TEXT NOT NULL CHECK (host <> ''),
                sandbox TEXT NOT NULL CHECK (sandbox <> ''),
                PRIMARY KEY (execution_id, sandbox)
            );
            CREATE TABLE session_delivery_arguments (
                execution_id INTEGER NOT NULL REFERENCES session_delivery (execution_id),
                position INTEGER NOT NULL CHECK (position >= 0),
                value BLOB NOT NULL,
                PRIMARY KEY (execution_id, position)
            );
            CREATE TABLE session_delivery_environment (
                execution_id INTEGER NOT NULL REFERENCES session_delivery (execution_id),
                position INTEGER NOT NULL CHECK (position >= 0),
                name BLOB NOT NULL,
                value BLOB NOT NULL,
                PRIMARY KEY (execution_id, position)
            );",
        },
        // #629: how a launch's agent exited by itself, read from its terminal before the
        // session closed it; `output` is its last lines, newline-separated.
        Migration {
            version: 9,
            sql: "CREATE TABLE session_exits (
                launch_id INTEGER PRIMARY KEY,
                ended_by TEXT NOT NULL CHECK (ended_by IN ('code', 'signal', 'unknown')),
                code INTEGER,
                output TEXT NOT NULL,
                CHECK ((ended_by = 'code') = (code IS NOT NULL))
            );",
        },
        // #701: the model and effort chosen at the start, which every launch passes again.
        Migration {
            version: 10,
            sql: "ALTER TABLE sessions ADD COLUMN model TEXT CHECK (model <> '');
            ALTER TABLE sessions ADD COLUMN effort TEXT CHECK (effort <> '');",
        },
        // #710: the coordinator that started a session, whose agent's questions go to it
        // before the owner; when the agent's own question was asked, which starts its time
        // with the coordinator, and when the coordinator handed it to the owner. A question
        // recorded before has no time and waits on the owner, as it did.
        Migration {
            version: 11,
            sql: "ALTER TABLE sessions ADD COLUMN started_by TEXT CHECK (started_by <> '');
            ALTER TABLE session_reports ADD COLUMN question_asked_at INTEGER;
            ALTER TABLE session_reports ADD COLUMN question_escalated_at INTEGER
                CHECK (question_escalated_at IS NULL OR question_asked_at IS NOT NULL);",
        },
        // The session-end sweep reads each clone's sessions at every tick (#767).
        Migration {
            version: 12,
            sql: "CREATE INDEX sessions_by_checkout ON sessions (checkout_id);",
        },
        // #64 (report 13): the release whose helpers and instruction bundle a launch ran with,
        // which `version --all` shows. A launch with no row here started before launches
        // recorded it. Launch rows are never deleted, so no row here outlives its launch.
        Migration {
            version: 13,
            sql: "CREATE TABLE session_launch_bundles (
                launch_id INTEGER PRIMARY KEY,
                release TEXT NOT NULL CHECK (release <> ''),
                instruction_manifest TEXT NOT NULL CHECK (instruction_manifest <> '')
            );",
        },
        // #782: each context compaction a harness announced. Seconds come from the daemon's
        // own clock between the start and the end; a compaction whose start was not seen has
        // no start time.
        Migration {
            version: 14,
            sql: "CREATE TABLE session_compactions (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                launch_id INTEGER NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'failed')),
                trigger TEXT CHECK (trigger IN ('auto', 'manual')),
                tokens_before INTEGER CHECK (tokens_before >= 0),
                tokens_after INTEGER CHECK (tokens_after >= 0),
                started_at INTEGER,
                ended_at INTEGER,
                reported_at INTEGER NOT NULL
            );
            CREATE INDEX session_compactions_by_session
                ON session_compactions (session_id, launch_id, id);",
        },
        // #783: the harness's own compaction threshold chosen at the start, in the provider's
        // own words, which every launch passes again.
        Migration {
            version: 15,
            sql: "ALTER TABLE sessions ADD COLUMN compaction_threshold TEXT
                CHECK (compaction_threshold <> '');",
        },
        // #479: the harness's own context at the end of the latest turn, in its tokens. A
        // report recorded before has none, as a launch before its first turn ends.
        Migration {
            version: 16,
            sql: "ALTER TABLE session_reports ADD COLUMN context_tokens INTEGER
                CHECK (context_tokens >= 0);
            ALTER TABLE session_reports ADD COLUMN context_window INTEGER
                CHECK (context_window IS NULL
                    OR (context_window > 0 AND context_tokens IS NOT NULL));",
        },
        // #59: the account a launch's agent runs on, the daemon's own record: chosen at the
        // launch, never read back from the session. `NULL` for a launch on no account (an
        // unsandboxed agent signs in from the owner's home, or no account had a credential)
        // and for launches recorded before.
        Migration {
            version: 17,
            sql: "ALTER TABLE session_launches ADD COLUMN account TEXT CHECK (account <> '');",
        },
        // #59: when each account last quota-stopped, which the next launches pass over for a
        // while, and the account a launch moved off after its session's quota stop.
        Migration {
            version: 18,
            sql: "CREATE TABLE account_quota_stops (
                provider TEXT NOT NULL CHECK (provider <> ''),
                account TEXT NOT NULL CHECK (account <> ''),
                stopped_at INTEGER NOT NULL,
                PRIMARY KEY (provider, account)
            );
            ALTER TABLE session_launches ADD COLUMN switched_from TEXT
                CHECK (switched_from <> '');",
        },
        // #59: the login accounts a launch passed over for want of the project's grant, one
        // line each naming the account and the grant that lets it; `NULL` for none.
        Migration {
            version: 19,
            sql: "ALTER TABLE session_launches ADD COLUMN passed_over TEXT
                CHECK (passed_over <> '');",
        },
        // #59: whether a launch bound its account's signed-in login, which one session at a
        // time uses; a launch on a credential shares nothing that refreshes.
        Migration {
            version: 20,
            sql: "ALTER TABLE session_launches ADD COLUMN uses_login INTEGER NOT NULL DEFAULT 0
                CHECK (uses_login IN (0, 1));",
        },
        // #158: each quota stop and its one resume at the reset, or why it was dropped.
        Migration {
            version: 21,
            sql: "CREATE TABLE session_quota_stops (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL CHECK (session_id <> ''),
                launch_id INTEGER NOT NULL,
                provider TEXT NOT NULL CHECK (provider <> ''),
                account TEXT CHECK (account <> ''),
                stopped_at INTEGER NOT NULL,
                confirmed_at INTEGER,
                resets_at INTEGER,
                switched_at INTEGER,
                resumed_at INTEGER,
                dropped_at INTEGER,
                dropped_reason TEXT CHECK (dropped_reason <> ''),
                CHECK ((dropped_at IS NULL) = (dropped_reason IS NULL)),
                CHECK (resumed_at IS NULL OR dropped_at IS NULL)
            );
            CREATE INDEX session_quota_stops_by_session ON session_quota_stops (session_id, id);",
        },
        // #158: failed turns whose reason a provider gives for a spent plan and a transient
        // limit alike, counted towards a quota stop; and what such a stop was inferred from.
        Migration {
            version: 22,
            sql: "CREATE TABLE session_limit_failures (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL CHECK (session_id <> ''),
                launch_id INTEGER NOT NULL,
                reason TEXT NOT NULL CHECK (reason <> ''),
                failed_at INTEGER NOT NULL
            );
            CREATE INDEX session_limit_failures_by_session
                ON session_limit_failures (session_id, launch_id, failed_at);
            ALTER TABLE session_quota_stops ADD COLUMN inferred_from TEXT
                CHECK (inferred_from <> '');",
        },
        // #718: a turn that ended because the provider was unreachable, always with the
        // harness's own word for it.
        Migration {
            version: 23,
            sql: "CREATE TABLE session_reports_with_network_lost (
                session TEXT PRIMARY KEY CHECK (session <> ''),
                state TEXT CHECK (state IN ('working', 'waiting')),
                label TEXT,
                doing TEXT,
                needs_input TEXT,
                reported_at INTEGER NOT NULL,
                launch_id INTEGER,
                outcome TEXT
                    CHECK (outcome IN ('completed', 'failed', 'quota-stopped', 'network-lost')),
                outcome_reason TEXT
                    CHECK (outcome_reason IS NULL OR outcome IN ('failed', 'network-lost')),
                question_source TEXT CHECK (question_source IN ('agent', 'dialog')),
                covered_question TEXT
                    CHECK (covered_question IS NULL OR question_source = 'dialog'),
                question_asked_at INTEGER,
                question_escalated_at INTEGER
                    CHECK (question_escalated_at IS NULL OR question_asked_at IS NOT NULL),
                context_tokens INTEGER CHECK (context_tokens >= 0),
                context_window INTEGER
                    CHECK (context_window IS NULL
                        OR (context_window > 0 AND context_tokens IS NOT NULL)),
                CHECK ((question_source IS NULL) = (needs_input IS NULL)),
                CHECK (outcome IS NULL OR outcome <> 'network-lost' OR outcome_reason IS NOT NULL)
            );
            INSERT INTO session_reports_with_network_lost
                (session, state, label, doing, needs_input, reported_at, launch_id, outcome,
                 outcome_reason, question_source, covered_question, question_asked_at,
                 question_escalated_at, context_tokens, context_window)
                SELECT session, state, label, doing, needs_input, reported_at, launch_id,
                    outcome, outcome_reason, question_source, covered_question,
                    question_asked_at, question_escalated_at, context_tokens, context_window
                FROM session_reports;
            DROP TABLE session_reports;
            ALTER TABLE session_reports_with_network_lost RENAME TO session_reports;",
        },
        // #718: a turn the network ended is resumed by the quota stops' pass once the
        // provider answers again.
        Migration {
            version: 24,
            sql: "ALTER TABLE session_quota_stops ADD COLUMN cause TEXT NOT NULL DEFAULT 'quota'
                CHECK (cause IN ('quota', 'network'));",
        },
        // #412: the owner's budget of agent minutes for the session; past it the queue places
        // nothing more on it.
        Migration {
            version: 25,
            sql: "ALTER TABLE sessions ADD COLUMN budget_minutes INTEGER
                CHECK (budget_minutes >= 1);",
        },
        // #412: a turn ended past the session's budget; the owner's next budget clears it, so
        // the next spent budget is news again.
        Migration {
            version: 26,
            sql: "ALTER TABLE sessions ADD COLUMN is_budget_spent INTEGER NOT NULL DEFAULT 0
                CHECK (is_budget_spent IN (0, 1));",
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
    (SELECT max(w.working_order) FROM session_launches w WHERE w.session_id = s.id),
    e.ended_by, e.code, e.output, s.model, s.effort, s.started_by, s.compaction_threshold,
    l.account, l.switched_from, l.passed_over, s.budget_minutes, s.is_budget_spent";
const SESSION_FROM: &str = "sessions s LEFT JOIN session_launches l ON l.id =
    (SELECT max(id) FROM session_launches WHERE session_id = s.id)
    LEFT JOIN session_exits e ON e.launch_id = l.id";

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
/// Session snapshots read within the caller's transaction.
pub async fn sessions(connection: &Connection, include_ended: bool) -> Result<Vec<Session>, Error> {
    let filter = if include_ended {
        "1"
    } else {
        "s.state <> 'ended'"
    };
    query_sessions(connection, filter, ()).await
}

/// Every session, ended ones included, that worked in the checkout `checkout`.
pub async fn checkout_sessions(
    connection: &Connection,
    checkout: CheckoutId,
) -> Result<Vec<Session>, Error> {
    query_sessions(connection, "s.checkout_id = ?1", [checkout.0]).await
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

/// One session snapshot read within the caller's transaction.
pub async fn session(connection: &Connection, id: &SessionId) -> Result<Session, Error> {
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
    pub(crate) model: Option<&'values str>,
    pub(crate) effort: Option<&'values str>,
    pub(crate) compaction_threshold: Option<&'values str>,
    pub(crate) started_by: Option<&'values SessionId>,
}

/// Records a new session, `launching` its first launch.
pub(crate) async fn insert_session(
    write: &WriteTransaction<'_>,
    new: &NewSession<'_>,
) -> Result<(), Error> {
    cockpit_projects::require_execution_start_allowed(write).await?;
    cockpit_projects::require_checkout_available(write, new.checkout).await?;
    let at = now();
    write
        .execute(
            "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir,
                 state, revision, created_at, changed_at, model, effort, started_by,
                 compaction_threshold)
             VALUES (?1, ?2, ?3, ?4, ?5, 'launching', 1, ?6, ?6, ?7, ?8, ?9, ?10)",
            (
                new.id.as_str(),
                new.provider.as_str(),
                new.checkout.0,
                i64::from(new.holds_checkout),
                path_text(new.working_dir)?,
                at,
                optional_text(new.model),
                optional_text(new.effort),
                optional_text(new.started_by.map(SessionId::as_str)),
                optional_text(new.compaction_threshold),
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
    if to == SessionState::Launching {
        cockpit_projects::require_execution_start_allowed(write).await?;
    }
    let current = session(write, id).await?;
    if to == SessionState::Launching {
        cockpit_projects::require_checkout_available(write, current.checkout).await?;
    }
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

/// Records how the session's latest launch's agent exited.
pub(crate) async fn insert_exit(
    write: &WriteTransaction<'_>,
    launch: &Launch,
    exit: &AgentExit,
) -> Result<(), Error> {
    let (ended_by, code) = match exit.status {
        ExitStatus::Code(code) => ("code", Value::Integer(i64::from(code))),
        ExitStatus::Signal => ("signal", Value::Null),
        ExitStatus::Unknown => ("unknown", Value::Null),
    };
    write
        .execute(
            "INSERT INTO session_exits (launch_id, ended_by, code, output) VALUES (?1, ?2, ?3, ?4)",
            (launch.number, ended_by, code, exit.output.join("\n")),
        )
        .await?;
    Ok(())
}

/// Records a launch and the start files its agent reads, as they were just before it ran.
#[allow(
    clippy::too_many_arguments,
    reason = "one launch's facts, each written into its rows"
)]
pub(crate) async fn insert_launch(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    kind: LaunchKind,
    purpose: LaunchPurpose,
    execution: ExecutionId,
    start_files: &StartFileSnapshot,
    bundle: Option<&LaunchBundle>,
    account: Option<&str>,
    switched_from: Option<&str>,
    passed_over: &[String],
    uses_login: bool,
) -> Result<(), Error> {
    let working_order = match purpose {
        LaunchPurpose::Work => Value::Integer(write.commit_order().await?),
        LaunchPurpose::Upkeep => Value::Null,
    };
    write
        .execute(
            "INSERT INTO session_launches
                (session_id, kind, execution_id, launched_at, working_order, account,
                 switched_from, passed_over, uses_login)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            (
                id.as_str(),
                kind.column(),
                execution.0,
                now(),
                working_order,
                optional_text(account),
                optional_text(switched_from),
                optional_text(
                    Some(passed_over.join("\n"))
                        .filter(|lines| !lines.is_empty())
                        .as_deref(),
                ),
                i64::from(uses_login),
            ),
        )
        .await?;
    let launch = write.last_insert_rowid();
    for file in &start_files.files {
        insert_start_file(write, launch, file).await?;
    }
    if let Some(bundle) = bundle {
        write
            .execute(
                "INSERT INTO session_launch_bundles (launch_id, release, instruction_manifest)
                 VALUES (?1, ?2, ?3)",
                (
                    launch,
                    bundle.release.as_str(),
                    bundle.instruction_manifest.as_str(),
                ),
            )
            .await?;
    }
    Ok(())
}

/// Records that `launch`'s account quota-stopped now (#59); nothing for a launch on no
/// account.
pub(crate) async fn record_quota_stop(
    write: &WriteTransaction<'_>,
    launch: i64,
) -> Result<(), Error> {
    let mut rows = write
        .query(
            "SELECT s.provider, l.account FROM session_launches l
             JOIN sessions s ON s.id = l.session_id
             WHERE l.id = ?1 AND l.account IS NOT NULL",
            [launch],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(());
    };
    let (provider, account): (String, String) = (row.get(0)?, row.get(1)?);
    drop(rows);
    write
        .execute(
            "INSERT INTO account_quota_stops (provider, account, stopped_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (provider, account) DO UPDATE SET stopped_at = excluded.stopped_at",
            (provider, account, now()),
        )
        .await?;
    Ok(())
}

/// `provider`'s accounts that quota-stopped at or after `since`, seconds since 1970.
pub(crate) async fn accounts_stopped_since(
    connection: &Connection,
    provider: &str,
    since: i64,
) -> Result<Vec<String>, Error> {
    let mut rows = connection
        .query(
            "SELECT account FROM account_quota_stops
             WHERE provider = ?1 AND stopped_at >= ?2 ORDER BY account",
            (provider, since),
        )
        .await?;
    let mut accounts = Vec::new();
    while let Some(row) = rows.next().await? {
        accounts.push(row.get(0)?);
    }
    Ok(accounts)
}

/// Each session's latest launch, as `s` and `l`, kept to the sessions whose agent is starting,
/// running or stopping by [`IS_LIVE`].
const LATEST_LAUNCHES: &str = "sessions s JOIN session_launches l ON l.id =
    (SELECT max(id) FROM session_launches WHERE session_id = s.id)";

const IS_LIVE: &str = "s.state IN ('launching', 'running', 'stopping')";

/// Each login account of `provider` whose latest launch of a session other than `except` is
/// live and bound its login, with that session, by account (#59): one session at a time uses
/// a login account. A session asleep, failed or ended holds none.
pub(crate) async fn logins_in_use(
    connection: &Connection,
    provider: &str,
    except: Option<&SessionId>,
) -> Result<Vec<(String, SessionId)>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT l.account, s.id FROM {LATEST_LAUNCHES}
                 WHERE s.provider = ?1 AND l.uses_login = 1 AND s.id IS NOT ?2 AND {IS_LIVE}
                 ORDER BY l.account, s.created_at, s.id"
            ),
            (provider, except.map(SessionId::as_str)),
        )
        .await?;
    let mut in_use = Vec::new();
    while let Some(row) = rows.next().await? {
        let id_text: String = row.get(1)?;
        let id = SessionId::parse(&id_text).map_err(|error| Error::InvalidStoredRow {
            reason: format!("session {id_text}: {error}"),
        })?;
        in_use.push((row.get(0)?, id));
    }
    Ok(in_use)
}

/// The bundle the latest launch of each session whose agent is starting, running or stopping
/// ran with, by creation. A session asleep, failed or ended launches again with whatever
/// bundle the daemon then holds, so it is not listed; nor is one whose first launch is not
/// recorded yet, which has run nothing.
pub async fn live_bundles(connection: &Connection) -> Result<Vec<SessionBundle>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT s.id, s.state, s.stopping_to, b.release, b.instruction_manifest
                 FROM {LATEST_LAUNCHES}
                 LEFT JOIN session_launch_bundles b ON b.launch_id = l.id
                 WHERE {IS_LIVE}
                 ORDER BY s.created_at, s.id"
            ),
            (),
        )
        .await?;
    let mut bundles = Vec::new();
    while let Some(row) = rows.next().await? {
        let id_text: String = row.get(0)?;
        let invalid = |reason: String| Error::InvalidStoredRow {
            reason: format!("session {id_text}: {reason}"),
        };
        let state_text: String = row.get(1)?;
        let stopping_to: Option<String> = row.get(2)?;
        bundles.push(SessionBundle {
            id: SessionId::parse(&id_text).map_err(|error| invalid(error.to_string()))?,
            state: SessionState::from_columns(&state_text, stopping_to.as_deref())
                .ok_or_else(|| invalid(format!("state {state_text:?} to {stopping_to:?}")))?,
            bundle: match (row.get::<Option<String>>(3)?, row.get::<Option<String>>(4)?) {
                (Some(release), Some(instruction_manifest)) => Some(LaunchBundle {
                    release,
                    instruction_manifest,
                }),
                _ => None,
            },
        });
    }
    Ok(bundles)
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

/// Marks that a turn of `id` ended past its budget (#412). Marking it again changes nothing,
/// so later turns past the same budget tell nothing new.
pub async fn mark_budget_spent(write: &WriteTransaction<'_>, id: &SessionId) -> Result<(), Error> {
    write
        .execute(
            "UPDATE sessions SET is_budget_spent = 1, revision = revision + 1, changed_at = ?2
             WHERE id = ?1 AND is_budget_spent = 0",
            (id.as_str(), now()),
        )
        .await?;
    Ok(())
}

/// Sets or lifts (`None`) `id`'s budget of agent minutes and clears its spent mark (#412).
pub(crate) async fn set_budget(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    minutes: Option<u32>,
) -> Result<Session, Error> {
    session(write, id).await?;
    write
        .execute(
            "UPDATE sessions SET budget_minutes = ?2, is_budget_spent = 0,
                revision = revision + 1, changed_at = ?3
             WHERE id = ?1",
            (id.as_str(), minutes.map(i64::from), now()),
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
                account: row.get(26)?,
                switched_from: row.get(27)?,
                passed_over: row
                    .get::<Option<String>>(28)?
                    .map(|lines| lines.lines().map(str::to_owned).collect())
                    .unwrap_or_default(),
            })
        }
    };
    let restart_pending = row.get::<i64>(17)? == 1;
    let working_launch_order = row.get(18)?;
    let exit = match row.get::<Option<String>>(19)? {
        None => None,
        Some(ended_by) => {
            let status = match (ended_by.as_str(), row.get::<Option<i64>>(20)?) {
                ("code", Some(code)) => ExitStatus::Code(
                    i32::try_from(code).map_err(|_| invalid(format!("exit code {code}")))?,
                ),
                ("signal", None) => ExitStatus::Signal,
                ("unknown", None) => ExitStatus::Unknown,
                (ended_by, code) => return Err(invalid(format!("exit {ended_by:?} {code:?}"))),
            };
            let output: String = row.get(21)?;
            Some(AgentExit {
                status,
                output: output.lines().map(str::to_owned).collect(),
            })
        }
    };
    let started_by = row
        .get::<Option<String>>(24)?
        .map(|starter| SessionId::parse(&starter))
        .transpose()
        .map_err(|error| invalid(format!("started_by: {error}")))?;
    let budget_minutes = row
        .get::<Option<i64>>(29)?
        .map(u32::try_from)
        .transpose()
        .map_err(|error| invalid(format!("budget_minutes: {error}")))?;
    Ok(Session {
        id,
        provider,
        checkout: CheckoutId(row.get(2)?),
        holds_checkout: row.get::<i64>(3)? == 1,
        working_dir: PathBuf::from(row.get::<String>(4)?),
        conversation,
        model: row.get(22)?,
        effort: row.get(23)?,
        compaction_threshold: row.get(25)?,
        design_link: row.get(6)?,
        state,
        reason: row.get(9)?,
        revision: row.get(10)?,
        created_at: row.get(11)?,
        changed_at: row.get(12)?,
        launch,
        exit,
        working_launch_order,
        restart_pending,
        started_by,
        budget_minutes,
        is_budget_spent: row.get::<i64>(30)? == 1,
    })
}

/// Bounded current lifecycle evidence on the maintenance reservation's own connection.
/// A full sentinel page is uncertainty, never proof that omitted sessions ended.
pub async fn maintenance_sessions(connection: &Connection) -> Result<Vec<Session>, Error> {
    let mut rows = connection
        .query(
            &format!("SELECT {SESSION_COLUMNS} FROM {SESSION_FROM} ORDER BY s.id LIMIT 4097"),
            (),
        )
        .await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(session_from_row(&row)?);
    }
    Ok(result)
}

/// Supplies current live/latest protection in the caller's snapshot. Output facts own the
/// project/mode; sessions owns whether the attributed session is live and its latest launch.
/// Unknown session identity is protected, never interpreted as ended.
pub async fn protects_output(
    connection: &Connection,
    output: &cockpit_processes::OutputFacts,
) -> Result<bool, Error> {
    let Some(session) = output.session.as_deref() else {
        return Ok(false);
    };
    let mut rows = connection.query("SELECT state, (SELECT max(execution_id) FROM session_launches WHERE session_id=s.id) FROM sessions s WHERE id=?1", [session]).await?;
    let Some(row) = rows.next().await? else {
        return Ok(true);
    };
    if row.get::<String>(0)? == "ended" {
        return Ok(false);
    }
    let latest: Option<i64> = row.get(1)?;
    drop(rows);
    Ok(latest == Some(output.execution.0)
        || cockpit_processes::latest_file_output(connection, session).await?
            == Some(output.execution))
}
