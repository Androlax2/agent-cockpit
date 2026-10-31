//! A scratch state database with the messaging tables.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::PathBuf;

use cockpit_messaging::{Coalescing, Envelope, Redelivery};
use cockpit_turso::Database;

/// Seconds since 1970, far from any expiry.
pub const NOW: i64 = 1_800_000_000;

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub database: Database,
}

impl Fixture {
    pub async fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(
            &dir.path().join("state.db"),
            &[cockpit_messaging::MIGRATIONS],
        )
        .await
        .unwrap();
        Fixture { dir, database }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.path().join("state.db")
    }
}

/// A decision for `destination`: never coalesced, never sent twice blindly.
pub fn envelope(effect_key: &str, destination: &str, payload: &str) -> Envelope {
    Envelope {
        effect_key: effect_key.to_owned(),
        producer: "teams".to_owned(),
        destination: destination.to_owned(),
        payload: payload.to_owned(),
        redelivery: Redelivery::Never,
        coalescing: Coalescing::Never,
        max_attempts: 3,
        expires_at: None,
    }
}
