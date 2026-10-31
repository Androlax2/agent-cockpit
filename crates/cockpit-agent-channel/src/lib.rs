#![doc = include_str!("../README.md")]

mod answers;
mod error;
mod launch;
mod process;
mod reports;

use cockpit_turso::{Migration, MigrationSet};

pub use answers::{AnswersDirectory, AnswersError, ProjectKey};
pub use error::{Error, Refusal, Unblock, UnblockedBy};
pub use launch::{LaunchToken, PeerAncestry, identify, mint, record_root};
pub use process::{ProcessError, ProcessIdentity, ancestry, identity_of};
pub use reports::{AgentState, Report, SessionReport, latest_reports, report};

/// The channel's tables. A launch is pending (`token` set, `bound_at` unset) until a report
/// from its tree binds it; binding clears the token, so it is spent. The root columns are the
/// process the launcher started, unset until `record_root`.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "agent-channel",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE agent_channel_launches (
                session TEXT PRIMARY KEY CHECK (session <> ''),
                token TEXT UNIQUE CHECK (token IS NULL OR length(token) = 64),
                root_boot TEXT CHECK (root_boot <> ''),
                root_pid INTEGER,
                root_start INTEGER,
                minted_at INTEGER NOT NULL,
                bound_at INTEGER,
                CHECK ((root_boot IS NULL) = (root_pid IS NULL)
                    AND (root_pid IS NULL) = (root_start IS NULL)),
                CHECK ((bound_at IS NULL) = (token IS NOT NULL)),
                CHECK (bound_at IS NULL OR root_pid IS NOT NULL)
            );
            CREATE INDEX agent_channel_launch_roots
                ON agent_channel_launches (root_pid, root_start);
            CREATE TABLE agent_channel_reports (
                session TEXT PRIMARY KEY CHECK (session <> ''),
                state TEXT CHECK (state IN ('working', 'waiting')),
                label TEXT,
                doing TEXT,
                needs_input TEXT,
                reported_at INTEGER NOT NULL
            );",
    }],
};
