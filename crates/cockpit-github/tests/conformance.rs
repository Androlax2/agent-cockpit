//! The adapter against the forge contract's conformance suite, over `gh` 2.104.0's answers.

mod common;

use cockpit_forge_api::conformance::{Recorded, check};
use cockpit_forge_api::{
    Availability, BranchName, Capability, ChangeId, ChangeRequest, ChangeState, Check, CheckState,
    Checks, Comment, CommitId, Created, CredentialScheme, CredentialUse, Forge, ForgeHost, HostUse,
    Issue, IssueFilter, IssueId, IssueState, IssueThread, ListLimit, NewChangeRequest, NewIssue,
    Posted, ReviewDecision, ScopeCheck, TokenKind,
};
use common::{FakeGh, repository, token};

const HEAD: &str = "greenfield/60-github-adapter";
const HEAD_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const REPOSITORY: &str = "--repo=owner/project";
const ISSUE_FIELDS: &str = "--json=number,title,state,labels,author,url";
const PULL_FIELDS: &str = "--json=number,title,url,state,isDraft,headRefName,headRefOid,\
                           baseRefName,mergeCommit,isCrossRepository";

fn answering_every_call() -> FakeGh {
    let mut gh = FakeGh::new();
    gh.answer_with(
        &[
            "issue",
            "list",
            REPOSITORY,
            "--state=open",
            "--limit=2",
            ISSUE_FIELDS,
            "--label=greenfield",
        ],
        "issue-list.json",
    )
    .answer_with(
        &[
            "issue",
            "view",
            "12",
            REPOSITORY,
            "--json=number,title,state,labels,author,url,body,comments",
        ],
        "issue-view.json",
    )
    .fail_with(
        &[
            "issue",
            "view",
            "999",
            REPOSITORY,
            "--json=number,title,state,labels,author,url,body,comments",
        ],
        "issue-view-missing.stderr",
    )
    .answer_with(
        &["pr", "view", "20", REPOSITORY, PULL_FIELDS],
        "pr-view.json",
    )
    .fail_with(
        &["pr", "view", "999", REPOSITORY, PULL_FIELDS],
        "pr-view-missing.stderr",
    )
    .answer_with(
        &[
            "pr",
            "list",
            REPOSITORY,
            &format!("--head={HEAD}"),
            "--state=all",
            "--limit=30",
            PULL_FIELDS,
        ],
        "pr-list-head.json",
    )
    .answer_with(
        &[
            "pr",
            "list",
            REPOSITORY,
            "--head=no-pull-request",
            "--state=all",
            "--limit=30",
            PULL_FIELDS,
        ],
        "pr-list-none.json",
    )
    .answer_with(
        &[
            "pr",
            "view",
            "20",
            REPOSITORY,
            "--json=headRefOid,statusCheckRollup",
        ],
        "pr-checks.json",
    )
    .answer_with(
        &["pr", "view", "20", REPOSITORY, "--json=reviewDecision"],
        "pr-review.json",
    )
    .answer_with(
        &["api", "repos/owner/project", "--jq=.full_name"],
        "api-repo.stdout",
    )
    .answer_with(
        &[
            "issue",
            "create",
            REPOSITORY,
            "--title=File the follow-up",
            "--body-file=-",
            "--label=greenfield",
        ],
        "issue-create.stdout",
    )
    .answer_with(
        &["issue", "comment", "12", REPOSITORY, "--body-file=-"],
        "issue-comment.stdout",
    )
    .answer_with(
        &[
            "pr",
            "create",
            REPOSITORY,
            "--title=The GitHub adapter",
            "--body-file=-",
            &format!("--head={HEAD}"),
            "--base=main",
            "--draft",
        ],
        "pr-create.stdout",
    );
    gh
}

fn issue(number: &str, title: &str, author: &str) -> Issue {
    Issue {
        id: IssueId::new(number).unwrap(),
        title: title.to_owned(),
        state: IssueState::Open,
        labels: vec!["greenfield".to_owned()],
        author: Some(author.to_owned()),
        url: format!("https://github.com/owner/project/issues/{number}"),
    }
}

fn pull_request() -> ChangeRequest {
    ChangeRequest {
        id: ChangeId::new("20").unwrap(),
        title: "The GitHub adapter".to_owned(),
        url: "https://github.com/owner/project/pull/20".to_owned(),
        state: ChangeState::Open,
        draft: false,
        head: BranchName::new(HEAD).unwrap(),
        head_commit: CommitId::new(HEAD_COMMIT).unwrap(),
        base: BranchName::new("main").unwrap(),
    }
}

