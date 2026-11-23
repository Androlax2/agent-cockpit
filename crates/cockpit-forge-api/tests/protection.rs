//! #695: a default branch's protection through the fake forge. Protect makes Torlek's rule
//! once, a second run writes nothing, a change of checks either way replaces it, a branch
//! other rules protect still gets it, and a forge without the capability answers with the
//! typed refusal.

mod common;

use cockpit_forge_api::fake::FakeForge;
use cockpit_forge_api::{
    BranchRules, Capability, Forge, ForgeError, Gap, ProtectOutcome, ProtectionPolicy,
};
use common::branch;

fn policy(checks: &[&str]) -> ProtectionPolicy {
    ProtectionPolicy::new(checks.iter().map(|check| (*check).to_owned()).collect()).unwrap()
}

fn protected_elsewhere(checks: &[&str]) -> BranchRules {
    BranchRules {
        change_request_required: true,
        required_checks: checks.iter().map(|check| (*check).to_owned()).collect(),
        force_push_blocked: true,
        deletion_blocked: true,
    }
}

fn writes(forge: &FakeForge) -> usize {
    forge
        .calls()
        .iter()
        .filter(|call| **call == "protect_default_branch")
        .count()
}

#[test]
fn an_unprotected_default_branch_reads_with_every_gap() {
    let forge = FakeForge::new();

    let read = forge.default_branch_protection(&forge.access()).unwrap();

    assert_eq!(read.branch, branch("main"));
    assert_eq!(
        read.rules.gaps(),
        [
            Gap::ChangeRequestNotRequired,
            Gap::NoRequiredCheck,
            Gap::ForcePushAllowed,
            Gap::DeletionAllowed,
        ]
    );
}

#[test]
fn protect_creates_the_rule_and_the_branch_then_reads_protected() {
    let forge = FakeForge::new();

    let protected = forge
        .protect_default_branch(&forge.access(), &policy(&["ci"]))
        .unwrap();

    assert_eq!(protected.branch, branch("main"));
    assert_eq!(protected.outcome, ProtectOutcome::Created);
    let read = forge.default_branch_protection(&forge.access()).unwrap();
    assert_eq!(read.rules, protected_elsewhere(&["ci"]));
    assert_eq!(read.rules.gaps(), []);
}

#[test]
fn a_second_protect_with_the_same_checks_changes_nothing() {
    let forge = FakeForge::new();
    forge
        .protect_default_branch(&forge.access(), &policy(&["ci"]))
        .unwrap();

    let again = forge
        .protect_default_branch(&forge.access(), &policy(&["ci"]))
        .unwrap();

    assert_eq!(again.outcome, ProtectOutcome::Unchanged);
    assert_eq!(forge.own_rule(), Some(policy(&["ci"])));
}

#[test]
fn protect_with_a_new_check_updates_its_own_rule() {
    let forge = FakeForge::new();
    forge
        .protect_default_branch(&forge.access(), &policy(&["ci"]))
        .unwrap();

    let again = forge
        .protect_default_branch(&forge.access(), &policy(&["ci", "lint"]))
        .unwrap();

    assert_eq!(again.outcome, ProtectOutcome::Updated);
    assert_eq!(forge.own_rule(), Some(policy(&["ci", "lint"])));
}

#[test]
fn protect_with_the_same_checks_in_another_order_changes_nothing() {
    let forge = FakeForge::new();
    forge
        .protect_default_branch(&forge.access(), &policy(&["ci", "lint"]))
        .unwrap();

    let again = forge
        .protect_default_branch(&forge.access(), &policy(&["lint", "ci"]))
        .unwrap();

    assert_eq!(again.outcome, ProtectOutcome::Unchanged);
}

#[test]
fn protect_with_a_check_dropped_updates_its_own_rule() {
    let forge = FakeForge::new();
    forge
        .protect_default_branch(&forge.access(), &policy(&["ci", "lint"]))
        .unwrap();

    let again = forge
        .protect_default_branch(&forge.access(), &policy(&["ci"]))
        .unwrap();

    assert_eq!(again.outcome, ProtectOutcome::Updated);
    assert_eq!(forge.own_rule(), Some(policy(&["ci"])));
}

#[test]
fn a_branch_other_rules_protect_still_gets_torleks_rule() {
    let forge = FakeForge::new().with_rules(protected_elsewhere(&["ci"]));

    let protected = forge
        .protect_default_branch(&forge.access(), &policy(&["ci"]))
        .unwrap();

    assert_eq!(protected.outcome, ProtectOutcome::Created);
    assert_eq!(forge.own_rule(), Some(policy(&["ci"])));
}

#[test]
fn a_forge_without_the_capability_answers_typed_and_writes_nothing() {
    let forge = FakeForge::new().without(Capability::ProtectionBootstrap);

    let refused = forge
        .protect_default_branch(&forge.access(), &policy(&["ci"]))
        .unwrap_err();

    assert_eq!(
        refused,
        ForgeError::Unsupported {
            capability: Capability::ProtectionBootstrap
        }
    );
    assert!(refused.is_unsupported());
    assert_eq!(forge.own_rule(), None);
    assert_eq!(writes(&forge), 1, "the call is recorded, never applied");
}

#[test]
fn a_forge_without_the_read_answers_typed() {
    let forge = FakeForge::new().without(Capability::ProtectionRead);

    let refused = forge
        .default_branch_protection(&forge.access())
        .unwrap_err();

    assert!(refused.is_unsupported(), "{refused:?}");
}

#[test]
fn gaps_name_only_what_the_branch_lacks() {
    let rules = BranchRules {
        required_checks: vec!["ci".to_owned()],
        force_push_blocked: true,
        ..BranchRules::default()
    };

    assert_eq!(
        rules.gaps(),
        [Gap::ChangeRequestNotRequired, Gap::DeletionAllowed]
    );
}

#[test]
fn a_policy_without_a_check_is_refused() {
    let refused = ProtectionPolicy::new(Vec::new()).unwrap_err();

    assert_eq!(refused.kind, "required check list");
}

#[test]
fn a_policy_naming_a_check_twice_is_refused() {
    let refused = ProtectionPolicy::new(vec!["ci".to_owned(), "ci".to_owned()]).unwrap_err();

    assert_eq!(refused.rule, "each check named once");
}

#[test]
fn a_check_name_that_is_blank_or_has_a_control_character_is_refused() {
    for name in ["", "  ", "ci\nfake", &"c".repeat(256)] {
        let refused = ProtectionPolicy::new(vec![name.to_owned()]).unwrap_err();
        assert_eq!(refused.kind, "required check name", "{name:?}");
    }
}
