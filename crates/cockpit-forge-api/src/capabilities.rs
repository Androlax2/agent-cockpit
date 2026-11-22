//! What a forge adapter can do and how Torlek knows it (report 08, as for providers).

use std::collections::BTreeMap;
use std::fmt;

use crate::ForgeError;

/// One thing a forge adapter may be able to do. Callers key every behavior on these, never on
/// the forge's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// List a repository's issues ([`crate::Forge::list_issues`]).
    IssueList,
    /// Read one issue with its comments ([`crate::Forge::read_issue`]).
    IssueRead,
    /// File an issue ([`crate::Forge::create_issue`]).
    IssueCreate,
    /// Comment on an issue ([`crate::Forge::comment_on_issue`]).
    IssueComment,
    /// Read a change request's state, head and base, by id or by head branch
    /// ([`crate::Forge::change_request`], [`crate::Forge::change_request_for`]).
    ChangeState,
    /// Read the checks on a change request's head ([`crate::Forge::change_checks`]).
    ChangeChecks,
    /// Read a change request's review decision ([`crate::Forge::change_review`]).
    ChangeReview,
    /// Open a change request ([`crate::Forge::open_change_request`]).
    ChangeOpen,
    /// Check a repository token's scope ([`crate::Forge::check_token`]).
    TokenScope,
    /// Read a branch's protection. No contract method yet: it arrives with its first caller,
    /// `torlek project protect` and `doctor` (#695).
    ProtectionRead,
    /// Set up the default branch's protection (change request and checks required, no
    /// force). No contract method yet (#695).
    ProtectionBootstrap,
}

impl Capability {
    pub const ALL: [Capability; 11] = [
        Capability::IssueList,
        Capability::IssueRead,
        Capability::IssueCreate,
        Capability::IssueComment,
        Capability::ChangeState,
        Capability::ChangeChecks,
        Capability::ChangeReview,
        Capability::ChangeOpen,
        Capability::TokenScope,
        Capability::ProtectionRead,
        Capability::ProtectionBootstrap,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Capability::IssueList => "issue-list",
            Capability::IssueRead => "issue-read",
            Capability::IssueCreate => "issue-create",
            Capability::IssueComment => "issue-comment",
            Capability::ChangeState => "change-state",
            Capability::ChangeChecks => "change-checks",
            Capability::ChangeReview => "change-review",
            Capability::ChangeOpen => "change-open",
            Capability::TokenScope => "token-scope",
            Capability::ProtectionRead => "protection-read",
            Capability::ProtectionBootstrap => "protection-bootstrap",
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How the adapter knows it can do something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// The conformance suite passes against recorded answers of `against`, the program or API
    /// version the adapter talks to (`<program> 2.83.0`, `REST 2022-11-28`).
    Tested { against: String },
    /// Documented or detected, but no recorded answer covers it.
    Discovered,
}

/// What the owner's capability report shows for one capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available { against: String },
    Unverified,
    Unavailable,
}

/// The capabilities an adapter reports. One not reported is unavailable; one withheld is
/// unavailable with a reason the owner reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    reported: BTreeMap<Capability, Provenance>,
    withheld: BTreeMap<Capability, String>,
}

impl Capabilities {
    pub fn none() -> Capabilities {
        Capabilities::default()
    }

    /// Reports `capability`, replacing what was reported for it before.
    pub fn with(mut self, capability: Capability, provenance: Provenance) -> Capabilities {
        self.withheld.remove(&capability);
        self.reported.insert(capability, provenance);
        self
    }

    /// Does not report `capability` and says why, such as a program too old for it;
    /// [`Capabilities::require`] repeats the reason.
    pub fn withholding(mut self, capability: Capability, reason: String) -> Capabilities {
        self.reported.remove(&capability);
        self.withheld.insert(capability, reason);
        self
    }

    /// Why `capability` is not reported, when the adapter said.
    pub fn withheld(&self, capability: Capability) -> Option<&str> {
        self.withheld.get(&capability).map(String::as_str)
    }

    /// Whether the adapter reports `capability`, tested or not.
    pub fn supports(&self, capability: Capability) -> bool {
        self.reported.contains_key(&capability)
    }

    pub fn provenance(&self, capability: Capability) -> Option<&Provenance> {
        self.reported.get(&capability)
    }

    pub fn availability(&self, capability: Capability) -> Availability {
        match self.provenance(capability) {
            Some(Provenance::Tested { against }) => Availability::Available {
                against: against.clone(),
            },
            Some(Provenance::Discovered) => Availability::Unverified,
            None => Availability::Unavailable,
        }
    }

    /// The typed refusal for an operation that needs `capability`, when it is not reported;
    /// with the reason it is withheld, when there is one.
    pub fn require(&self, capability: Capability) -> Result<(), ForgeError> {
        if self.supports(capability) {
            return Ok(());
        }
        Err(match self.withheld(capability) {
            Some(reason) => ForgeError::Withheld {
                capability,
                reason: reason.to_owned(),
            },
            None => ForgeError::unsupported(capability),
        })
    }

    /// Every reported capability with its provenance, in [`Capability`] order.
    pub fn reported(&self) -> impl Iterator<Item = (Capability, &Provenance)> {
        self.reported
            .iter()
            .map(|(capability, provenance)| (*capability, provenance))
    }
}