/// What `gh` 2.104.0's answers above mean in the contract's words.
fn recorded() -> Recorded {
    let mut recorded = Recorded::new(repository(), token());
    recorded.list_issues = Some((
        IssueFilter {
            state: Some(IssueState::Open),
            labels: vec!["greenfield".to_owned()],
            limit: ListLimit::new(2).unwrap(),
        },
        vec![
            issue("12", "Read issues beside todos", "octo"),
            issue("7", "Show PR state as evidence", "ghost"),
        ],
    ));
    recorded.read_issue = Some((
        IssueThread {
            issue: issue("12", "Read issues beside todos", "octo"),
            body: "Issues are a task source.".to_owned(),
            comments: vec![
                Comment {
                    author: Some("reviewer".to_owned()),
                    body: "Decided: beside todos.".to_owned(),
                },
                Comment {
                    author: None,
                    body: "A comment of a deleted account.".to_owned(),
                },
            ],
        },
        IssueId::new("999").unwrap(),
    ));
    recorded.change_request = Some((
        pull_request(),
        ChangeId::new("999").unwrap(),
        BranchName::new("no-pull-request").unwrap(),
    ));
    recorded.change_checks = Some((
        ChangeId::new("20").unwrap(),
        Checks {
            commit: CommitId::new(HEAD_COMMIT).unwrap(),
            checks: vec![
                Check {
                    name: "greenfield".to_owned(),
                    state: CheckState::Passed,
                    required: None,
                },
                Check {
                    name: "docs".to_owned(),
                    state: CheckState::Pending,
                    required: None,
                },
                Check {
                    name: "license/cla".to_owned(),
                    state: CheckState::Failed,
                    required: None,
                },
            ],
        },
    ));
    recorded.change_review = Some((ChangeId::new("20").unwrap(), ReviewDecision::Required));
    recorded.check_token = Some((
        TokenKind::Api,
        ScopeCheck::Unverified {
            reason: "GitHub does not show a fine-grained token's permissions or the \
                     repositories it reaches: check on its settings page that it has \
                     contents read, change requests write, issues write and nothing more, on \
                     this repository alone"
                .to_owned(),
        },
    ));
    recorded.create_issue = Some((
        NewIssue {
            title: "File the follow-up".to_owned(),
            body: "Found while reviewing.".to_owned(),
            labels: vec!["greenfield".to_owned()],
        },
        Created {
            id: IssueId::new("13").unwrap(),
            url: "https://github.com/owner/project/issues/13".to_owned(),
        },
    ));
    recorded.comment_on_issue = Some((
        IssueId::new("12").unwrap(),
        "Done in #20.".to_owned(),
        Posted {
            url: "https://github.com/owner/project/issues/12#issuecomment-503".to_owned(),
        },
    ));
    recorded.open_change_request = Some((
        NewChangeRequest {
            title: "The GitHub adapter".to_owned(),
            body: "Part 1 of 2.".to_owned(),
            head: BranchName::new(HEAD).unwrap(),
            base: BranchName::new("main").unwrap(),
            draft: true,
        },
        Created {
            id: ChangeId::new("22").unwrap(),
            url: "https://github.com/owner/project/pull/22".to_owned(),
        },
    ));
    recorded
}

#[test]
fn the_adapter_conforms_to_the_forge_contract_over_ghs_answers() {
    let gh = answering_every_call();

    let violations = check(&gh.adapter(), &recorded());

    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn it_declares_githubs_hosts_and_the_token_each_takes() {
    let adapter = FakeGh::new().adapter();
    let git = ForgeHost::new("github.com").unwrap();
    let api = ForgeHost::new("api.github.com").unwrap();
    let uses = |token, host: &ForgeHost, scheme, used_for| CredentialUse {
        token,
        host: host.clone(),
        scheme,
        used_for,
    };

    assert_eq!(
        adapter.declaration().uses(),
        [
            uses(
                TokenKind::Api,
                &git,
                CredentialScheme::Basic,
                HostUse::GitRead
            ),
            uses(TokenKind::Api, &api, CredentialScheme::Bearer, HostUse::Api),
            uses(
                TokenKind::Push,
                &git,
                CredentialScheme::Basic,
                HostUse::GitPush
            ),
        ]
    );
}

#[test]
fn its_capabilities_stay_unverified_until_answers_are_recorded_from_a_live_gh() {
    let adapter = FakeGh::new().adapter();

    assert_eq!(adapter.version(), Some("2.104.0"));
    assert_eq!(
        adapter.capabilities().availability(Capability::IssueList),
        Availability::Unverified
    );
}

#[test]
fn branch_protection_is_not_reported_until_its_contract_methods_exist() {
    let capabilities = FakeGh::new().adapter().capabilities();

    for capability in [Capability::ProtectionRead, Capability::ProtectionBootstrap] {
        assert_eq!(
            capabilities.availability(capability),
            Availability::Unavailable,
            "{capability}"
        );
    }
}

#[test]
fn a_gh_that_cannot_run_withholds_every_capability_naming_why() {
    let gh = FakeGh::new();
    let adapter =
        cockpit_github::GitHub::new(gh.program().with_file_name("absent"), gh.config_dir());

    let capabilities = adapter.capabilities();

    assert_eq!(adapter.version(), None);
    let reason = capabilities.withheld(Capability::IssueList).unwrap();
    assert!(reason.contains("cannot run"), "{reason}");
    assert!(capabilities.reported().next().is_none());
}
