/// A check as the project declares it. A receipt applies only to the definition it ran:
/// changing any field makes the earlier receipts stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckDefinition {
    /// The name a gate requires it by, unique within a project.
    pub name: String,
    /// The command line the project's own runner (Cargo, Make, ...) executes.
    pub command: String,
    /// Relative to the checkout root; `.` is the root.
    pub working_directory: String,
    /// Where it runs: `native`, a sandbox profile, a container engine.
    pub environment: String,
}

impl CheckDefinition {
    /// Refused as recording a receipt of it would be, so a runner refuses a check before
    /// running it instead of after.
    pub fn validate(&self) -> Result<(), crate::Refusal> {
        crate::receipts::check_definition(self)
    }
}

/// The exact source a check ran on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The commit checked out.
    pub head: String,
    /// What the task's delta is measured from. Reported, not matched: the same head is the
    /// same code whatever it was compared against.
    pub base: Option<String>,
    /// A digest of the uncommitted changes, `None` for a clean checkout. The caller computes
    /// it; this crate runs no program.
    pub dirty: Option<String>,
}

/// A file the check covered and its SHA-256 when the run started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    /// Relative to the checkout root.
    pub path: String,
    /// Lower-case hex.
    pub digest: String,
}

/// What a run found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    /// What the check needs is not there (a tool, a service, a credential).
    Unavailable,
    /// Something outside the check stopped it (a refused authorization, no capacity).
    Blocked,
    Cancelled,
    /// The run ended without a trustworthy result (the runner lost it, the daemon restarted).
    Unknown,
}

impl Outcome {
    pub const ALL: [Outcome; 6] = [
        Outcome::Passed,
        Outcome::Failed,
        Outcome::Unavailable,
        Outcome::Blocked,
        Outcome::Cancelled,
        Outcome::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Failed => "failed",
            Outcome::Unavailable => "unavailable",
            Outcome::Blocked => "blocked",
            Outcome::Cancelled => "cancelled",
            Outcome::Unknown => "unknown",
        }
    }

    /// Only the exact name parses.
    pub fn parse(name: &str) -> Option<Outcome> {
        Outcome::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == name)
    }
}

/// One run of a check, as its runner reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReceipt {
    pub project: String,
    /// The runner's identity for this execution: the same run delivered twice is one receipt.
    pub run_id: String,
    pub check: CheckDefinition,
    pub source: Source,
    /// Fingerprinted with [`crate::fingerprint`] before the run started, so an edit during
    /// the run makes the receipt stale too.
    pub artifacts: Vec<Artifact>,
    pub outcome: Outcome,
    pub exit_code: Option<i64>,
    /// Recording keeps its last [`crate::OUTPUT_MAX_BYTES`].
    pub output: String,
    /// Where the full output and larger result artifacts are.
    pub output_references: Vec<String>,
    pub started_at: i64,
    /// When the result was known; the newest receipt is the latest to finish. Never later
    /// than the recording time.
    pub finished_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReceiptId(pub i64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub id: ReceiptId,
    /// The run as stored: artifacts ordered by path, output cut to its tail.
    pub recorded: NewReceipt,
    /// The size of the output the runner reported.
    pub output_bytes: u64,
    pub recorded_at: i64,
}

impl Receipt {
    pub fn is_output_truncated(&self) -> bool {
        self.output_bytes > self.recorded.output.len() as u64
    }
}

/// A statement that a check passed with no run behind it: kept so the next agent and the
/// human read it, never counted as evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewClaim {
    pub project: String,
    pub check_name: String,
    /// The result the claim is about: it shows only on that exact source.
    pub source: Source,
    /// Who said it, as the daemon identified them.
    pub claimant: String,
    pub statement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClaimId(pub i64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub id: ClaimId,
    pub check_name: String,
    pub source: Source,
    pub claimant: String,
    pub statement: String,
    pub claimed_at: i64,
}

/// Why a check's newest receipt does not apply to the result asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleReason {
    /// The check's command, working directory or environment is no longer the one that ran.
    CheckChanged,
    /// It ran on another head or other uncommitted changes.
    SourceChanged {
        recorded: Source,
    },
    ArtifactChanged {
        path: String,
    },
    ArtifactMissing {
        path: String,
    },
    /// The file can no longer be read as a file in the checkout.
    ArtifactUnreadable {
        path: String,
        reason: String,
    },
}

/// Where a required check stands on an exact result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckStatus {
    /// A receipt applies; its outcome.
    Checked(Outcome),
    /// Receipts exist, none applies; why not.
    Stale(StaleReason),
    /// No receipt on this source, only prose claims about it.
    Unverified,
    /// Neither a receipt nor a claim.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub name: String,
    pub status: CheckStatus,
    /// The newest receipt of exactly this definition on this source, stale or not: the
    /// evidence the status is about.
    pub receipt: Option<ReceiptId>,
}

impl CheckReport {
    /// What has to happen for the check to pass, `None` once it has (report 06: every
    /// check that holds a gate open names its next action).
    pub fn next_action(&self) -> Option<String> {
        let name = &self.name;
        let action = match &self.status {
            CheckStatus::Checked(Outcome::Passed) => return None,
            CheckStatus::Checked(Outcome::Failed) => {
                format!("fix what `{name}` reported and run it again")
            }
            CheckStatus::Checked(Outcome::Unavailable) => {
                format!("restore what `{name}` needs, then run it again")
            }
            CheckStatus::Checked(Outcome::Blocked) => {
                format!("lift what blocked `{name}`, then run it again")
            }
            CheckStatus::Checked(Outcome::Cancelled) => format!("run `{name}` again"),
            CheckStatus::Checked(Outcome::Unknown) => {
                format!("reconcile `{name}`: its result was lost, run it again")
            }
            CheckStatus::Stale(_) => format!("run `{name}` again on the current result"),
            CheckStatus::Unverified => {
                format!("run `{name}`: a claim that it passed is not evidence")
            }
            CheckStatus::Missing => format!("run `{name}`"),
        };
        Some(action)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateOutcome {
    /// Every required check passed on this exact result.
    Passed,
    /// A required check failed on this exact result.
    Failed,
    /// Nothing failed, but not every check passed: each names its next action.
    Open,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateReport {
    pub outcome: GateOutcome,
    /// In the order the gate required them.
    pub checks: Vec<CheckReport>,
}
