//! Gate outcomes: which receipt applies to an exact result, and what the required checks add
//! up to. Freshness is computed when asked, never stored, so a receipt cannot be marked
//! current after its artifact changed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::artifacts::file_digest;
use crate::error::invalid;
use crate::{
    CheckDefinition, CheckReport, CheckStatus, Claim, GateOutcome, GateReport, Outcome, Receipt,
    Refusal, Source, StaleReason,
};

/// The receipts and claims of some checks of one project, as [`crate::evidence`] loaded them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub(crate) receipts: BTreeMap<String, Vec<Receipt>>,
    pub(crate) claims: BTreeMap<String, Vec<Claim>>,
}

impl Evidence {
    /// The check's receipts, newest first; at most [`crate::RECEIPTS_LOADED_PER_CHECK`].
    pub fn receipts(&self, check_name: &str) -> &[Receipt] {
        self.receipts.get(check_name).map_or(&[], Vec::as_slice)
    }

    /// The check's claims, newest first.
    pub fn claims(&self, check_name: &str) -> &[Claim] {
        self.claims.get(check_name).map_or(&[], Vec::as_slice)
    }

    /// The outcome of a gate requiring `checks` on the exact result `source` in `checkout`.
    /// Reads the receipts' artifacts from the checkout, so call it outside a transaction.
    pub fn evaluate(
        &self,
        checks: &[CheckDefinition],
        source: &Source,
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
        let reports: Vec<CheckReport> = checks
            .iter()
            .map(|check| self.report(check, source, checkout))
            .collect();
        Ok(GateReport {
            outcome: gate_outcome(&reports),
            checks: reports,
        })
    }

    fn report(&self, check: &CheckDefinition, source: &Source, checkout: &Path) -> CheckReport {
        let receipts = self.receipts(&check.name);
        let mut newest_stale = None;
        for receipt in receipts {
            match staleness(receipt, check, source, checkout) {
                None => {
                    return CheckReport {
                        name: check.name.clone(),
                        status: CheckStatus::Checked(receipt.recorded.outcome),
                        receipt: Some(receipt.id),
                    };
                }
                Some(reason) => {
                    newest_stale.get_or_insert(reason);
                }
            }
        }
        let status = match newest_stale {
            Some(reason) => CheckStatus::Stale(reason),
            None if self.claims(&check.name).is_empty() => CheckStatus::Missing,
            None => CheckStatus::Unverified,
        };
        CheckReport {
            name: check.name.clone(),
            status,
            receipt: None,
        }
    }
}

/// Why `receipt` does not apply to `check` on `source`, `None` when it does.
fn staleness(
    receipt: &Receipt,
    check: &CheckDefinition,
    source: &Source,
    checkout: &Path,
) -> Option<StaleReason> {
    let recorded = &receipt.recorded;
    if recorded.check != *check {
        return Some(StaleReason::CheckChanged);
    }
    if recorded.source.head != source.head || recorded.source.dirty != source.dirty {
        return Some(StaleReason::SourceChanged {
            recorded: recorded.source.clone(),
        });
    }
    recorded.artifacts.iter().find_map(|artifact| {
        let path = artifact.path.clone();
        match file_digest(checkout, &artifact.path) {
            Ok(Some(digest)) if digest == artifact.digest => None,
            Ok(Some(_)) => Some(StaleReason::ArtifactChanged { path }),
            Ok(None) => Some(StaleReason::ArtifactMissing { path }),
            Err(reason) => Some(StaleReason::ArtifactUnreadable { path, reason }),
        }
    })
}

fn gate_outcome(reports: &[CheckReport]) -> GateOutcome {
    let has = |outcome| {
        reports
            .iter()
            .any(|report| report.status == CheckStatus::Checked(outcome))
    };
    if has(Outcome::Failed) {
        return GateOutcome::Failed;
    }
    let all_passed = reports
        .iter()
        .all(|report| report.status == CheckStatus::Checked(Outcome::Passed));
    if all_passed {
        GateOutcome::Passed
    } else {
        GateOutcome::Open
    }
}
