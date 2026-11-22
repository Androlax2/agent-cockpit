//! The `merged` requirement (#60, Theo 2026-11-22): a workflow step that names it completes
//! only once the forge reports the change request of the checkout's branch merged, with the
//! checkout's head as its head. It is read from the forge at each completion with the
//! project's API token, never cached as evidence (the handoff lists it as satisfied, as it
//! does a check), and never a declared check: a check named `merged` does not stand in for
//! it. The forge is asked about the local branch's name, the one `torlek checkout push`
//! pushes to. No preset names it yet.

use std::path::Path;

use cockpit_forge_api::{Access, BranchName, ChangeState, CommitId, Forge, ForgeError};
use cockpit_protocol::{ErrorCode, ProtocolError, Unblock, UnblockedBy};

use crate::approvals::Failed;
use crate::forge_reads::Reader;

/// The requirement's name, as a step lists it.
pub(crate) const MERGED: &str = "merged";

/// What the forge says of the checkout's change request.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Merged,
    /// Not merged, or not this head: the reason names which.
    Unmet(String),
}

/// The head of `project`'s checkout at `checkout`, once the forge reports the change request
/// of its branch merged with that head; refused naming why when it is not. Blocking: it asks
/// Git for the branch and head, then reads the forge.
pub(crate) fn merged_head(
    reader: &Reader<'_>,
    project: &str,
    checkout: &Path,
) -> Result<String, Failed> {
    let unmet = |reason: String| {
        agent_refused(
            format!("the step requires `{MERGED}`: {reason}"),
            format!(
                "merge the change request at the checkout's head, or ask the owner to waive \
                 `{MERGED}`"
            ),
        )
    };
    let Some(branch) = reader
        .git
        .current_branch(checkout)
        .map_err(|error| checkout_unread(&error))?
    else {
        return Err(unmet(
            "HEAD is detached, so no branch names a change request".to_owned(),
        ));
    };
    let unreadable = |what: &str, value: &str| {
        agent_refused(
            format!("the checkout's {what} {value:?} is not one the forge names"),
            "switch to the branch the change request was opened from".to_owned(),
        )
    };
    let branch_name = BranchName::new(&branch).map_err(|_| unreadable("branch", &branch))?;
    let head = reader
        .git
        .source_state(checkout)
        .map_err(|error| checkout_unread(&error))?
        .head;
    let head_id = CommitId::new(&head).map_err(|_| unreadable("head", &head))?;
    match reader.read(
        project,
        checkout,
        &format!("the state of {project}'s change request for {branch}"),
        "`torlek workflow complete` again",
        |forge, access| verdict(forge, access, &branch_name, &head_id),
    )? {
        Verdict::Merged => Ok(head),
        Verdict::Unmet(reason) => Err(unmet(reason)),
    }
}

fn agent_refused(message: String, action: String) -> Failed {
    Failed(ProtocolError {
        code: ErrorCode::Refused,
        message,
        unblock: Some(Unblock {
            action,
            by: UnblockedBy::Agent,
        }),
    })
}

fn checkout_unread(error: &dyn std::fmt::Display) -> Failed {
    tracing::error!(%error, "reading the checkout for the `merged` requirement failed");
    Failed(ProtocolError::new(
        ErrorCode::Internal,
        "the daemon failed to read the checkout's branch and head",
    ))
}

/// The verdict on `branch`'s newest change request, which must have merged with `head` as
/// its head commit: one merged with another head does not hold this result.
pub(crate) fn verdict(
    forge: &dyn Forge,
    access: &Access<'_>,
    branch: &BranchName,
    head: &CommitId,
) -> Result<Verdict, ForgeError> {
    let Some(change) = forge.change_request_for(access, branch)? else {
        return Ok(Verdict::Unmet(format!(
            "no change request has {branch} as its head"
        )));
    };
    Ok(match change.state {
        ChangeState::Merged { .. } if change.head_commit == *head => Verdict::Merged,
        ChangeState::Merged { .. } => Verdict::Unmet(format!(
            "{} merged with head {}, not the checkout's {head}",
            change.url, change.head_commit
        )),
        ChangeState::Open => Verdict::Unmet(format!("{} is open", change.url)),
        ChangeState::Closed => Verdict::Unmet(format!("{} was closed unmerged", change.url)),
    })
}

#[cfg(test)]
mod tests {
    use cockpit_forge_api::fake::FakeForge;
    use cockpit_forge_api::{ChangeId, ChangeRequest};

    use super::*;

    const HEAD: &str = "1111111111111111111111111111111111111111";
    const OTHER: &str = "2222222222222222222222222222222222222222";

    fn change(state: ChangeState, head_commit: &str) -> ChangeRequest {
        ChangeRequest {
            id: ChangeId::new("12").unwrap(),
            title: "the change".to_owned(),
            url: "https://forge.example/owner/app/pull/12".to_owned(),
            state,
            draft: false,
            head: BranchName::new("feature/x").unwrap(),
            head_commit: CommitId::new(head_commit).unwrap(),
            base: BranchName::new("main").unwrap(),
        }
    }

    fn judged(forge: &FakeForge) -> Verdict {
        verdict(
            forge,
            &forge.access(),
            &BranchName::new("feature/x").unwrap(),
            &CommitId::new(HEAD).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_change_merged_with_the_checkouts_head_meets_it() {
        let forge =
            FakeForge::new().with_change(change(ChangeState::Merged { commit: None }, HEAD));

        assert_eq!(judged(&forge), Verdict::Merged);
    }

    #[test]
    fn a_change_merged_with_another_head_does_not_hold_this_result() {
        let forge =
            FakeForge::new().with_change(change(ChangeState::Merged { commit: None }, OTHER));

        let Verdict::Unmet(reason) = judged(&forge) else {
            panic!("another head met it");
        };
        assert!(reason.contains(OTHER) && reason.contains(HEAD), "{reason}");
    }

    #[test]
    fn an_open_or_closed_change_does_not_meet_it() {
        for (state, said) in [
            (ChangeState::Open, "is open"),
            (ChangeState::Closed, "closed unmerged"),
        ] {
            let forge = FakeForge::new().with_change(change(state, HEAD));

            let Verdict::Unmet(reason) = judged(&forge) else {
                panic!("{said}: met");
            };
            assert!(reason.contains(said), "{reason}");
        }
    }

    #[test]
    fn a_branch_with_no_change_request_does_not_meet_it() {
        let Verdict::Unmet(reason) = judged(&FakeForge::new()) else {
            panic!("no change request met it");
        };
        assert!(reason.contains("no change request"), "{reason}");
    }

    #[test]
    fn a_forge_that_cannot_read_change_state_fails_loud() {
        let forge = FakeForge::new().without(cockpit_forge_api::Capability::ChangeState);

        let failed = verdict(
            &forge,
            &forge.access(),
            &BranchName::new("feature/x").unwrap(),
            &CommitId::new(HEAD).unwrap(),
        );

        assert!(failed.is_err_and(|error| error.is_unsupported()));
    }
}
