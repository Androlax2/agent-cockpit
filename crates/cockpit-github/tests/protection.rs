//! #695: the default branch's protection over a fake `gh`. The read takes three `gh api`
//! calls with the caller's token; protect writes Torlek's ruleset once, replaces its own when
//! it falls short, and writes nothing on a branch that already meets the policy.

mod common;

use cockpit_forge_api::{
    BranchName, BranchRules, Forge, ForgeError, Gap, ProtectOutcome, ProtectionPolicy,
};
use common::{FakeGh, TOKEN, with_access};
use serde_json::Value;

const RULESETS: &[&str] = &[
    "api",
    "repos/owner/project/rulesets?includes_parents=false&per_page=100",
];
const CREATE: &[&str] = &[
    "api",
    "--method=POST",
    "repos/owner/project/rulesets",
    "--input=-",
];
const REPLACE: &[&str] = &[
    "api",
    "--method=PUT",
    "repos/owner/project/rulesets/42",
    "--input=-",
];

fn policy(checks: &[&str]) -> ProtectionPolicy {
    ProtectionPolicy::new(checks.iter().map(|check| (*check).to_owned()).collect()).unwrap()
}

fn unprotected() -> FakeGh {
    let mut gh = FakeGh::new();
    gh.answer_protection_read(
        "api-rules-branch-none.json",
        "api-branch-protected-false.stdout",
    );
    gh
}

fn written_calls(gh: &FakeGh) -> Vec<Vec<String>> {
    gh.calls()
        .into_iter()
        .map(|call| call.arguments)
        .filter(|arguments| {
            arguments
                .iter()
                .any(|argument| argument.starts_with("--method="))
        })
        .collect()
}

#[test]
fn an_unprotected_default_branch_is_read_with_every_gap() {
    let gh = unprotected();

    let read = with_access(|access| gh.adapter().default_branch_protection(access)).unwrap();

    assert_eq!(read.branch, BranchName::new("main").unwrap());
    assert!(!read.has_unread_protection);
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
fn the_rules_of_every_ruleset_on_the_branch_are_read_together() {
    let mut gh = FakeGh::new();
    gh.answer_protection_read(
        "api-rules-branch-protected.json",
        "api-branch-protected-true.stdout",
    );

    let read = with_access(|access| gh.adapter().default_branch_protection(access)).unwrap();

    assert_eq!(
        read.rules,
        BranchRules {
            change_request_required: true,
            required_checks: vec!["ci".to_owned()],
            force_push_blocked: true,
            deletion_blocked: true,
        }
    );
    assert!(read.has_unread_protection);
}

#[test]
fn every_read_carries_the_callers_token_and_no_body() {
    let gh = unprotected();

    with_access(|access| gh.adapter().default_branch_protection(access)).unwrap();

    let calls = gh.calls();
    assert_eq!(calls.len(), 3, "{calls:#?}");
    for call in calls {
        assert_eq!(
            call.environment.get("GH_TOKEN").map(String::as_str),
            Some(TOKEN)
        );
        assert!(
            !call
                .arguments
                .iter()
                .any(|argument| argument.contains(TOKEN))
        );
        assert_eq!(call.stdin, "");
    }
}

#[test]
fn protect_creates_torleks_ruleset_on_the_default_branch_with_no_bypass() {
    let mut gh = unprotected();
    gh.answer_with(RULESETS, "api-rulesets-none.json")
        .answer_with(CREATE, "api-ruleset-written.json");

    let protected = with_access(|access| {
        gh.adapter()
            .protect_default_branch(access, &policy(&["ci"]))
    })
    .unwrap();

    assert_eq!(protected.outcome, ProtectOutcome::Created);
    let create = gh
        .calls()
        .into_iter()
        .find(|call| call.arguments == CREATE)
        .expect("a ruleset is created");
    let body: Value = serde_json::from_str(&create.stdin).unwrap();
    assert_eq!(body["name"], "torlek-default-branch");
    assert_eq!(body["enforcement"], "active");
    assert_eq!(body["bypass_actors"], serde_json::json!([]));
    assert_eq!(
        body["conditions"]["ref_name"]["include"],
        serde_json::json!(["~DEFAULT_BRANCH"])
    );
    let types: Vec<&str> = body["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        [
            "deletion",
            "non_fast_forward",
            "pull_request",
            "required_status_checks"
        ]
    );
    assert_eq!(
        body["rules"][2]["parameters"]["required_approving_review_count"],
        0
    );
    assert_eq!(
        body["rules"][3]["parameters"]["required_status_checks"],
        serde_json::json!([{ "context": "ci" }])
    );
}

