#![doc = include_str!("../README.md")]

mod display;
mod error;
mod guides;
mod hints;
mod pty;
mod redact;
mod restart;
mod rows;
mod runs;

use cockpit_turso::{Migration, MigrationSet};

pub use display::{FOLD_CHARS, FOLD_LINES, Folded, Hidden, Shown, fold, is_outside_project, show};
pub use error::{Error, Refusal, UnblockedBy};
pub use guides::{
    Guide, GuideId, GuideRequest, GuideState, GuideStep, Step, answer_step, ask_guide, close_guide,
    guide_request, guide_status, guide_status_if_fresh, open_guides, parse_guide, report_stuck,
};
pub use hints::{build_files_run, timeout_without_foreground};
pub use pty::{NoResult, Output, Ran, RunSettings, Runner, present};
pub use redact::{redact_secrets, strip_terminal_controls};
pub use restart::ended_after_restart;
pub use rows::{EXPIRES_AFTER_SECONDS, Project, REFRESH_AFTER_SECONDS};
pub use runs::{
    APPROVAL_GRACE_SECONDS, Approved, Interrupted, RunAsk, RunId, RunRequest, RunResult, RunState,
    action_digest, approve_run, ask_run, decline_run, edit_run, finish_run, interrupted_runs,
    may_ask, open_runs, queue_order, reconcile_run, record_not_run, record_outcome_unknown,
    resolve_cwd, run_request, run_status, run_status_if_fresh, start_run,
};

/// The approvals tables. A run request keeps its asked command beside the current one, so an
/// owner's edit stays visible; its decision columns are set once, by the transaction that
/// leaves `asking`. Rows are never deleted: a late result names the request it belongs to.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "approvals",
    migrations: &[
        Migration {
            version: 1,
            sql: "CREATE TABLE approvals_runs (
                id INTEGER PRIMARY KEY,
                session TEXT NOT NULL CHECK (session <> ''),
                revision INTEGER NOT NULL CHECK (revision >= 1),
                command TEXT NOT NULL CHECK (command <> ''),
                asked_command TEXT NOT NULL CHECK (asked_command <> ''),
                cwd TEXT NOT NULL CHECK (substr(cwd, 1, 1) = '/'),
                project TEXT CHECK (project <> ''),
                project_root TEXT CHECK (substr(project_root, 1, 1) = '/'),
                reason TEXT NOT NULL CHECK (reason <> ''),
                state TEXT NOT NULL CHECK (state IN ('asking', 'declined', 'approved',
                    'running', 'finished', 'not-run', 'outcome-unknown')),
                asked_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                decided_at INTEGER,
                approved_digest TEXT CHECK (length(approved_digest) = 64),
                exit_code INTEGER,
                output TEXT,
                output_omitted_lines INTEGER CHECK (output_omitted_lines >= 0),
                output_truncated_bytes INTEGER CHECK (output_truncated_bytes >= 0),
                redacted INTEGER CHECK (redacted >= 0),
                note TEXT,
                ended_at INTEGER,
                CHECK ((project IS NULL) = (project_root IS NULL)),
                CHECK ((state = 'asking') = (decided_at IS NULL)),
                CHECK ((state IN ('asking', 'declined')) = (approved_digest IS NULL)),
                CHECK ((state = 'finished') = (exit_code IS NOT NULL)),
                CHECK ((state = 'finished') = (output IS NOT NULL)),
                CHECK ((state IN ('not-run', 'outcome-unknown')) = (note IS NOT NULL)),
                CHECK ((state IN ('finished', 'not-run', 'outcome-unknown'))
                    = (ended_at IS NOT NULL))
            );
            CREATE INDEX approvals_runs_open ON approvals_runs (asked_at)
                WHERE state IN ('asking', 'approved', 'running');
            CREATE TABLE approvals_guides (
                id INTEGER PRIMARY KEY,
                session TEXT NOT NULL CHECK (session <> ''),
                title TEXT NOT NULL CHECK (title <> ''),
                cwd TEXT NOT NULL CHECK (substr(cwd, 1, 1) = '/'),
                project TEXT CHECK (project <> ''),
                project_root TEXT CHECK (substr(project_root, 1, 1) = '/'),
                state TEXT NOT NULL CHECK (state IN ('asking', 'done', 'stuck', 'closed')),
                current_step INTEGER NOT NULL CHECK (current_step >= 0),
                step_count INTEGER NOT NULL CHECK (step_count >= 1),
                stuck_note TEXT,
                asked_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                ended_at INTEGER,
                CHECK ((project IS NULL) = (project_root IS NULL)),
                CHECK (current_step <= step_count),
                CHECK ((state = 'done') = (current_step = step_count)),
                CHECK ((state = 'stuck') = (stuck_note IS NOT NULL)),
                CHECK ((state = 'asking') = (ended_at IS NULL))
            );
            CREATE TABLE approvals_guide_steps (
                guide INTEGER NOT NULL REFERENCES approvals_guides (id),
                position INTEGER NOT NULL CHECK (position >= 0),
                section TEXT,
                text TEXT NOT NULL CHECK (text <> ''),
                open TEXT NOT NULL,
                copy TEXT NOT NULL,
                ask TEXT,
                run TEXT,
                answer TEXT,
                run_request INTEGER REFERENCES approvals_runs (id),
                answered_at INTEGER,
                PRIMARY KEY (guide, position)
            );",
        },
        Migration {
            version: 2,
            // #156: the grace after a revision appears needs its time; an edit sets it again.
            sql: "ALTER TABLE approvals_runs ADD COLUMN revised_at INTEGER NOT NULL DEFAULT 0;
            UPDATE approvals_runs SET revised_at = asked_at;",
        },
        // #163: a running run names the `cockpit-processes` execution it reserved, so a
        // restart reports what became of it. Earlier rows keep NULL.
        Migration {
            version: 3,
            sql: "ALTER TABLE approvals_runs ADD COLUMN execution INTEGER
                CHECK (execution >= 1);",
        },
    ],
};
