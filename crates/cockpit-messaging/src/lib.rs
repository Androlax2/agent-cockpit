#![doc = include_str!("../README.md")]

mod entities;
mod error;
mod feed;
mod outbox;
mod receipts;

use cockpit_turso::{Migration, MigrationSet};

pub use entities::{EntityChange, Published, publish, retract};
pub use error::{Error, Refusal, ResnapshotReason};
pub use feed::{Feed, FeedBatch, FeedCursor, FeedPosition, discard_history};
pub use outbox::{
    Claim, Coalescing, Deliverer, Delivery, DeliveryState, Enqueued, Envelope, EnvelopeId, Failure,
    Redelivery, Resolution, StoredEnvelope, claim_due, deliver_due, enqueue, envelope,
    reconcile_deliveries, record_delivery, resolve_uncertain, uncertain_envelopes,
};
pub use receipts::{ReceiptId, Received, receive};

/// The messaging tables. Envelope and entity rows are never deleted: a late receipt names
/// the envelope it belongs to, and an entity's tombstone keeps its revision increasing.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "messaging",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE messaging_envelopes (
                id INTEGER PRIMARY KEY,
                effect_key TEXT NOT NULL UNIQUE CHECK (effect_key <> ''),
                producer TEXT NOT NULL CHECK (producer <> ''),
                destination TEXT NOT NULL CHECK (destination <> ''),
                payload TEXT NOT NULL,
                redelivery TEXT NOT NULL CHECK (redelivery IN ('receiver-deduplicates',
                    'never')),
                replaceable_slot TEXT CHECK (replaceable_slot <> ''),
                state TEXT NOT NULL CHECK (state IN ('queued', 'submitting', 'received',
                    'uncertain', 'failed', 'expired', 'superseded')),
                attempts INTEGER NOT NULL CHECK (attempts >= 0),
                max_attempts INTEGER NOT NULL CHECK (max_attempts >= 1),
                due_at INTEGER NOT NULL,
                expires_at INTEGER,
                receipt TEXT CHECK (receipt <> ''),
                note TEXT,
                superseded_by INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                CHECK ((state = 'received') = (receipt IS NOT NULL)),
                CHECK ((state = 'superseded') = (superseded_by IS NOT NULL))
            );
            CREATE INDEX messaging_envelopes_due ON messaging_envelopes (due_at, id)
                WHERE state = 'queued';
            CREATE INDEX messaging_envelopes_slot
                ON messaging_envelopes (destination, replaceable_slot)
                WHERE state = 'queued' AND replaceable_slot IS NOT NULL;
            CREATE TABLE messaging_receipts (
                id INTEGER PRIMARY KEY,
                receiver TEXT NOT NULL CHECK (receiver <> ''),
                effect_key TEXT NOT NULL CHECK (effect_key <> ''),
                content TEXT NOT NULL,
                received_at INTEGER NOT NULL,
                UNIQUE (receiver, effect_key)
            );
            CREATE TABLE messaging_entities (
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL CHECK (kind <> ''),
                entity TEXT NOT NULL CHECK (entity <> ''),
                revision INTEGER NOT NULL CHECK (revision >= 1),
                state TEXT,
                UNIQUE (kind, entity)
            );
            CREATE TABLE messaging_feed_retention (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                retained_after INTEGER NOT NULL CHECK (retained_after >= 0)
            );",
    }],
};
