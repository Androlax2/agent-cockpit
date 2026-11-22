//! `gh`'s JSON answers in the contract's words. A field missing, of another type, or a value
//! GitHub does not document is refused as malformed, never guessed.

use cockpit_forge_api::{
    BranchName, ChangeId, ChangeRequest, ChangeState, Check, CheckState, Checks, Comment, CommitId,
    ForgeError, Issue, IssueId, IssueState, IssueThread, ReviewDecision,
};
use serde_json::{Map, Value};

type Object = Map<String, Value>;

/// The `--json` fields an issue list or read asks for.
pub(crate) const ISSUE_FIELDS: &str = "number,title,state,labels,author,url";
pub(crate) const ISSUE_THREAD_FIELDS: &str = "number,title,state,labels,author,url,body,comments";
/// The `--json` fields a pull request read asks for.
pub(crate) const PULL_FIELDS: &str = "number,title,url,state,isDraft,headRefName,headRefOid,baseRefName,mergeCommit,isCrossRepository";
/// The head commit and its checks, read in one answer so the checks are those of that commit.
pub(crate) const CHECK_FIELDS: &str = "headRefOid,statusCheckRollup";

fn malformed(reason: impl Into<String>) -> ForgeError {
    ForgeError::Malformed {
        reason: reason.into(),
    }
}

pub(crate) fn parse(text: &str) -> Result<Value, ForgeError> {
    serde_json::from_str(text).map_err(|error| malformed(format!("`gh` printed no JSON: {error}")))
}

fn object<'v>(value: &'v Value, what: &str) -> Result<&'v Object, ForgeError> {
    value
        .as_object()
        .ok_or_else(|| malformed(format!("{what} is not an object")))
}

pub(crate) fn array<'v>(value: &'v Value, what: &str) -> Result<&'v Vec<Value>, ForgeError> {
    value
        .as_array()
        .ok_or_else(|| malformed(format!("{what} is not a list")))
}

fn field<'v>(object: &'v Object, name: &str) -> Result<&'v Value, ForgeError> {
    object
        .get(name)
        .ok_or_else(|| malformed(format!("the answer has no `{name}`")))
}

fn text(object: &Object, name: &str) -> Result<String, ForgeError> {
    field(object, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| malformed(format!("`{name}` is not text")))
}

fn flag(object: &Object, name: &str) -> Result<bool, ForgeError> {
    field(object, name)?
        .as_bool()
        .ok_or_else(|| malformed(format!("`{name}` is not true or false")))
}

fn number(object: &Object, name: &str) -> Result<String, ForgeError> {
    field(object, name)?
        .as_u64()
        .map(|number| number.to_string())
        .ok_or_else(|| malformed(format!("`{name}` is not a number")))
}

/// An author's login; `None` for an account that is gone (`null`, or no login).
fn author(object: &Object) -> Result<Option<String>, ForgeError> {
    match object.get("author") {
        None | Some(Value::Null) => Ok(None),
        Some(author) => Ok(object_login(author)),
    }
}

fn object_login(author: &Value) -> Option<String> {
    author
        .get("login")
        .and_then(Value::as_str)
        .filter(|login| !login.is_empty())
        .map(str::to_owned)
}

fn invalid(error: cockpit_forge_api::InvalidValue) -> ForgeError {
    malformed(error.to_string())
}

