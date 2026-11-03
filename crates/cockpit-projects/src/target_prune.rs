//! Owner-authorized cleanup of literal clone targets, separate from whole-clone sweep.
mod filesystem;
mod manifest;
mod mount;
mod quarantine;
mod store;
mod worker;

use crate::{Checkout, CheckoutId, Error, Git, TargetSize};
use cockpit_turso::{Database, WriteTransaction};

pub(crate) use store::MIGRATION;
pub use store::{
    paused_automatic_prunes, prune_status, require_checkout_available,
    require_execution_start_allowed, require_working_directory_available,
};
pub use worker::{
    PruneStep, PruneStepReceipt, abandon_target_prune, begin_prune_step, complete_prune_step,
    pause_target_prune, recover_target_prunes, resume_target_prune,
};

/// Owner policy. Filesystem output is disposable only under the explicit owner contract;
/// this does not protect against an unmanaged concurrent mutator with the same rights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunePolicy {
    pub automatic: bool,
    pub projects: Vec<String>,
    pub confirm_above_bytes: u64,
    pub interval_minutes: u32,
    pub base_ref: Option<String>,
}

impl Default for PrunePolicy {
    fn default() -> Self {
        Self {
            automatic: false,
            projects: Vec::new(),
            confirm_above_bytes: 1024 * 1024 * 1024,
            interval_minutes: 60,
            base_ref: None,
        }
    }
}

/// Capability-owned observations composed by the daemon on one transaction connection.
/// A session label is not termination evidence. Wire callers cannot supply this proof.
#[derive(Debug, Clone, Default)]
pub struct PruneEvidence {
    pub ended_sessions: Option<String>,
    pub busy: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PrunePreview {
    pub checkout: Checkout,
    pub measurement: TargetSize,
    pub eligibility: Option<String>,
    pub kept_reason: Option<String>,
    pub confirmation_required: bool,
    snapshot: Option<manifest::Snapshot>,
    policy: PrunePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneStatus {
    pub target: String,
    pub quarantine: Option<String>,
    pub epoch: String,
    pub operation: i64,
    pub revision: i64,
    pub checkout: CheckoutId,
    pub phase: String,
    pub apparent_bytes: u64,
    pub removed_apparent_bytes: u64,
    pub absent_unconfirmed_bytes: u64,
    pub eligibility: String,
    pub reason: Option<String>,
}

fn refused(reason: impl Into<String>) -> Error {
    Error::Refused {
        reason: reason.into(),
    }
}

/// Metadata/Git observation, always outside the writer and on bounded blocking work.
/// Missing/incomplete/unsupported measurements never create deletion authority.
pub async fn preview_target_prune(
    database: &Database,
    git: &Git,
    checkout: CheckoutId,
    policy: &PrunePolicy,
    evidence: &PruneEvidence,
) -> Result<PrunePreview, Error> {
    let (stored, main) = {
        let read = database.read().await?;
        let stored = crate::store::stored_checkout(&read, checkout).await?;
        let project = crate::store::project_named(&read, stored.checkout.project.as_str()).await?;
        let main = crate::store::main_checkout(&read, project.id).await?;
        (stored, main)
    };
    let policy = policy.clone();
    let evidence = evidence.clone();
    let git = git.clone();
    crate::blocking(move || Ok(filesystem::preview(stored, main, &git, policy, evidence))).await
}

/// Revalidate the observed registration and current capability evidence in the caller's
/// transaction before persisting a plan. The dry-run path never calls this function.
pub async fn prepare_target_prune(
    write: &WriteTransaction<'_>,
    epoch: &str,
    preview: &PrunePreview,
    evidence: &PruneEvidence,
) -> Result<PruneStatus, Error> {
    store::prepare(write, epoch, preview, evidence).await
}

/// Current capability evidence must come from this same write transaction, after awaits.
pub async fn reserve_target_prune(
    write: &WriteTransaction<'_>,
    epoch: &str,
    plan: i64,
    revision: i64,
    preview: &PrunePreview,
    evidence: &PruneEvidence,
    automatic: bool,
) -> Result<PruneStatus, Error> {
    store::reserve(write, epoch, plan, revision, preview, evidence, automatic).await
}