#[test]
fn protect_on_a_branch_that_meets_the_policy_writes_nothing() {
    let mut gh = FakeGh::new();
    gh.answer_protection_read(
        "api-rules-branch-protected.json",
        "api-branch-protected-true.stdout",
    );

    let protected = with_access(|access| {
        gh.adapter()
            .protect_default_branch(access, &policy(&["ci"]))
    })
    .unwrap();

    assert_eq!(protected.outcome, ProtectOutcome::Unchanged);
    assert_eq!(gh.calls().len(), 3, "the read alone: {:#?}", gh.calls());
}

#[test]
fn protect_replaces_its_own_ruleset_when_the_branch_falls_short() {
    let mut gh = FakeGh::new();
    gh.answer_protection_read(
        "api-rules-branch-partial.json",
        "api-branch-protected-true.stdout",
    )
    .answer_with(RULESETS, "api-rulesets-own.json")
    .answer_with(REPLACE, "api-ruleset-written.json");

    let protected = with_access(|access| {
        gh.adapter()
            .protect_default_branch(access, &policy(&["ci"]))
    })
    .unwrap();

    assert_eq!(protected.outcome, ProtectOutcome::Updated);
    assert_eq!(written_calls(&gh), [REPLACE]);
}

#[test]
fn a_token_without_administration_is_refused_by_github() {
    let mut gh = unprotected();
    gh.answer_with(RULESETS, "api-rulesets-none.json")
        .fail_with(CREATE, "api-forbidden.stderr");

    let refused = with_access(|access| {
        gh.adapter()
            .protect_default_branch(access, &policy(&["ci"]))
    })
    .unwrap_err();

    assert!(
        matches!(&refused, ForgeError::Refused { reason } if reason.contains("HTTP 403")),
        "{refused:?}"
    );
}

#[test]
fn a_written_ruleset_that_is_not_torleks_is_malformed() {
    let mut gh = unprotected();
    gh.answer_with(RULESETS, "api-rulesets-none.json").answer(
        CREATE,
        0,
        r#"{"id":43,"name":"main"}"#,
        "",
    );

    let refused = with_access(|access| {
        gh.adapter()
            .protect_default_branch(access, &policy(&["ci"]))
    })
    .unwrap_err();

    assert!(
        matches!(refused, ForgeError::Malformed { .. }),
        "{refused:?}"
    );
}

#[test]
fn a_protected_mark_other_than_true_or_false_is_malformed() {
    let mut gh = FakeGh::new();
    gh.answer_with(
        &["api", "repos/owner/project", "--jq=.default_branch"],
        "api-default-branch.stdout",
    )
    .answer_with(
        &[
            "api",
            "repos/owner/project/rules/branches/main?per_page=100",
        ],
        "api-rules-branch-none.json",
    )
    .answer(
        &[
            "api",
            "repos/owner/project/branches/main",
            "--jq=.protected",
        ],
        0,
        "yes\n",
        "",
    );

    let refused = with_access(|access| gh.adapter().default_branch_protection(access)).unwrap_err();

    assert!(
        matches!(refused, ForgeError::Malformed { .. }),
        "{refused:?}"
    );
}

#[test]
fn a_default_branch_with_url_characters_is_escaped_in_the_path() {
    let mut gh = FakeGh::new();
    gh.answer(
        &["api", "repos/owner/project", "--jq=.default_branch"],
        0,
        "release/v1#2\n",
        "",
    )
    .answer_with(
        &[
            "api",
            "repos/owner/project/rules/branches/release/v1%232?per_page=100",
        ],
        "api-rules-branch-none.json",
    )
    .answer_with(
        &[
            "api",
            "repos/owner/project/branches/release/v1%232",
            "--jq=.protected",
        ],
        "api-branch-protected-false.stdout",
    );

    let read = with_access(|access| gh.adapter().default_branch_protection(access)).unwrap();

    assert_eq!(read.branch.as_str(), "release/v1#2");
}