pub(crate) fn issue(value: &Value) -> Result<Issue, ForgeError> {
    let object = object(value, "an issue")?;
    let state = match text(object, "state")?.as_str() {
        "OPEN" => IssueState::Open,
        "CLOSED" => IssueState::Closed,
        other => return Err(malformed(format!("issue state {other:?}"))),
    };
    let labels = array(field(object, "labels")?, "`labels`")?
        .iter()
        .map(|label| {
            label
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| malformed("a label has no name"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Issue {
        id: IssueId::new(number(object, "number")?).map_err(invalid)?,
        title: text(object, "title")?,
        state,
        labels,
        author: author(object)?,
        url: text(object, "url")?,
    })
}

pub(crate) fn issue_thread(value: &Value) -> Result<IssueThread, ForgeError> {
    let object = object(value, "an issue")?;
    let comments = array(field(object, "comments")?, "`comments`")?
        .iter()
        .map(|comment| {
            let comment = self::object(comment, "a comment")?;
            Ok(Comment {
                author: author(comment)?,
                body: text(comment, "body")?,
            })
        })
        .collect::<Result<Vec<_>, ForgeError>>()?;
    Ok(IssueThread {
        issue: issue(value)?,
        body: text(object, "body")?,
        comments,
    })
}

/// A pull request, and whether its head is in another repository (a fork).
pub(crate) fn pull(value: &Value) -> Result<(ChangeRequest, bool), ForgeError> {
    let object = object(value, "a pull request")?;
    let state = match text(object, "state")?.as_str() {
        "OPEN" => ChangeState::Open,
        "CLOSED" => ChangeState::Closed,
        "MERGED" => ChangeState::Merged {
            commit: match field(object, "mergeCommit")? {
                Value::Null => None,
                merge => Some(commit(&text(
                    self::object(merge, "`mergeCommit`")?,
                    "oid",
                )?)?),
            },
        },
        other => return Err(malformed(format!("pull request state {other:?}"))),
    };
    let request = ChangeRequest {
        id: ChangeId::new(number(object, "number")?).map_err(invalid)?,
        title: text(object, "title")?,
        url: text(object, "url")?,
        state,
        draft: flag(object, "isDraft")?,
        head: branch(&text(object, "headRefName")?)?,
        head_commit: commit(&text(object, "headRefOid")?)?,
        base: branch(&text(object, "baseRefName")?)?,
    };
    Ok((request, flag(object, "isCrossRepository")?))
}

fn commit(id: &str) -> Result<CommitId, ForgeError> {
    CommitId::new(id).map_err(invalid)
}

fn branch(name: &str) -> Result<BranchName, ForgeError> {
    BranchName::new(name).map_err(invalid)
}

/// `reviewDecision`: empty when the base branch requires no review.
pub(crate) fn review(value: &Value) -> Result<ReviewDecision, ForgeError> {
    let object = object(value, "a pull request")?;
    match field(object, "reviewDecision")? {
        Value::Null => Ok(ReviewDecision::NotRequired),
        Value::String(decision) => match decision.as_str() {
            "" => Ok(ReviewDecision::NotRequired),
            "APPROVED" => Ok(ReviewDecision::Approved),
            "CHANGES_REQUESTED" => Ok(ReviewDecision::ChangesRequested),
            "REVIEW_REQUIRED" => Ok(ReviewDecision::Required),
            other => Err(malformed(format!("review decision {other:?}"))),
        },
        _ => Err(malformed("`reviewDecision` is not text")),
    }
}

/// The head commit and the check runs and commit statuses on it. GitHub does not say here
/// which are required, so `required` is unknown.
pub(crate) fn checks(value: &Value) -> Result<Checks, ForgeError> {
    let object = object(value, "a pull request")?;
    let checks = match field(object, "statusCheckRollup")? {
        Value::Null => Vec::new(),
        rollup => array(rollup, "`statusCheckRollup`")?
            .iter()
            .map(check)
            .collect::<Result<Vec<_>, _>>()?,
    };
    Ok(Checks {
        commit: commit(&text(object, "headRefOid")?)?,
        checks,
    })
}

fn check(value: &Value) -> Result<Check, ForgeError> {
    let object = object(value, "a check")?;
    let (name, state) = match text(object, "__typename")?.as_str() {
        "CheckRun" => (text(object, "name")?, check_run_state(object)?),
        "StatusContext" => (
            text(object, "context")?,
            status_state(&text(object, "state")?)?,
        ),
        other => return Err(malformed(format!("check kind {other:?}"))),
    };
    Ok(Check {
        name,
        state,
        required: None,
    })
}

fn check_run_state(object: &Object) -> Result<CheckState, ForgeError> {
    if text(object, "status")? != "COMPLETED" {
        return Ok(CheckState::Pending);
    }
    match text(object, "conclusion")?.as_str() {
        "SUCCESS" => Ok(CheckState::Passed),
        "FAILURE" | "TIMED_OUT" | "STARTUP_FAILURE" | "ACTION_REQUIRED" => Ok(CheckState::Failed),
        "NEUTRAL" | "SKIPPED" => Ok(CheckState::Skipped),
        "CANCELLED" => Ok(CheckState::Cancelled),
        // GitHub marks a check stale when it is too old to count: it must run again.
        "STALE" => Ok(CheckState::Pending),
        other => Err(malformed(format!("check conclusion {other:?}"))),
    }
}

fn status_state(state: &str) -> Result<CheckState, ForgeError> {
    match state {
        "SUCCESS" => Ok(CheckState::Passed),
        "FAILURE" | "ERROR" => Ok(CheckState::Failed),
        "PENDING" | "EXPECTED" => Ok(CheckState::Pending),
        other => Err(malformed(format!("commit status {other:?}"))),
    }
}

/// The number at the end of the URL `gh` prints for what it made
/// (`https://github.com/owner/name/issues/12`), when the URL is under `prefix`.
pub(crate) fn created_number(stdout: &str, prefix: &str) -> Result<(String, String), ForgeError> {
    let url = stdout
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .ok_or_else(|| malformed("`gh` printed no URL"))?;
    let number = url
        .strip_prefix(prefix)
        .filter(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| malformed(format!("`gh` printed {url:?}, not a URL under {prefix}")))?;
    Ok((number.to_owned(), url.to_owned()))
}
