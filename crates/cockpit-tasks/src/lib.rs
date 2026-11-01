#![doc = include_str!("../README.md")]

mod drafts;
mod error;
mod images;
mod items;
mod model;
mod text;

use cockpit_turso::{Migration, MigrationSet};

pub use drafts::{discard_draft, drafts, save_draft};
pub use error::{Error, Refusal, UnblockedBy};
pub use images::{IMAGE_MAX_BYTES, IMAGES_PER_ITEM_MAX, Image, add_image, image};
pub use items::{
    ListFilter, add, approve, decline, delete, edit, finish, item, list, propose, reopen,
    report_blocker, session_work, set_priority, set_queued, start, unwait, wait,
};
pub use model::{
    Actor, Draft, DraftTarget, Edit, ImageInfo, Item, ItemId, Kind, NewItem, NextAction, Priority,
    SessionWork, Settings, Status, Wait,
};
pub use text::{NOTES_MAX_CHARS, SUMMARY_MAX_CHARS, TITLE_MAX_CHARS, WHY_MAX_CHARS};

/// The tasks tables. Items are never removed by a status change: a declined proposal stays
/// as the memory that stops it being proposed again, a done item keeps the summary the items
/// waiting on it read. Only `delete` removes a row, with its waits, images and draft.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "tasks",
    migrations: &[
        Migration {
            version: 1,
            sql: "CREATE TABLE tasks_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project TEXT NOT NULL CHECK (project <> ''),
                title TEXT NOT NULL CHECK (title <> ''),
                title_key TEXT NOT NULL CHECK (title_key <> ''),
                notes TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('task', 'feature', 'bug', 'improvement',
                    'chore', 'research')),
                priority INTEGER NOT NULL CHECK (priority >= 1 AND priority <= 6),
                status TEXT NOT NULL CHECK (status IN ('proposed', 'todo', 'doing', 'done',
                    'declined')),
                is_blocker_report INTEGER NOT NULL CHECK (is_blocker_report IN (0, 1)),
                why TEXT CHECK (why <> ''),
                filed_by TEXT CHECK (filed_by <> ''),
                filed_from TEXT CHECK (filed_from <> ''),
                doing_session TEXT CHECK (doing_session <> ''),
                summary TEXT CHECK (summary <> ''),
                is_queued INTEGER NOT NULL CHECK (is_queued IN (0, 1)),
                intent_revision INTEGER NOT NULL CHECK (intent_revision >= 1),
                state_revision INTEGER NOT NULL CHECK (state_revision >= 1),
                created_at INTEGER NOT NULL,
                changed_at INTEGER NOT NULL,
                CHECK (filed_from IS NULL OR filed_by IS NOT NULL),
                CHECK (status = 'doing' OR doing_session IS NULL),
                CHECK (status = 'done' OR summary IS NULL),
                CHECK (status = 'todo' OR is_queued = 0),
                CHECK (is_blocker_report = 0 OR why IS NOT NULL),
                CHECK (status NOT IN ('proposed', 'declined') OR filed_by IS NOT NULL)
            );
            CREATE INDEX tasks_items_by_project ON tasks_items (project, status);
            CREATE INDEX tasks_items_declined ON tasks_items (project, title_key)
                WHERE status = 'declined';
            CREATE TABLE tasks_waits (
                item INTEGER NOT NULL REFERENCES tasks_items (id),
                waits_on INTEGER NOT NULL REFERENCES tasks_items (id),
                CHECK (item <> waits_on),
                PRIMARY KEY (item, waits_on)
            );
            CREATE INDEX tasks_waits_on ON tasks_waits (waits_on);
            CREATE TABLE tasks_images (
                item INTEGER NOT NULL REFERENCES tasks_items (id),
                number INTEGER NOT NULL CHECK (number >= 1),
                media_type TEXT NOT NULL CHECK (media_type IN ('image/png', 'image/jpeg',
                    'image/gif', 'image/webp')),
                bytes BLOB NOT NULL,
                added_at INTEGER NOT NULL,
                PRIMARY KEY (item, number)
            );
            CREATE TABLE tasks_drafts (
                id INTEGER PRIMARY KEY,
                project TEXT CHECK (project <> ''),
                item INTEGER REFERENCES tasks_items (id),
                base_intent_revision INTEGER CHECK (base_intent_revision >= 1),
                title TEXT NOT NULL,
                notes TEXT NOT NULL,
                saved_at INTEGER NOT NULL,
                CHECK ((project IS NULL) <> (item IS NULL)),
                CHECK ((item IS NULL) = (base_intent_revision IS NULL))
            );",
        },
        // #157: the session that was doing an item when it was finished, so a session's
        // "finished" label reads task evidence (decided in #201). Items done before it
        // name none.
        Migration {
            version: 2,
            sql: "ALTER TABLE tasks_items ADD COLUMN finished_by TEXT CHECK (finished_by <> '');",
        },
        // #226: when an item was finished, so a session the owner relaunches is labelled
        // finished only by work done since. Items done before it have none: `changed_at`
        // moves on edits after the finish, so it could count an old finish as new.
        Migration {
            version: 3,
            sql: "ALTER TABLE tasks_items ADD COLUMN finished_at INTEGER;",
        },
        // #273: where a finish lies among the database's commits, so the label orders it
        // against a launch without the clock's seconds. Items done before it have none.
        Migration {
            version: 4,
            sql: "ALTER TABLE tasks_items ADD COLUMN finished_order INTEGER;",
        },
    ],
};
