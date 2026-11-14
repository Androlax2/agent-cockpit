#![doc = include_str!("../README.md")]

mod display;
mod egress;
mod error;
mod guides;
mod hints;
mod limit_hits;
mod pty;
mod redact;
mod restart;
mod retention;
mod rows;
mod rule_scan;
mod rules;
mod runs;
mod shell;
mod sudo;
mod sudo_editable;
mod sudo_unasked;
mod typed_secret;

use cockpit_turso::{Migration, MigrationSet};

pub use display::{FOLD_CHARS, FOLD_LINES, Folded, Hidden, Shown, fold, is_outside_project, show};
pub use egress::EgressProxy;
pub use error::{Error, Refusal, UnblockedBy};
pub use guides::{
    Guide, GuideId, GuideRequest, GuideState, GuideStep, Step, answer_step, ask_guide, close_guide,
    guide_request, guide_status, guide_status_if_fresh, open_guides, parse_guide, report_stuck,
    shown_guide,
};
pub use hints::{build_files_run, timeout_without_foreground};
pub use limit_hits::limit_hits;
pub use pty::{
    NoResult, Output, PROMPT_MAX_CHARS, Ran, RunConfinement, RunSettings, Runner, Syntax, present,
};
pub use redact::{redact_secrets, strip_terminal_controls};
pub use restart::ended_after_restart;
pub use retention::{RunOutputs, retirable_run_outputs, retire_run_outputs};
pub use rows::{EXPIRES_AFTER_SECONDS, Project, REFRESH_AFTER_SECONDS};
pub use rule_scan::{Finding, path_from, scan, scan_unfinished, sudo_findings};
pub use rules::Severity;
pub use runs::{
    APPROVAL_GRACE_SECONDS, Approved, DecidedThrough, FiledRun, Interrupted, REACH_MAX_ENTRIES,
    Reach, RunAsk, RunDecision, RunId, RunRequest, RunResult, RunState, StartAsk, StartSession,
    action_digest, approve_run, ask_run, ask_run_unless_pending, ask_start, decided_runs,
    decline_run, edit_run, finish_run, finish_start, interrupted_runs, may_ask, open_runs,
    purge_run_output, queue_order, reconcile_run, record_checks, record_not_run,
    record_outcome_unknown, resolve_cwd, resolve_write, run_request, run_status,
    run_status_if_fresh, shown_run, start_run, validate_start,
};
pub use sudo::{
    SudoAnswered, SudoAsk, SudoAskpass, SudoHelper, TRUSTED_SUDO, sudo_asks_every_time,
};
pub use sudo_editable::{
    Contents, FILE_HEAD_LINES, FileNow, Lookup, RootFile, RootRuns, SessionWrites, changed_file,
    digest_as_read, file_now, file_sha256, root_runs,
};
pub use sudo_unasked::unasked_sudo_watch_holds;
pub use typed_secret::{wipe, wipe_vec};

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
        // #285: the `run-commands` grant a request was asked under; the owner may decide it
        // only while that same grant still covers the session. Earlier rows keep NULL and
        // read as asked by a session that may no longer ask.
        Migration {
            version: 4,
            sql: "ALTER TABLE approvals_runs ADD COLUMN asking_grant INTEGER;
            ALTER TABLE approvals_guides ADD COLUMN asking_grant INTEGER;",
        },
        // #410: a coordinator's request to start a session waits in the same queue; the
        // daemon starts it once approved instead of running its command, which describes it.
        Migration {
            version: 5,
            sql: "ALTER TABLE approvals_runs ADD COLUMN kind TEXT NOT NULL DEFAULT 'command'
                CHECK (kind IN ('command', 'start-session'));
            ALTER TABLE approvals_runs ADD COLUMN start_prompt TEXT;
            ALTER TABLE approvals_runs ADD COLUMN start_clone TEXT CHECK (start_clone <> '');",
        },
        // A start names its agent once the daemon wires more than one (#453, #469).
        Migration {
            version: 6,
            sql: "ALTER TABLE approvals_runs ADD COLUMN start_agent TEXT CHECK (start_agent <> '');",
        },
        // #657: the owner client a decision came through. Earlier decisions keep NULL.
        Migration {
            version: 7,
            sql: "ALTER TABLE approvals_runs ADD COLUMN decided_through TEXT
                CHECK (decided_through IN ('tui', 'cli'));",
        },
        // #701: a start chooses its model and effort.
        Migration {
            version: 8,
            sql: "ALTER TABLE approvals_runs ADD COLUMN start_model TEXT CHECK (start_model <> '');
            ALTER TABLE approvals_runs ADD COLUMN start_effort TEXT
                CHECK (start_effort <> '');",
        },
        // #406: when retention cleared a finished command's output; the rest of the row,
        // its decision included, stays as the audit record.
        Migration {
            version: 9,
            sql: "ALTER TABLE approvals_runs ADD COLUMN output_retired_at INTEGER
                CHECK (output_retired_at IS NULL OR (state = 'finished' AND output = ''));",
        },
        // #1301: when the owner stopped a running command, which then finished with the exit
        // status the stop left. Earlier rows keep NULL.
        Migration {
            version: 10,
            sql: "ALTER TABLE approvals_runs ADD COLUMN stopped_by_owner_at INTEGER
                CHECK (stopped_by_owner_at IS NULL OR state = 'finished');",
        },
        // #1374: how many sudo processes in a finished command ended having asked the owner
        // nothing ("sudo ran without asking you"). NULL when none, as on earlier rows.
        Migration {
            version: 11,
            sql: "ALTER TABLE approvals_runs ADD COLUMN sudo_ran_unasked INTEGER
                CHECK (sudo_ran_unasked IS NULL OR (sudo_ran_unasked > 0 AND state = 'finished'));",
        },
        // #1386: how many times a secret the owner typed for a finished command appeared in
        // its output and was redacted. NULL when none, as on earlier rows.
        Migration {
            version: 12,
            sql: "ALTER TABLE approvals_runs ADD COLUMN typed_secret_redacted INTEGER
                CHECK (typed_secret_redacted IS NULL
                    OR (typed_secret_redacted > 0 AND state = 'finished'));",
        },
        // #1383: what the revision's sudo runs that its asking session can change, each file
        // with its hash, or that it cannot be told (`sudo_editable::encode`). NULL when not
        // checked: a start request or an earlier row.
        Migration {
            version: 13,
            sql: "ALTER TABLE approvals_runs ADD COLUMN root_runs TEXT
                CHECK (root_runs IS NULL OR substr(root_runs, 1, 3) = 'v1 ');",
        },
        // #1388: what the rule scan found in the revision, one finding a line
        // (`rule_scan::encode`), kept with the decision. NULL when not scanned: a start
        // request or an earlier row.
        Migration {
            version: 14,
            sql: "ALTER TABLE approvals_runs ADD COLUMN findings TEXT
                CHECK (findings IS NULL OR substr(findings, 1, 2) = 'v1');",
        },
        // #1387: the folders a confined run's card names to write beyond its checkout and the
        // hosts it reaches, one per line, NULL when none; and the limits a finished run hit,
        // one per line, NULL when none.
        Migration {
            version: 15,
            sql: "ALTER TABLE approvals_runs ADD COLUMN card_writes TEXT
                CHECK (card_writes IS NULL OR length(card_writes) > 0);
                ALTER TABLE approvals_runs ADD COLUMN card_hosts TEXT
                CHECK (card_hosts IS NULL OR length(card_hosts) > 0);
                ALTER TABLE approvals_runs ADD COLUMN limit_hits TEXT
                CHECK (limit_hits IS NULL OR (length(limit_hits) > 0 AND state = 'finished'));",
        },
    ],
};
pub use runs::maintenance_runs;
