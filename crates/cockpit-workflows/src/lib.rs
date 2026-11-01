#![doc = include_str!("../README.md")]

mod error;
mod model;
mod packet;
mod presets;
mod runs;
mod text;

use cockpit_turso::{Migration, MigrationSet};

/// Who acts, as the daemon authenticated them; the same actor `cockpit-tasks` takes.
pub use cockpit_tasks::Actor;
pub use error::{Error, Refusal, UnblockedBy};
pub use model::{
    Blocked, BlockedAction, Checkpoint, Handoff, HandoffKind, HandoffRecord, NextAction, ReadyStep,
    Run, RunFilter, RunId, RunStatus, Step, StepStatus,
};
pub use packet::{Goal, Packet, PacketStep, packet};
pub use presets::{
    CODE_CHANGE, DESIGN_LED, Doer, PRESETS, Preset, RESEARCH, StepDefinition, preset,
    preset_version, presets, select_preset,
};
pub use runs::{
    Completion, begin, block, cancel, check_capabilities, checkpoint, complete, handoffs,
    ready_steps, release, run, runs, start, unblock,
};
pub use text::{
    BLOCKED_DETAIL_MAX_CHARS, CHECKPOINT_MAX_CHARS, EVIDENCE_MAX, EVIDENCE_MAX_CHARS,
    HANDOFF_MAX_BYTES,
};

/// The workflow tables. A run names its item without a foreign key: the item belongs to
/// `cockpit-tasks`, which may delete it, and a run's history outlives that. Steps name their
/// preset step so a release whose preset disagrees with a stored run fails loud.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "workflows",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE workflows_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item INTEGER NOT NULL,
                project TEXT NOT NULL CHECK (project <> ''),
                preset TEXT NOT NULL CHECK (preset <> ''),
                preset_version INTEGER NOT NULL CHECK (preset_version >= 1),
                status TEXT NOT NULL CHECK (status IN ('running', 'delivered', 'cancelled')),
                intent_revision INTEGER NOT NULL CHECK (intent_revision >= 1),
                blocked_action TEXT CHECK (blocked_action IN ('answer-question',
                    'approve-operation', 'restore-capability', 'wait-for-capacity',
                    'reconcile-result')),
                blocked_detail TEXT CHECK (blocked_detail <> ''),
                blocked_since INTEGER,
                created_at INTEGER NOT NULL,
                changed_at INTEGER NOT NULL,
                CHECK ((blocked_action IS NULL) = (blocked_detail IS NULL)),
                CHECK ((blocked_action IS NULL) = (blocked_since IS NULL)),
                CHECK (status = 'running' OR blocked_action IS NULL)
            );
            CREATE UNIQUE INDEX workflows_runs_running ON workflows_runs (item)
                WHERE status = 'running';
            CREATE INDEX workflows_runs_by_project ON workflows_runs (project, status);
            CREATE TABLE workflows_steps (
                run INTEGER NOT NULL REFERENCES workflows_runs (id),
                position INTEGER NOT NULL CHECK (position >= 1),
                name TEXT NOT NULL CHECK (name <> ''),
                status TEXT NOT NULL CHECK (status IN ('pending', 'ready', 'active', 'done')),
                session TEXT CHECK (session <> ''),
                checkpoint TEXT CHECK (checkpoint <> ''),
                checkpoint_revision INTEGER NOT NULL CHECK (checkpoint_revision >= 0),
                checkpoint_at INTEGER,
                attempts INTEGER NOT NULL CHECK (attempts >= 0),
                finished_at INTEGER,
                PRIMARY KEY (run, position),
                CHECK ((status = 'active') = (session IS NOT NULL)),
                CHECK ((checkpoint IS NULL) = (checkpoint_at IS NULL)),
                CHECK ((status = 'done') = (finished_at IS NOT NULL))
            );
            CREATE INDEX workflows_steps_by_session ON workflows_steps (session)
                WHERE session IS NOT NULL;
            CREATE TABLE workflows_handoffs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run INTEGER NOT NULL REFERENCES workflows_runs (id),
                position INTEGER NOT NULL CHECK (position >= 1),
                kind TEXT NOT NULL CHECK (kind IN ('completed', 'continued')),
                session TEXT CHECK (session <> ''),
                changed TEXT NOT NULL CHECK (changed <> ''),
                checked TEXT NOT NULL CHECK (checked <> ''),
                outcome TEXT NOT NULL CHECK (outcome <> ''),
                needs_human TEXT NOT NULL,
                evidence TEXT NOT NULL,
                satisfied TEXT NOT NULL,
                waived TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                CHECK (kind = 'completed' OR (satisfied = '' AND waived = ''))
            );
            CREATE INDEX workflows_handoffs_by_run ON workflows_handoffs (run, id);",
    }],
};
