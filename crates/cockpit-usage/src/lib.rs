#![doc = include_str!("../README.md")]

mod error;
mod model;
mod quota;

use cockpit_turso::{Migration, MigrationSet};

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
    migrations: &[Migration {
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
    }],
};
