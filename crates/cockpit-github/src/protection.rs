//! The default branch's protection on GitHub (#695), through `gh api`: the rules every ruleset
//! applies to the branch, read with the API token, and Torlek's own repository ruleset, made
//! or replaced with the token the owner gives for that call.

use cockpit_forge_api::{BranchName, BranchRules, ForgeError, ProtectionPolicy};
use serde_json::{Value, json};

use crate::answers;

/// The name of Torlek's ruleset; protect finds its own by it, and GitHub keeps names unique
/// within a repository.
pub(crate) const RULESET_NAME: &str = "torlek-default-branch";

/// The most rules or rulesets one page lists; a full page may be cut short.
pub(crate) const PAGE: usize = 100;

/// `branch` as one part of a URL path: GitHub takes a branch's `/` as is, and anything else
/// but letters, digits and `-._~` escaped.
pub(crate) fn path_segment(branch: &BranchName) -> String {
    branch
        .as_str()
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The branch `gh api repos/OWNER/NAME --jq=.default_branch` printed.
pub(crate) fn default_branch(stdout: &str) -> Result<BranchName, ForgeError> {
    BranchName::new(stdout.trim_end_matches('\n')).map_err(|error| ForgeError::Malformed {
        reason: format!("the default branch: {error}"),
    })
}

/// `true` or `false` as `--jq=.protected` prints it.
pub(crate) fn is_protected(stdout: &str) -> Result<bool, ForgeError> {
    match stdout.trim_end_matches('\n') {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(ForgeError::Malformed {
            reason: format!("the branch's `protected` is {other:?}, not true or false"),
        }),
    }
}

/// The rules of `GET repos/OWNER/NAME/rules/branches/BRANCH`, active ones only, from every
/// ruleset that applies. A rule of another type says nothing of these four and is skipped.
pub(crate) fn rules(value: &Value) -> Result<BranchRules, ForgeError> {
    let listed = answers::array(value, "the branch's rules")?;
    if listed.len() >= PAGE {
        return Err(ForgeError::Unavailable {
            reason: format!("the branch has {PAGE} rules or more, and a full page may be cut"),
        });
    }
    let mut rules = BranchRules::default();
    for rule in listed {
        let kind = rule
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("a rule has no `type`"))?;
        match kind {
            "pull_request" => rules.change_request_required = true,
            "non_fast_forward" => rules.force_push_blocked = true,
            "deletion" => rules.deletion_blocked = true,
            "required_status_checks" => {
                for check in required_checks(rule)? {
                    if !rules.required_checks.contains(&check) {
                        rules.required_checks.push(check);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(rules)
}

fn required_checks(rule: &Value) -> Result<Vec<String>, ForgeError> {
    let checks = rule
        .get("parameters")
        .and_then(|parameters| parameters.get("required_status_checks"))
        .ok_or_else(|| malformed("a required checks rule lists no checks"))?;
    answers::array(checks, "the required checks")?
        .iter()
        .map(|check| {
            check
                .get("context")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| malformed("a required check has no `context`"))
        })
        .collect()
}

/// The id of Torlek's ruleset among `GET repos/OWNER/NAME/rulesets?includes_parents=false`,
/// when there is one.
pub(crate) fn own_ruleset(value: &Value) -> Result<Option<u64>, ForgeError> {
    let listed = answers::array(value, "the repository's rulesets")?;
    for ruleset in listed {
        let name = ruleset
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("a ruleset has no `name`"))?;
        if name == RULESET_NAME {
            return ruleset_id(ruleset).map(Some);
        }
    }
    if listed.len() >= PAGE {
        return Err(ForgeError::Unavailable {
            reason: format!(
                "the repository has {PAGE} rulesets or more, and Torlek's may be past the first page"
            ),
        });
    }
    Ok(None)
}

fn ruleset_id(ruleset: &Value) -> Result<u64, ForgeError> {
    ruleset
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| malformed("a ruleset has no numeric `id`"))
}

/// The ruleset `policy` stands for: active on whichever branch is the default, with no one
/// allowed to bypass it.
pub(crate) fn ruleset_body(policy: &ProtectionPolicy) -> String {
    let checks: Vec<Value> = policy
        .required_checks()
        .iter()
        .map(|check| json!({ "context": check }))
        .collect();
    json!({
        "name": RULESET_NAME,
        "target": "branch",
        "enforcement": "active",
        "bypass_actors": [],
        "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
        "rules": [
            { "type": "deletion" },
            { "type": "non_fast_forward" },
            {
                "type": "pull_request",
                "parameters": {
                    "required_approving_review_count": 0,
                    "dismiss_stale_reviews_on_push": false,
                    "require_code_owner_review": false,
                    "require_last_push_approval": false,
                    "required_review_thread_resolution": false
                }
            },
            {
                "type": "required_status_checks",
                "parameters": {
                    "strict_required_status_checks_policy": false,
                    "required_status_checks": checks
                }
            }
        ]
    })
    .to_string()
}

/// Refuses an answer to a ruleset write that is not Torlek's ruleset, or, for a replacement,
/// not the one replaced.
pub(crate) fn check_written(value: &Value, replaced: Option<u64>) -> Result<(), ForgeError> {
    let id = ruleset_id(value)?;
    let name = value.get("name").and_then(Value::as_str);
    if name != Some(RULESET_NAME) {
        return Err(malformed(format!(
            "GitHub answered the ruleset write with ruleset {name:?}, not {RULESET_NAME:?}"
        )));
    }
    if let Some(replaced) = replaced
        && replaced != id
    {
        return Err(malformed(format!(
            "asked to replace ruleset {replaced}, GitHub answered ruleset {id}"
        )));
    }
    Ok(())
}

fn malformed(reason: impl Into<String>) -> ForgeError {
    ForgeError::Malformed {
        reason: reason.into(),
    }
}
