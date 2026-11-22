//! The fake forge other crates' tests use in place of a real one.

mod common;

use cockpit_forge_api::fake::FakeForge;
use cockpit_forge_api::{
    Access, Capability, ChangeId, ChangeState, Forge, ForgeError, IssueFilter, IssueId, IssueState,
    ListLimit, NewChangeRequest, NewIssue, Permission, ReviewDecision, ScopeCheck, ScopeProblem,
    Token, TokenKind,
};
use common::{branch, change, commit, passed_checks, thread};

fn filter(state: Option<IssueState>, labels: &[&str], limit: usize) -> IssueFilter {
    IssueFilter {
        state,
        labels: labels.iter().map(|label| (*label).to_owned()).collect(),
        limit: ListLimit::new(limit).unwrap(),
    }
}

fn seeded() -> FakeForge {
    FakeForge::new()
        .with_issue(thread("1", IssueState::Open, &["greenfield"]))
        .with_issue(thread("2", IssueState::Closed, &["greenfield"]))
        .with_issue(thread("3", IssueState::Open, &[]))
}

fn listed_ids(forge: &FakeForge, filter: &IssueFilter) -> Vec<String> {
    forge
        .list_issues(&forge.access(), filter)
        .unwrap()
        .into_iter()
        .map(|issue| issue.id.as_str().to_owned())
        .collect()
}

#[test]
fn issues_are_listed_newest_first_by_state_and_labels_up_to_the_limit() {
    let forge = seeded();

    assert_eq!(listed_ids(&forge, &filter(None, &[], 100)), ["3", "2", "1"]);
    assert_eq!(
        listed_ids(&forge, &filter(Some(IssueState::Open), &[], 100)),
        ["3", "1"]
    );
    assert_eq!(
        listed_ids(&forge, &filter(None, &["greenfield"], 100)),
        ["2", "1"]
    );
    assert_eq!(listed_ids(&forge, &filter(None, &[], 1)), ["3"]);
}

#[test]
fn a_filed_issue_can_be_read_back_with_its_comment() {
    let forge = FakeForge::new();
    let access = forge.access();
    let created = forge
        .create_issue(
            &access,
            &NewIssue {
                title: "a task".to_owned(),
                body: "do it".to_owned(),
                labels: vec!["todo".to_owned()],
            },
        )
        .unwrap();

    forge
        .comment_on_issue(&access, &created.id, "started")
        .unwrap();
    let read = forge.read_issue(&access, &created.id).unwrap();

    assert_eq!(read.issue.title, "a task");
    assert_eq!(read.body, "do it");
    assert_eq!(read.comments.last().unwrap().body, "started");
    assert_eq!(read.issue.url, created.url);
}

#[test]
fn a_missing_issue_is_not_found() {
    let forge = seeded();

    let error = forge
        .read_issue(&forge.access(), &IssueId::new("99").unwrap())
        .unwrap_err();

    assert!(matches!(error, ForgeError::NotFound { .. }), "{error:?}");
}

#[test]
fn a_change_request_is_found_by_id_and_by_its_head_newest_first() {
    let older = change("10", "feature", ChangeState::Closed);
    let newer = change("11", "feature", ChangeState::Open);
    let forge = FakeForge::new()
        .with_change(older.clone())
        .with_change(newer.clone());
    let access = forge.access();

    assert_eq!(forge.change_request(&access, &older.id).unwrap(), older);
    assert_eq!(
        forge
            .change_request_for(&access, &branch("feature"))
            .unwrap(),
        Some(newer)
    );
    assert_eq!(
        forge.change_request_for(&access, &branch("other")).unwrap(),
        None
    );
}

#[test]
fn merged_checks_and_review_are_read_separately() {
    let merged = change(
        "12",
        "feature",
        ChangeState::Merged {
            commit: Some(commit('b')),
        },
    );
    let forge = FakeForge::new()
        .with_change(merged.clone())
        .with_checks(merged.id.clone(), passed_checks())
        .with_review(merged.id.clone(), ReviewDecision::Approved);
    let access = forge.access();

    assert_eq!(
        forge.change_request(&access, &merged.id).unwrap().state,
        ChangeState::Merged {
            commit: Some(commit('b'))
        }
    );
    assert_eq!(
        forge.change_checks(&access, &merged.id).unwrap(),
        passed_checks()
    );
    assert_eq!(
        forge.change_review(&access, &merged.id).unwrap(),
        ReviewDecision::Approved
    );
}

#[test]
fn a_change_request_opens_from_a_branch_the_forge_has() {
    let forge = FakeForge::new().with_branch(branch("feature"), commit('c'));
    let access = forge.access();
    let request = NewChangeRequest {
        title: "feature".to_owned(),
        body: String::new(),
        head: branch("feature"),
        base: branch("main"),
        draft: false,
    };

    let opened = forge.open_change_request(&access, &request).unwrap();
    let read = forge.change_request(&access, &opened.id).unwrap();

    assert_eq!(read.head_commit, commit('c'));
    assert_eq!(read.state, ChangeState::Open);
}

#[test]
fn a_change_request_from_a_branch_the_forge_lacks_is_refused() {
    let forge = FakeForge::new();
    let request = NewChangeRequest {
        title: "feature".to_owned(),
        body: String::new(),
        head: branch("feature"),
        base: branch("main"),
        draft: false,
    };

    let error = forge
        .open_change_request(&forge.access(), &request)
        .unwrap_err();

    assert!(matches!(error, ForgeError::Refused { .. }), "{error:?}");
}

#[test]
fn a_scope_check_answers_what_it_was_told_and_unverified_otherwise() {
    let refused = ScopeCheck::Refused {
        problems: vec![ScopeProblem::Excess(Permission::ContentsWrite)],
    };
    let forge = FakeForge::new().with_scope(TokenKind::Api, refused.clone());
    let access = forge.access();

    assert_eq!(forge.check_token(&access, TokenKind::Api).unwrap(), refused);
    assert!(matches!(
        forge.check_token(&access, TokenKind::Push).unwrap(),
        ScopeCheck::Unverified { .. }
    ));
}

#[test]
fn another_token_is_refused_without_repeating_it() {
    let forge = seeded();
    let wrong = Token::new("wrong-token-value").unwrap();
    let access = Access {
        repository: forge.repository(),
        token: &wrong,
    };

    let error = forge
        .list_issues(&access, &filter(None, &[], 1))
        .unwrap_err();

    assert!(matches!(error, ForgeError::Refused { .. }), "{error:?}");
    assert!(!format!("{error} {error:?}").contains("wrong-token-value"));
}

#[test]
fn a_capability_taken_away_answers_unsupported_and_the_call_is_recorded() {
    let forge = seeded().without(Capability::IssueList);

    let error = forge
        .list_issues(&forge.access(), &filter(None, &[], 1))
        .unwrap_err();

    assert_eq!(
        error,
        ForgeError::Unsupported {
            capability: Capability::IssueList
        }
    );
    assert_eq!(forge.calls(), ["list_issues"]);
    assert!(
        forge
            .change_checks(&forge.access(), &ChangeId::new("1").unwrap())
            .is_err()
    );
}
