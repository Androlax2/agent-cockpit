//! What the adapter makes of `gh`'s answers beyond the conformance suite: forks, refusals,
//! errors, check states and the scope check.

mod common;

use cockpit_forge_api::{
    Access, BranchName, ChangeId, ChangeState, CheckState, CommitId, Forge, ForgeError, ForgeHost,
    IssueFilter, IssueId, ListLimit, Permission, Repository, RepositoryPath, ScopeCheck,
    ScopeProblem, Token, TokenKind,
};
use common::{FakeGh, TOKEN, read_fixture, with_access};

const REPOSITORY: &str = "--repo=owner/project";
const PULL_FIELDS: &str = "--json=number,title,url,state,isDraft,headRefName,headRefOid,\
                           baseRefName,mergeCommit,isCrossRepository";

fn head_lookup(branch: &str) -> Vec<String> {
    [
        "pr",
        "list",
        REPOSITORY,
        &format!("--head={branch}"),
        "--state=all",
        "--limit=30",
        PULL_FIELDS,
    ]
    .map(str::to_owned)
    .to_vec()
}

fn arguments(owned: &[String]) -> Vec<&str> {
    owned.iter().map(String::as_str).collect()
}

#[test]
fn a_pull_request_from_a_forks_branch_of_the_same_name_is_not_the_branchs() {
    let branch = "greenfield/60-github-adapter";
    let lookup = head_lookup(branch);
    let mut gh = FakeGh::new();
    gh.answer_with(&arguments(&lookup), "pr-list-head.json");

    let found = with_access(|access| {
        gh.adapter()
            .change_request_for(access, &BranchName::new(branch).unwrap())
    });

    assert_eq!(found.unwrap().unwrap().id.as_str(), "20");
}

#[test]
fn only_a_forks_pull_request_on_the_branch_is_none() {
    let branch = "greenfield/60-github-adapter";
    let lookup = head_lookup(branch);
    let forks_only: serde_json::Value =
        serde_json::from_str(&read_fixture("pr-list-head.json")).unwrap();
    let forks_only = serde_json::Value::Array(vec![forks_only[0].clone()]);
    let mut gh = FakeGh::new();
    gh.answer(&arguments(&lookup), 0, &forks_only.to_string(), "");

    let found = with_access(|access| {
        gh.adapter()
            .change_request_for(access, &BranchName::new(branch).unwrap())
    });

    assert_eq!(found.unwrap(), None);
}

#[test]
fn a_merged_pull_request_names_its_merge_commit() {
    let mut gh = FakeGh::new();
    gh.answer_with(
        &["pr", "view", "20", REPOSITORY, PULL_FIELDS],
        "pr-view-merged.json",
    );

    let request = with_access(|access| {
        gh.adapter()
            .change_request(access, &ChangeId::new("20").unwrap())
    });

    assert_eq!(
        request.unwrap().state,
        ChangeState::Merged {
            commit: Some(CommitId::new("89abcdef0123456789abcdef0123456789abcdef").unwrap())
        }
    );
}

#[test]
fn a_state_github_does_not_document_is_malformed_not_guessed() {
    let unknown = read_fixture("pr-view.json").replace("\"OPEN\"", "\"QUEUED\"");
    let mut gh = FakeGh::new();
    gh.answer(
        &["pr", "view", "20", REPOSITORY, PULL_FIELDS],
        0,
        &unknown,
        "",
    );

    let request = with_access(|access| {
        gh.adapter()
            .change_request(access, &ChangeId::new("20").unwrap())
    });

    assert!(
        matches!(request, Err(ForgeError::Malformed { .. })),
        "{request:?}"
    );
}

#[test]
fn an_answer_about_another_pull_request_is_malformed() {
    let other = read_fixture("pr-view.json").replace("\"number\":20", "\"number\":21");
    let mut gh = FakeGh::new();
    gh.answer(
        &["pr", "view", "20", REPOSITORY, PULL_FIELDS],
        0,
        &other,
        "",
    );

    let request = with_access(|access| {
        gh.adapter()
            .change_request(access, &ChangeId::new("20").unwrap())
    });

    assert!(
        matches!(request, Err(ForgeError::Malformed { .. })),
        "{request:?}"
    );
}

