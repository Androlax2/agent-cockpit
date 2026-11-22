//! Seeded values for the fake forge's tests.

#![allow(dead_code, reason = "each test file uses some of these")]

use cockpit_forge_api::{
    BranchName, ChangeId, ChangeRequest, ChangeState, Check, CheckState, Checks, Comment, CommitId,
    Issue, IssueId, IssueState, IssueThread,
};

pub fn commit(digit: char) -> CommitId {
    CommitId::new(digit.to_string().repeat(40)).unwrap()
}

pub fn branch(name: &str) -> BranchName {
    BranchName::new(name).unwrap()
}

pub fn thread(id: &str, state: IssueState, labels: &[&str]) -> IssueThread {
    IssueThread {
        issue: Issue {
            id: IssueId::new(id).unwrap(),
            title: format!("issue {id}"),
            state,
            labels: labels.iter().map(|label| (*label).to_owned()).collect(),
            author: Some("someone".to_owned()),
            url: format!("https://forge.example.com/owner/project/issues/{id}"),
        },
        body: format!("body of {id}"),
        comments: vec![Comment {
            author: None,
            body: "a comment".to_owned(),
        }],
    }
}

pub fn change(id: &str, head: &str, state: ChangeState) -> ChangeRequest {
    ChangeRequest {
        id: ChangeId::new(id).unwrap(),
        title: format!("change {id}"),
        url: format!("https://forge.example.com/owner/project/pulls/{id}"),
        state,
        draft: false,
        head: branch(head),
        head_commit: commit('a'),
        base: branch("main"),
    }
}

pub fn passed_checks() -> Checks {
    Checks {
        commit: commit('a'),
        checks: vec![Check {
            name: "ci".to_owned(),
            state: CheckState::Passed,
            required: Some(true),
        }],
    }
}
