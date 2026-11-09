#![doc = include_str!("../README.md")]

mod activity;
mod cost;
mod day;
mod digest;
mod error;
mod model;
mod prices;
mod quota;

use cockpit_turso::{Migration, MigrationSet};

pub use activity::{
    Approval, DayTime, Incident, JOIN_GAP_SECONDS, MAX_TURN_SECONDS, OpenTurn, ProjectTime, Turn,
    open_turns, record_approval, record_incident, record_turn, time_with_agent, turn_began,
    turn_ended, turn_seconds,
};
pub use cost::{
    DayCost, Recorded, costs, cursors, forget_home, price_unpriced, project_costs_since,
    record_replies,
};
pub use day::{Day, MAX_RANGE_DAYS};
pub use digest::{DayDigest, ProjectDay, digests};
pub use error::Error;
pub use model::{
    AccountName, AccountQuota, Alert, DEFAULT_ALERT_THRESHOLDS, DEFAULT_FRESH_FOR_SECONDS, Failure,
    MAX_FAILURE_CHARS, QuotaPolicy, WindowQuota, WindowState,
};
pub use quota::{quotas, record_failure, record_reading};

/// The quota tables. An account's row keeps its last reading and its last failed refresh; its
/// windows are replaced by each reading; an alert row says a threshold already fired for the
/// window instance named by `window_reset`, so a restart never repeats it.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "usage",
    migrations: &[
        Migration {
            version: 1,
            sql: "CREATE TABLE usage_quota_accounts (
            provider TEXT NOT NULL CHECK (provider <> ''),
            account TEXT NOT NULL CHECK (account <> ''),
            plan TEXT CHECK (plan <> ''),
            is_blocked INTEGER CHECK (is_blocked IN (0, 1)),
            observed_at INTEGER,
            failed_at INTEGER,
            failure TEXT CHECK (failure <> ''),
            PRIMARY KEY (provider, account),
            CHECK ((observed_at IS NULL) = (is_blocked IS NULL)),
            CHECK (observed_at IS NOT NULL OR plan IS NULL),
            CHECK ((failed_at IS NULL) = (failure IS NULL)),
            CHECK (observed_at IS NOT NULL OR failed_at IS NOT NULL)
        );
        CREATE TABLE usage_quota_windows (
            provider TEXT NOT NULL,
            account TEXT NOT NULL,
            limit_key TEXT NOT NULL CHECK (limit_key <> ''),
            scope TEXT CHECK (scope <> ''),
            length_seconds INTEGER CHECK (length_seconds > 0),
            used_percent REAL NOT NULL CHECK (used_percent >= 0),
            resets_at INTEGER,
            PRIMARY KEY (provider, account, limit_key)
        );
        CREATE TABLE usage_quota_alerts (
            provider TEXT NOT NULL,
            account TEXT NOT NULL,
            limit_key TEXT NOT NULL,
            threshold INTEGER NOT NULL CHECK (threshold BETWEEN 1 AND 100),
            window_reset INTEGER,
            used_percent REAL NOT NULL CHECK (used_percent >= threshold),
            fired_at INTEGER NOT NULL,
            PRIMARY KEY (provider, account, limit_key, threshold)
        );",
        },
        // #699: the cost archive. A reply is one row per provider and reply key, whichever
        // account's home it was read in first; its cost is in nano-dollars, null for a model
        // without a known price. A cursor says how far one home's record source was counted.
        Migration {
            version: 2,
            sql: "CREATE TABLE usage_cost_replies (
            provider TEXT NOT NULL CHECK (provider <> ''),
            reply_key TEXT NOT NULL CHECK (reply_key <> ''),
            account TEXT NOT NULL CHECK (account <> ''),
            project TEXT NOT NULL CHECK (project <> ''),
            day TEXT NOT NULL CHECK (day <> ''),
            at INTEGER NOT NULL,
            model TEXT NOT NULL CHECK (model <> ''),
            input_tokens INTEGER NOT NULL CHECK (input_tokens >= 0),
            output_tokens INTEGER NOT NULL CHECK (output_tokens >= 0),
            cache_read_tokens INTEGER NOT NULL CHECK (cache_read_tokens >= 0),
            cache_write_5m_tokens INTEGER NOT NULL CHECK (cache_write_5m_tokens >= 0),
            cache_write_1h_tokens INTEGER NOT NULL CHECK (cache_write_1h_tokens >= 0),
            is_fast INTEGER NOT NULL CHECK (is_fast IN (0, 1)),
            cost_nanos INTEGER CHECK (cost_nanos >= 0),
            PRIMARY KEY (provider, reply_key)
        );
        CREATE INDEX usage_cost_replies_by_day ON usage_cost_replies (day);
        CREATE TABLE usage_cost_cursors (
            provider TEXT NOT NULL,
            home_scope TEXT NOT NULL CHECK (home_scope <> ''),
            source TEXT NOT NULL CHECK (source <> ''),
            read_to INTEGER NOT NULL CHECK (read_to >= 0),
            PRIMARY KEY (provider, home_scope, source)
        );",
        },
        // #700: what time with the agent and the daily digest are made of, recorded as it
        // happens. A turn is its start and end; an approval and an incident are one row each
        // under the key the caller names them by.
        Migration {
            version: 3,
            sql: "CREATE TABLE usage_turns (
            session TEXT NOT NULL CHECK (session <> ''),
            project TEXT NOT NULL CHECK (project <> ''),
            day TEXT NOT NULL CHECK (day <> ''),
            started_at INTEGER NOT NULL,
            ended_at INTEGER NOT NULL CHECK (ended_at >= started_at),
            PRIMARY KEY (session, started_at)
        );
        CREATE INDEX usage_turns_by_start ON usage_turns (started_at);
        CREATE INDEX usage_turns_by_day ON usage_turns (day);
        CREATE TABLE usage_approvals (
            key TEXT PRIMARY KEY CHECK (key <> ''),
            project TEXT NOT NULL CHECK (project <> ''),
            day TEXT NOT NULL CHECK (day <> ''),
            at INTEGER NOT NULL
        );
        CREATE INDEX usage_approvals_by_day ON usage_approvals (day);
        CREATE TABLE usage_incidents (
            provider TEXT NOT NULL CHECK (provider <> ''),
            key TEXT NOT NULL CHECK (key <> ''),
            title TEXT NOT NULL CHECK (title <> ''),
            started_at INTEGER NOT NULL,
            resolved_at INTEGER CHECK (resolved_at >= started_at),
            PRIMARY KEY (provider, key)
        );",
        },
        // #700: the turn a session is in now, kept until its end is known, so a restart
        // closes it from the session's own record and none is lost or counted twice.
        Migration {
            version: 4,
            sql: "CREATE TABLE usage_open_turns (
            session TEXT PRIMARY KEY CHECK (session <> ''),
            project TEXT NOT NULL CHECK (project <> ''),
            started_at INTEGER NOT NULL
        );",
        },
    ],
};