#[test]
fn an_id_github_cannot_number_is_not_found_without_running_gh() {
    let gh = FakeGh::new();

    let read = with_access(|access| {
        gh.adapter()
            .read_issue(access, &IssueId::new("PROJ-1").unwrap())
    });

    assert!(matches!(read, Err(ForgeError::NotFound { .. })), "{read:?}");
    assert!(gh.calls().is_empty());
}

#[test]
fn a_label_with_a_comma_is_refused_before_gh_splits_it() {
    let gh = FakeGh::new();
    let filter = IssueFilter {
        state: None,
        labels: vec!["needs, owner".to_owned()],
        limit: ListLimit::new(10).unwrap(),
    };

    let listed = with_access(|access| gh.adapter().list_issues(access, &filter));

    assert!(
        matches!(listed, Err(ForgeError::Refused { .. })),
        "{listed:?}"
    );
    assert!(gh.calls().is_empty());
}

#[test]
fn a_gh_error_never_repeats_the_token() {
    let mut gh = FakeGh::new();
    gh.answer(
        &["pr", "view", "20", REPOSITORY, "--json=reviewDecision"],
        1,
        "",
        &format!("error connecting with token {TOKEN}: connection refused\n"),
    );

    let review = with_access(|access| {
        gh.adapter()
            .change_review(access, &ChangeId::new("20").unwrap())
    });

    let error = review.unwrap_err().to_string();
    assert!(!error.contains(TOKEN), "{error}");
    assert!(error.contains("<token>"), "{error}");
}

#[test]
fn bad_credentials_are_a_refusal() {
    let mut gh = FakeGh::new();
    gh.fail_with(
        &["pr", "view", "20", REPOSITORY, "--json=reviewDecision"],
        "api-unauthorized.stderr",
    );

    let review = with_access(|access| {
        gh.adapter()
            .change_review(access, &ChangeId::new("20").unwrap())
    });

    assert!(
        matches!(review, Err(ForgeError::Refused { .. })),
        "{review:?}"
    );
}

#[test]
fn a_repository_off_github_is_refused() {
    let gh = FakeGh::new();
    let repository = Repository {
        host: ForgeHost::new("forge.example.com").unwrap(),
        path: RepositoryPath::new("owner/project").unwrap(),
    };
    let token = Token::new(TOKEN).unwrap();
    let access = Access {
        repository: &repository,
        token: &token,
    };

    let review = gh
        .adapter()
        .change_review(&access, &ChangeId::new("20").unwrap());

    assert!(
        matches!(review, Err(ForgeError::Refused { .. })),
        "{review:?}"
    );
    assert!(gh.calls().is_empty());
}

#[test]
fn a_repository_path_that_is_not_owner_and_name_is_refused() {
    let repository = Repository {
        host: ForgeHost::new("github.com").unwrap(),
        path: RepositoryPath::new("group/subgroup/project").unwrap(),
    };

    let checked = FakeGh::new().adapter().check_repository(&repository);

    assert!(
        matches!(checked, Err(ForgeError::Refused { .. })),
        "{checked:?}"
    );
}

fn check_state_of(entry: &str) -> CheckState {
    let answer = format!(
        r#"{{"headRefOid":"0123456789abcdef0123456789abcdef01234567","statusCheckRollup":[{entry}]}}"#
    );
    let mut gh = FakeGh::new();
    gh.answer(
        &[
            "pr",
            "view",
            "20",
            REPOSITORY,
            "--json=headRefOid,statusCheckRollup",
        ],
        0,
        &answer,
        "",
    );
    let checks = with_access(|access| {
        gh.adapter()
            .change_checks(access, &ChangeId::new("20").unwrap())
    })
    .unwrap();
    checks.checks[0].state
}

#[test]
fn each_check_run_conclusion_maps_to_a_check_state() {
    let cases = [
        ("COMPLETED", "SUCCESS", CheckState::Passed),
        ("COMPLETED", "FAILURE", CheckState::Failed),
        ("COMPLETED", "TIMED_OUT", CheckState::Failed),
        ("COMPLETED", "STARTUP_FAILURE", CheckState::Failed),
        ("COMPLETED", "ACTION_REQUIRED", CheckState::Failed),
        ("COMPLETED", "NEUTRAL", CheckState::Skipped),
        ("COMPLETED", "SKIPPED", CheckState::Skipped),
        ("COMPLETED", "CANCELLED", CheckState::Cancelled),
        ("COMPLETED", "STALE", CheckState::Pending),
        ("QUEUED", "", CheckState::Pending),
        ("IN_PROGRESS", "", CheckState::Pending),
        ("WAITING", "", CheckState::Pending),
    ];
    for (status, conclusion, expected) in cases {
        let entry = format!(
            r#"{{"__typename":"CheckRun","name":"ci","status":"{status}","conclusion":"{conclusion}"}}"#
        );

        assert_eq!(check_state_of(&entry), expected, "{status} {conclusion}");
    }
}

