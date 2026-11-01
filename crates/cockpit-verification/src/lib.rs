#![doc = include_str!("../README.md")]

mod artifacts;
mod error;
mod gate;
mod model;
mod receipts;

use cockpit_turso::{Migration, MigrationSet};

pub use artifacts::fingerprint;
pub use error::{Error, Refusal};
pub use gate::Evidence;
pub use model::{
    Artifact, CheckDefinition, CheckReport, CheckStatus, Claim, ClaimId, GateOutcome, GateReport,
    NewClaim, NewReceipt, Outcome, Receipt, ReceiptId, Source, StaleReason,
};
pub use receipts::{
    ARTIFACTS_MAX, COMMAND_MAX_CHARS, NAME_MAX_CHARS, OUTPUT_MAX_BYTES, OUTPUT_REFERENCES_MAX,
    RECEIPTS_LOADED_PER_CHECK, STATEMENT_MAX_CHARS, evidence, record_claim, record_receipt,
};

/// The verification tables. Receipts and claims are append-only: evidence is never edited,
/// and whether a receipt still applies is decided when a gate is evaluated.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "verification",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE verification_receipts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project TEXT NOT NULL CHECK (project <> ''),
                run_id TEXT NOT NULL CHECK (run_id <> ''),
                check_name TEXT NOT NULL CHECK (check_name <> ''),
                command TEXT NOT NULL CHECK (command <> ''),
                working_directory TEXT NOT NULL CHECK (working_directory <> ''),
                environment TEXT NOT NULL CHECK (environment <> ''),
                head TEXT NOT NULL CHECK (head <> ''),
                base TEXT CHECK (base <> ''),
                dirty TEXT CHECK (dirty <> ''),
                outcome TEXT NOT NULL CHECK (outcome IN ('passed', 'failed', 'unavailable',
                    'blocked', 'cancelled', 'unknown')),
                exit_code INTEGER,
                output TEXT NOT NULL,
                output_bytes INTEGER NOT NULL CHECK (output_bytes >= 0),
                started_at INTEGER NOT NULL,
                finished_at INTEGER NOT NULL CHECK (finished_at >= started_at),
                recorded_at INTEGER NOT NULL,
                CHECK (outcome <> 'passed' OR exit_code = 0),
                CHECK (outcome <> 'failed' OR exit_code IS NULL OR exit_code <> 0),
                UNIQUE (project, run_id)
            );
            CREATE INDEX verification_receipts_by_check
                ON verification_receipts (project, check_name, id);
            CREATE TABLE verification_artifacts (
                receipt INTEGER NOT NULL REFERENCES verification_receipts (id),
                path TEXT NOT NULL CHECK (path <> ''),
                digest TEXT NOT NULL CHECK (length(digest) = 64),
                PRIMARY KEY (receipt, path)
            );
            CREATE TABLE verification_output_references (
                receipt INTEGER NOT NULL REFERENCES verification_receipts (id),
                position INTEGER NOT NULL CHECK (position >= 0),
                reference TEXT NOT NULL CHECK (reference <> ''),
                PRIMARY KEY (receipt, position)
            );
            CREATE TABLE verification_claims (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project TEXT NOT NULL CHECK (project <> ''),
                check_name TEXT NOT NULL CHECK (check_name <> ''),
                claimant TEXT NOT NULL CHECK (claimant <> ''),
                statement TEXT NOT NULL CHECK (statement <> ''),
                claimed_at INTEGER NOT NULL
            );
            CREATE INDEX verification_claims_by_check
                ON verification_claims (project, check_name, id);",
    }],
};
