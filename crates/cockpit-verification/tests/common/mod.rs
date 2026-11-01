//! A scratch state database with the verification tables and a checkout directory.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::PathBuf;

use cockpit_turso::Database;
use cockpit_verification::{
    CheckDefinition, Claim, Evidence, NewClaim, NewReceipt, Outcome, Receipt, Source,
};

pub const NOW: i64 = 1_800_000_000;
pub const PROJECT: &str = "cockpit";
pub const HEAD: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub database: Database,
}

impl Fixture {
    pub async fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("checkout")).unwrap();
        let database = Database::open(
            &dir.path().join("state.db"),
            &[cockpit_verification::MIGRATIONS],
        )
        .await
        .unwrap();
        Fixture { dir, database }
    }

    pub fn checkout(&self) -> PathBuf {
        self.dir.path().join("checkout")
    }

    pub fn write_file(&self, path: &str, content: &str) {
        let full = self.checkout().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }

    pub async fn record(&self, new: NewReceipt) -> Result<Receipt, cockpit_verification::Error> {
        let write = self.database.write().await.unwrap();
        let recorded = cockpit_verification::record_receipt(&write, new, NOW).await;
        write.commit().await.unwrap();
        recorded
    }

    pub async fn claim(&self, new: NewClaim) -> Result<Claim, cockpit_verification::Error> {
        let write = self.database.write().await.unwrap();
        let claimed = cockpit_verification::record_claim(&write, new, NOW).await;
        write.commit().await.unwrap();
        claimed
    }

    pub async fn evidence(&self, check_names: &[&str]) -> Evidence {
        let read = self.database.read().await.unwrap();
        cockpit_verification::evidence(&read, PROJECT, check_names)
            .await
            .unwrap()
    }

    /// A receipt of `check` on `source` over the checkout's `artifacts`, fingerprinted now.
    pub fn receipt(
        &self,
        run_id: &str,
        check: &CheckDefinition,
        source: &Source,
        artifacts: &[&str],
        outcome: Outcome,
    ) -> NewReceipt {
        NewReceipt {
            project: PROJECT.to_owned(),
            run_id: run_id.to_owned(),
            check: check.clone(),
            source: source.clone(),
            artifacts: cockpit_verification::fingerprint(&self.checkout(), artifacts).unwrap(),
            outcome,
            exit_code: match outcome {
                Outcome::Passed => Some(0),
                Outcome::Failed => Some(1),
                _ => None,
            },
            output: format!("{} finished", check.name),
            output_references: Vec::new(),
            started_at: NOW - 30,
            finished_at: NOW - 1,
        }
    }
}

pub fn check(name: &str) -> CheckDefinition {
    CheckDefinition {
        name: name.to_owned(),
        command: format!("make {name}"),
        working_directory: ".".to_owned(),
        environment: "native".to_owned(),
    }
}

pub fn clean_source() -> Source {
    Source {
        head: HEAD.to_owned(),
        base: Some("2d890dc0000000000000000000000000000000000".to_owned()),
        dirty: None,
    }
}

pub fn claim(check_name: &str) -> NewClaim {
    NewClaim {
        project: PROJECT.to_owned(),
        check_name: check_name.to_owned(),
        claimant: "session-7".to_owned(),
        statement: "All tests pass.".to_owned(),
    }
}

/// The refusal inside `result`, failing the test on success or another error.
pub fn refusal<T: std::fmt::Debug>(
    result: Result<T, cockpit_verification::Error>,
) -> cockpit_verification::Refusal {
    match result {
        Err(cockpit_verification::Error::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}
