//! Gate outcomes: which receipt applies to an exact result, and what the required checks add
//! up to. Freshness is computed when asked, never stored, so a receipt cannot be marked
//! current after its artifact changed.

use std::collections::HashMap;
use std::path::Path;

use crate::artifacts::Checkout;
use crate::{
    CheckDefinition, CheckReport, CheckStatus, Claim, GateOutcome, GateReport, Outcome, Receipt,
    Refusal, Source, StaleReason,
};

/// What a project holds about a gate's required checks on one exact source, as
/// [`crate::evidence`] loaded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub(crate) source: Source,
    /// In the order the gate requires the checks.
    pub(crate) checks: Vec<CheckEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckEvidence {
    pub(crate) check: CheckDefinition,
    /// The newest receipt of exactly this definition on exactly this source.
    pub(crate) receipt: Option<Receipt>,
    /// The source of the newest receipt of this check that is not such a match: it
    /// explains why the check is stale when no receipt matches.
    pub(crate) other_source: Option<Source>,
    /// Claims about this source, newest first.
    pub(crate) claims: Vec<Claim>,
}

/// Each artifact's digest, read at most once per evaluation.
type DigestCache = HashMap<String, Result<Option<String>, String>>;

impl Evidence {
    /// The newest receipt of the check's exact definition on this source.
    pub fn receipt(&self, check_name: &str) -> Option<&Receipt> {
        self.check(check_name)?.receipt.as_ref()
    }

    /// The check's claims about this source, newest first; at most
    /// [`crate::CLAIMS_LOADED_PER_CHECK`].
    pub fn claims(&self, check_name: &str) -> &[Claim] {
        self.check(check_name)
            .map_or(&[], |evidence| evidence.claims.as_slice())
    }

    fn check(&self, check_name: &str) -> Option<&CheckEvidence> {
        self.checks
            .iter()
            .find(|evidence| evidence.check.name == check_name)
    }

    /// The gate's outcome in `checkout`. Reads the receipts' artifacts from the checkout, so
    /// call it outside a transaction.
    pub fn evaluate(&self, checkout: &Path) -> Result<GateReport, Refusal> {
        let checkout = Checkout::open(checkout)?;
        let mut digests = DigestCache::new();
        let reports: Vec<CheckReport> = self
            .checks
            .iter()
            .map(|evidence| {
                let status = self.status(evidence, &checkout, &mut digests);
                let receipt = match status {
                    CheckStatus::Checked(_) => evidence.receipt.as_ref().map(|receipt| receipt.id),
                    _ => None,
                };
                CheckReport {
                    name: evidence.check.name.clone(),
                    status,
                    receipt,
                }
            })
            .collect();
        Ok(GateReport {
            outcome: gate_outcome(&reports),
            checks: reports,
        })
    }

    /// The newest exact receipt decides: once its artifacts changed, an older receipt of the
    /// same source and check does not stand in for it.
    fn status(
        &self,
        evidence: &CheckEvidence,
        checkout: &Checkout,
        digests: &mut DigestCache,
    ) -> CheckStatus {
        if let Some(receipt) = &evidence.receipt {
            return match changed_artifact(receipt, checkout, digests) {
                None => CheckStatus::Checked(receipt.recorded.outcome),
                Some(reason) => CheckStatus::Stale(reason),
            };
        }
        if let Some(other) = &evidence.other_source {
            let is_same_source = other.head == self.source.head && other.dirty == self.source.dirty;
            return CheckStatus::Stale(if is_same_source {
                StaleReason::CheckChanged
            } else {
                StaleReason::SourceChanged {
                    recorded: other.clone(),
                }
            });
        }
        if evidence.claims.is_empty() {
            CheckStatus::Missing
        } else {
            CheckStatus::Unverified
        }
    }
}

/// Why `receipt`'s artifacts are no longer what it checked, `None` when every one is.
fn changed_artifact(
    receipt: &Receipt,
    checkout: &Checkout,
    digests: &mut DigestCache,
) -> Option<StaleReason> {
    receipt.recorded.artifacts.iter().find_map(|artifact| {
        if !digests.contains_key(&artifact.path) {
            digests.insert(artifact.path.clone(), checkout.digest(&artifact.path));
        }
        let current = &digests[&artifact.path];
        let path = || artifact.path.clone();
        match current {
            Ok(Some(digest)) if *digest == artifact.digest => None,
            Ok(Some(_)) => Some(StaleReason::ArtifactChanged { path: path() }),
            Ok(None) => Some(StaleReason::ArtifactMissing { path: path() }),
            Err(reason) => Some(StaleReason::ArtifactUnreadable {
                path: path(),
                reason: reason.clone(),
            }),
        }
    })
}

fn gate_outcome(reports: &[CheckReport]) -> GateOutcome {
    let is = |report: &CheckReport, outcome| report.status == CheckStatus::Checked(outcome);
    if reports.iter().any(|report| is(report, Outcome::Failed)) {
        GateOutcome::Failed
    } else if reports.iter().all(|report| is(report, Outcome::Passed)) {
        GateOutcome::Passed
    } else {
        GateOutcome::Open
    }
}
