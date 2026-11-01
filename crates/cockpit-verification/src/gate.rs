//! Gate outcomes: which receipt applies to an exact result, and what the required checks add
//! up to. Freshness is computed when asked, never stored, so a receipt cannot be marked
//! current after its artifact changed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::artifacts::Checkout;
use crate::error::invalid;
use crate::{
    CheckDefinition, CheckReport, CheckStatus, Claim, GateOutcome, GateReport, Outcome, Receipt,
    Refusal, Source, StaleReason,
};

/// The evidence of some checks of one project on one exact source, as [`crate::evidence`]
/// loaded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub(crate) source: Source,
    /// Per check: its receipts on this source, newest first.
    pub(crate) receipts: BTreeMap<String, Vec<Receipt>>,
    /// Per check: its newest receipt on any other source, which explains a stale check.
    pub(crate) newest_elsewhere: BTreeMap<String, Receipt>,
    /// Per check: its claims on this source, newest first.
    pub(crate) claims: BTreeMap<String, Vec<Claim>>,
}

/// Each artifact's digest, read at most once per evaluation.
type DigestCache = HashMap<String, Result<Option<String>, String>>;

impl Evidence {
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// The check's receipts on this source, newest first; at most
    /// [`crate::RECEIPTS_LOADED_PER_CHECK`].
    pub fn receipts(&self, check_name: &str) -> &[Receipt] {
        self.receipts.get(check_name).map_or(&[], Vec::as_slice)
    }

    /// The check's claims on this source, newest first; at most
    /// [`crate::CLAIMS_LOADED_PER_CHECK`].
    pub fn claims(&self, check_name: &str) -> &[Claim] {
        self.claims.get(check_name).map_or(&[], Vec::as_slice)
    }

    /// The outcome of a gate requiring `checks` on this evidence's source in `checkout`.
    /// Reads the receipts' artifacts from the checkout, so call it outside a transaction.
    pub fn evaluate(
        &self,
        checks: &[CheckDefinition],
        checkout: &Path,
    ) -> Result<GateReport, Refusal> {
        if checks.is_empty() {
            return Err(invalid("checks", "a gate requires at least one check"));
        }
        let mut seen = BTreeSet::new();
        for check in checks {
            if !seen.insert(check.name.as_str()) {
                return Err(invalid(
                    "checks",
                    format!("`{}` is required twice", check.name),
                ));
            }
            if !self.receipts.contains_key(&check.name) {
                return Err(invalid(
                    "checks",
                    format!("the evidence of `{}` was not loaded", check.name),
                ));
            }
        }
        let checkout = Checkout::open(checkout)?;
        let mut digests = DigestCache::new();
        let reports: Vec<CheckReport> = checks
            .iter()
            .map(|check| self.report(check, &checkout, &mut digests))
            .collect();
        Ok(GateReport {
            outcome: gate_outcome(&reports),
            checks: reports,
        })
    }

    /// The newest receipt of this exact check on this source decides: once its artifacts
    /// changed, an older receipt of the same source and check does not stand in for it.
    fn report(
        &self,
        check: &CheckDefinition,
        checkout: &Checkout,
        digests: &mut DigestCache,
    ) -> CheckReport {
        let receipts = self.receipts(&check.name);
        let (status, receipt) = match receipts
            .iter()
            .find(|receipt| receipt.recorded.check == *check)
        {
            Some(receipt) => match changed_artifact(receipt, checkout, digests) {
                None => (
                    CheckStatus::Checked(receipt.recorded.outcome),
                    Some(receipt.id),
                ),
                Some(reason) => (CheckStatus::Stale(reason), None),
            },
            None if !receipts.is_empty() => (CheckStatus::Stale(StaleReason::CheckChanged), None),
            None => match self.newest_elsewhere.get(&check.name) {
                Some(elsewhere) => (
                    CheckStatus::Stale(StaleReason::SourceChanged {
                        recorded: elsewhere.recorded.source.clone(),
                    }),
                    None,
                ),
                None if self.claims(&check.name).is_empty() => (CheckStatus::Missing, None),
                None => (CheckStatus::Unverified, None),
            },
        };
        CheckReport {
            name: check.name.clone(),
            status,
            receipt,
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
        let current = digests
            .entry(artifact.path.clone())
            .or_insert_with(|| checkout.digest(&artifact.path));
        let path = artifact.path.clone();
        match current {
            Ok(Some(digest)) if *digest == artifact.digest => None,
            Ok(Some(_)) => Some(StaleReason::ArtifactChanged { path }),
            Ok(None) => Some(StaleReason::ArtifactMissing { path }),
            Err(reason) => Some(StaleReason::ArtifactUnreadable {
                path,
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
