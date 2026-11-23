//! A default branch's protection (#695): the rules the forge enforces on it, which `torlek
//! doctor` reads with the project's API token, and Torlek's own rule, which `torlek project
//! protect` sets up with a token the owner types for that one call (Theo, 2026-11-23).

use std::fmt;

use crate::{BranchName, InvalidValue};

/// Longest required check name; a forge's check names are far shorter.
const MAX_CHECK_BYTES: usize = 255;

/// What `torlek project protect` makes the forge enforce on the default branch (Theo,
/// 2026-11-23): every change through a change request, with no approval required (a lone
/// owner cannot approve their own); each of the owner's named checks passing; no force push;
/// no deletion; and no one allowed to bypass any of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectionPolicy {
    required_checks: Vec<String>,
}

impl ProtectionPolicy {
    /// Refuses no check, a repeated one, and a name that is empty, longer than 255 bytes, or
    /// holds a control character or only spaces.
    pub fn new(required_checks: Vec<String>) -> Result<ProtectionPolicy, InvalidValue> {
        if required_checks.is_empty() {
            return Err(InvalidValue {
                kind: "required check list",
                value: String::new(),
                rule: "at least one check the default branch's changes must pass",
            });
        }
        for (index, check) in required_checks.iter().enumerate() {
            let is_plain = check.len() <= MAX_CHECK_BYTES
                && !check.trim().is_empty()
                && !check.chars().any(char::is_control);
            if !is_plain {
                return Err(InvalidValue {
                    kind: "required check name",
                    value: check.clone(),
                    rule: "1 to 255 bytes, not only spaces, with no control character",
                });
            }
            if required_checks[..index].contains(check) {
                return Err(InvalidValue {
                    kind: "required check list",
                    value: check.clone(),
                    rule: "each check named once",
                });
            }
        }
        Ok(ProtectionPolicy { required_checks })
    }

    pub fn required_checks(&self) -> &[String] {
        &self.required_checks
    }
}

/// What the forge enforces on a branch, from every rule that applies to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BranchRules {
    pub change_request_required: bool,
    /// Checks a change must pass, in the forge's order.
    pub required_checks: Vec<String>,
    pub force_push_blocked: bool,
    pub deletion_blocked: bool,
}

/// One way a branch falls short of being protected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gap {
    ChangeRequestNotRequired,
    /// No check is required at all.
    NoRequiredCheck,
    ForcePushAllowed,
    DeletionAllowed,
}

impl fmt::Display for Gap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Gap::ChangeRequestNotRequired => formatter.write_str("no change request required"),
            Gap::NoRequiredCheck => formatter.write_str("no check required"),
            Gap::ForcePushAllowed => formatter.write_str("force pushes allowed"),
            Gap::DeletionAllowed => formatter.write_str("deletion allowed"),
        }
    }
}

impl BranchRules {
    /// What keeps the branch from being protected at all: a change request, at least one
    /// check, no force push and no deletion. What `doctor` reports.
    pub fn gaps(&self) -> Vec<Gap> {
        [
            (!self.change_request_required).then_some(Gap::ChangeRequestNotRequired),
            self.required_checks
                .is_empty()
                .then_some(Gap::NoRequiredCheck),
            (!self.force_push_blocked).then_some(Gap::ForcePushAllowed),
            (!self.deletion_blocked).then_some(Gap::DeletionAllowed),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// A repository's default branch and what the forge enforces on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultBranch {
    pub branch: BranchName,
    pub rules: BranchRules,
    /// The forge says the branch is protected by something whose rules this read cannot
    /// see, such as an older kind of protection the token may not read: [`BranchRules`] may
    /// then be short of what is enforced.
    pub has_unread_protection: bool,
}

/// What [`crate::Forge::protect_default_branch`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectOutcome {
    /// Torlek's rule was made.
    Created,
    /// Torlek's rule existed and said something else; it was replaced.
    Updated,
    /// Torlek's rule already said the policy; nothing was written.
    Unchanged,
}

impl ProtectOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            ProtectOutcome::Created => "created",
            ProtectOutcome::Updated => "updated",
            ProtectOutcome::Unchanged => "unchanged",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Protected {
    pub branch: BranchName,
    pub outcome: ProtectOutcome,
}
