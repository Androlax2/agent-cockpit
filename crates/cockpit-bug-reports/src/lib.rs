#![doc = include_str!("../README.md")]

mod error;
mod issue;
mod model;
mod reports;
mod text;

use cockpit_turso::{Migration, MigrationSet};

pub use error::{Error, Refusal};
pub use issue::{FIELDS, Issue, REPOSITORY, TEMPLATE, URL_MAX_BYTES, issue, search_query};
pub use model::{
    Bundle, Edit, Facts, Filed, Fingerprint, Kind, NewReport, Origin, Report, ReportId, Source,
    Status, UNKNOWN_COMPONENT,
};
pub use reports::{
    SESSION_DRAFTS_PER_DAY, attach_bundle, discard, drafts_waiting, edit, file, list, mark_sent,
    report,
};
pub use text::{COMPONENT_MAX_CHARS, DESCRIPTION_MAX_CHARS, STEPS_MAX_CHARS, TITLE_MAX_CHARS};

/// The bug reports table. A row is never removed: a sent or discarded report is what keeps
/// its fingerprint from opening another draft the same day.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "bug_reports",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE bug_reports (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            origin TEXT NOT NULL CHECK (origin IN ('owner', 'agent', 'daemon')),
            requested_by_owner INTEGER NOT NULL CHECK (requested_by_owner IN (0, 1)),
            session TEXT CHECK (session <> ''),
            is_limited INTEGER NOT NULL CHECK (is_limited IN (0, 1)),
            status TEXT NOT NULL CHECK (status IN ('draft', 'sent', 'discarded')),
            title TEXT NOT NULL CHECK (title <> ''),
            description TEXT NOT NULL CHECK (description <> ''),
            steps TEXT NOT NULL,
            version TEXT NOT NULL CHECK (version <> ''),
            kind TEXT NOT NULL CHECK (kind IN ('timeout', 'unreachable', 'internal', 'panic',
                'stall', 'crash', 'other')),
            component TEXT NOT NULL CHECK (component <> ''),
            environment TEXT NOT NULL,
            bundle_path TEXT CHECK (bundle_path <> ''),
            bundle_contents TEXT,
            bundle_problem TEXT CHECK (bundle_problem <> ''),
            occurrences INTEGER NOT NULL CHECK (occurrences >= 1),
            issue_url TEXT CHECK (issue_url <> ''),
            day INTEGER NOT NULL,
            created_at INTEGER NOT NULL,
            changed_at INTEGER NOT NULL,
            CHECK ((origin = 'agent') = (session IS NOT NULL)),
            CHECK (origin = 'agent' OR requested_by_owner = 0),
            CHECK (origin <> 'owner' OR is_limited = 0),
            CHECK ((bundle_path IS NULL) = (bundle_contents IS NULL)),
            CHECK (bundle_path IS NULL OR bundle_problem IS NULL),
            CHECK (status = 'sent' OR issue_url IS NULL)
        );
        CREATE INDEX bug_reports_by_fingerprint ON bug_reports (version, kind, component, day)
            WHERE is_limited = 1;
        CREATE INDEX bug_reports_by_session ON bug_reports (session, day)
            WHERE session IS NOT NULL;
        CREATE INDEX bug_reports_drafts ON bug_reports (status) WHERE status = 'draft';",
    }],
};