#[test]
fn each_commit_status_maps_to_a_check_state() {
    let cases = [
        ("SUCCESS", CheckState::Passed),
        ("FAILURE", CheckState::Failed),
        ("ERROR", CheckState::Failed),
        ("PENDING", CheckState::Pending),
        ("EXPECTED", CheckState::Pending),
    ];
    for (state, expected) in cases {
        let entry = format!(r#"{{"__typename":"StatusContext","context":"ci","state":"{state}"}}"#);

        assert_eq!(check_state_of(&entry), expected, "{state}");
    }
}

fn scope_of(token: &str, status: i32, stderr: &str) -> ScopeCheck {
    let mut gh = FakeGh::new();
    let stdout = if status == 0 { "owner/project\n" } else { "" };
    gh.answer(
        &["api", "repos/owner/project", "--jq=.full_name"],
        status,
        stdout,
        stderr,
    );
    let repository = common::repository();
    let token = Token::new(token).unwrap();
    let access = Access {
        repository: &repository,
        token: &token,
    };
    gh.adapter().check_token(&access, TokenKind::Push).unwrap()
}

#[test]
fn a_classic_token_is_refused_for_reaching_every_repository() {
    let scope = scope_of("ghp_fakeclassic", 0, "");

    assert_eq!(
        scope,
        ScopeCheck::Refused {
            problems: vec![ScopeProblem::OtherRepositories]
        }
    );
}

#[test]
fn a_fine_grained_token_is_unverified_naming_what_to_check() {
    let scope = scope_of(TOKEN, 0, "");

    let ScopeCheck::Unverified { reason } = scope else {
        panic!("{scope:?}");
    };
    assert!(reason.contains("contents write"), "{reason}");
}

#[test]
fn a_token_github_does_not_accept_is_refused() {
    let scope = scope_of(TOKEN, 1, &read_fixture("api-unauthorized.stderr"));

    assert_eq!(
        scope,
        ScopeCheck::Refused {
            problems: vec![ScopeProblem::NotAccepted]
        }
    );
}

#[test]
fn a_token_that_cannot_see_the_repository_lacks_contents_read() {
    let scope = scope_of(TOKEN, 1, &read_fixture("api-not-found.stderr"));

    assert_eq!(
        scope,
        ScopeCheck::Refused {
            problems: vec![ScopeProblem::Missing(Permission::ContentsRead)]
        }
    );
}

#[test]
fn a_forbidden_answer_to_the_scope_check_is_an_error_not_a_verdict() {
    let mut gh = FakeGh::new();
    gh.answer(
        &["api", "repos/owner/project", "--jq=.full_name"],
        1,
        "",
        "gh: API rate limit exceeded (HTTP 403)\n",
    );

    let scope = with_access(|access| gh.adapter().check_token(access, TokenKind::Api));

    assert!(
        matches!(scope, Err(ForgeError::Refused { .. })),
        "{scope:?}"
    );
}

#[test]
fn a_missing_repository_is_not_found_as_the_repository() {
    let mut gh = FakeGh::new();
    gh.answer(
        &["pr", "view", "20", REPOSITORY, "--json=reviewDecision"],
        1,
        "",
        "GraphQL: Could not resolve to a Repository with the name 'owner/project'. (repository)\n",
    );

    let review = with_access(|access| {
        gh.adapter()
            .change_review(access, &ChangeId::new("20").unwrap())
    });

    assert_eq!(
        review,
        Err(ForgeError::NotFound {
            what: "the repository".to_owned()
        })
    );
}

#[test]
fn a_configuration_folder_that_does_not_exist_is_named() {
    let gh = FakeGh::new();
    let adapter = cockpit_github::GitHub::new(gh.program(), gh.config_dir().join("absent"));

    let reason = adapter
        .capabilities()
        .withheld(cockpit_forge_api::Capability::IssueList)
        .unwrap()
        .to_owned();

    assert!(reason.contains("configuration folder"), "{reason}");
}
