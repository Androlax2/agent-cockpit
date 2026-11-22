//! The conformance suite: the fake forge conforms, and an adapter breaking a rule is reported.

mod common;

use cockpit_forge_api::conformance::{Recorded, check};
use cockpit_forge_api::fake::FakeForge;
use cockpit_forge_api::{
    Access, Capabilities, Capability, ChangeId, ChangeState, Created, Declaration, Forge,
    ForgeError, ForgeName, Issue, IssueFilter, IssueId, IssueState, ListLimit, NewChangeRequest,
    NewIssue, Posted, Provenance, ReviewDecision, ScopeCheck, TokenKind,
};
use common::{branch, change, commit, passed_checks, thread};

fn seeded() -> FakeForge {
    FakeForge::new()
        .with_issue(thread("1", IssueState::Open, &[]))
        .with_issue(thread("2", IssueState::Closed, &[]))
        .with_issue(thread("3", IssueState::Open, &[]))
        .with_change(change("10", "feature", ChangeState::Open))
        .with_checks(ChangeId::new("10").unwrap(), passed_checks())
        .with_review(ChangeId::new("10").unwrap(), ReviewDecision::Approved)
        .with_branch(branch("next"), commit('d'))
        .with_scope(TokenKind::Api, ScopeCheck::Verified)
}

/// What the seeded fake answers, written out as an adapter writes its recordings' meaning.
fn recorded(forge: &FakeForge) -> Recorded {
    let issues: Vec<Issue> = ["3", "2", "1"]
        .iter()
        .map(|id| thread(id, IssueState::Open, &[]).issue)
        .map(|mut issue| {
            if issue.id.as_str() == "2" {
                issue.state = IssueState::Closed;
            }
            issue
        })
        .collect();
    let base = "https://forge.example.com/owner/project";
    let mut recorded = Recorded::new(forge.repository().clone(), forge.token().clone());
    recorded.list_issues = Some((
        IssueFilter {
            state: None,
            labels: Vec::new(),
            limit: ListLimit::new(100).unwrap(),
        },
        issues,
    ));
    recorded.read_issue = Some((
        thread("1", IssueState::Open, &[]),
        IssueId::new("99").unwrap(),
    ));
    recorded.create_issue = Some((
        NewIssue {
            title: "new".to_owned(),
            body: "body".to_owned(),
            labels: Vec::new(),
        },
        Created {
            id: IssueId::new("11").unwrap(),
            url: format!("{base}/issues/11"),
        },
    ));
    recorded.comment_on_issue = Some((
        IssueId::new("1").unwrap(),
        "hello".to_owned(),
        Posted {
            url: format!("{base}/issues/1#comment-2"),
        },
    ));
    recorded.change_request = Some((
        change("10", "feature", ChangeState::Open),
        ChangeId::new("99").unwrap(),
        branch("no-change-here"),
    ));
    recorded.change_checks = Some((ChangeId::new("10").unwrap(), passed_checks()));
    recorded.change_review = Some((ChangeId::new("10").unwrap(), ReviewDecision::Approved));
    recorded.open_change_request = Some((
        NewChangeRequest {
            title: "next".to_owned(),
            body: String::new(),
            head: branch("next"),
            base: branch("main"),
            draft: false,
        },
        Created {
            id: ChangeId::new("12").unwrap(),
            url: format!("{base}/pulls/12"),
        },
    ));
    recorded.check_token = Some((TokenKind::Api, ScopeCheck::Verified));
    recorded
}

fn messages(forge: &dyn Forge, recorded: &Recorded) -> Vec<String> {
    check(forge, recorded)
        .into_iter()
        .map(|violation| violation.to_string())
        .collect()
}

#[test]
fn the_fake_forge_conforms() {
    let forge = seeded();

    assert_eq!(messages(&forge, &recorded(&forge)), Vec::<String>::new());
}

#[test]
fn a_capability_not_reported_conforms_with_its_typed_refusal_and_no_recording() {
    let forge = seeded().without(Capability::ChangeReview);
    let mut recorded = recorded(&forge);
    recorded.change_review = None;

    assert_eq!(messages(&forge, &recorded), Vec::<String>::new());
}

#[test]
fn a_tested_capability_without_a_recording_is_reported() {
    let forge = seeded();
    let mut recorded = recorded(&forge);
    recorded.check_token = None;

    assert_eq!(
        messages(&forge, &recorded),
        ["`token-scope` is reported tested, but no answer was recorded for it"]
    );
}

#[test]
fn a_recording_for_a_capability_not_reported_is_reported() {
    let forge = seeded().without(Capability::ChangeReview);

    assert_eq!(
        messages(&forge, &recorded(&forge)),
        ["`change-review` is not reported, but an answer was recorded for it"]
    );
}

#[test]
fn an_answer_other_than_the_recording_is_reported() {
    let forge = seeded();
    let mut recorded = recorded(&forge);
    recorded.change_review = Some((ChangeId::new("10").unwrap(), ReviewDecision::Required));

    let found = messages(&forge, &recorded);

    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].starts_with("change_review answered Approved"),
        "{found:?}"
    );
}

#[test]
fn a_recorded_repository_on_an_undeclared_host_is_reported() {
    let forge = seeded();
    let mut recorded = recorded(&forge);
    recorded.repository.host = cockpit_forge_api::ForgeHost::new("other.example.com").unwrap();

    let found = messages(&forge, &recorded);

    assert!(
        found.contains(
            &"the recorded repository's host other.example.com is not declared for Git".to_owned()
        ),
        "{found:?}"
    );
}

/// An adapter that breaks the contract in one way, over the fake.
struct Broken {
    fake: FakeForge,
    breaking: Breaking,
}

enum Breaking {
    /// Reports no review capability, yet answers the call.
    AnswersUnreported,
    /// Reports no review capability, and fails the call instead of refusing it.
    FailsUnreported,
    /// Repeats the token in an error.
    LeaksToken,
}

impl Forge for Broken {
    fn name(&self) -> &ForgeName {
        self.fake.name()
    }

    fn capabilities(&self) -> Capabilities {
        let reported = self.fake.capabilities();
        match self.breaking {
            Breaking::LeaksToken => reported,
            Breaking::AnswersUnreported | Breaking::FailsUnreported => reported
                .reported()
                .filter(|(capability, _)| *capability != Capability::ChangeReview)
                .fold(Capabilities::none(), |kept, (capability, provenance)| {
                    kept.with(capability, provenance.clone())
                }),
        }
    }

    fn declaration(&self) -> &Declaration {
        self.fake.declaration()
    }

    fn check_repository(
        &self,
        repository: &cockpit_forge_api::Repository,
    ) -> Result<(), ForgeError> {
        self.fake.check_repository(repository)
    }

    fn list_issues(
        &self,
        access: &Access<'_>,
        filter: &IssueFilter,
    ) -> Result<Vec<Issue>, ForgeError> {
        self.fake.list_issues(access, filter)
    }

    fn read_issue(
        &self,
        access: &Access<'_>,
        id: &IssueId,
    ) -> Result<cockpit_forge_api::IssueThread, ForgeError> {
        match self.breaking {
            Breaking::LeaksToken if id.as_str() == "99" => Err(ForgeError::Unavailable {
                reason: format!("request with {} failed", access.token.expose()),
            }),
            _ => self.fake.read_issue(access, id),
        }
    }

    fn create_issue(
        &self,
        access: &Access<'_>,
        issue: &NewIssue,
    ) -> Result<Created<IssueId>, ForgeError> {
        self.fake.create_issue(access, issue)
    }

    fn comment_on_issue(
        &self,
        access: &Access<'_>,
        id: &IssueId,
        body: &str,
    ) -> Result<Posted, ForgeError> {
        self.fake.comment_on_issue(access, id, body)
    }

    fn change_request(
        &self,
        access: &Access<'_>,
        id: &ChangeId,
    ) -> Result<cockpit_forge_api::ChangeRequest, ForgeError> {
        self.fake.change_request(access, id)
    }

    fn change_request_for(
        &self,
        access: &Access<'_>,
        branch: &cockpit_forge_api::BranchName,
    ) -> Result<Option<cockpit_forge_api::ChangeRequest>, ForgeError> {
        self.fake.change_request_for(access, branch)
    }

    fn change_checks(
        &self,
        access: &Access<'_>,
        id: &ChangeId,
    ) -> Result<cockpit_forge_api::Checks, ForgeError> {
        self.fake.change_checks(access, id)
    }

    fn change_review(
        &self,
        access: &Access<'_>,
        id: &ChangeId,
    ) -> Result<ReviewDecision, ForgeError> {
        match self.breaking {
            Breaking::AnswersUnreported => Ok(ReviewDecision::NotRequired),
            Breaking::FailsUnreported => Err(ForgeError::Unavailable {
                reason: "no route".to_owned(),
            }),
            Breaking::LeaksToken => self.fake.change_review(access, id),
        }
    }

    fn open_change_request(
        &self,
        access: &Access<'_>,
        request: &NewChangeRequest,
    ) -> Result<Created<ChangeId>, ForgeError> {
        self.fake.open_change_request(access, request)
    }

    fn check_token(&self, access: &Access<'_>, kind: TokenKind) -> Result<ScopeCheck, ForgeError> {
        self.fake.check_token(access, kind)
    }
}

fn broken(breaking: Breaking) -> (Broken, Recorded) {
    let fake = seeded();
    let mut recorded = recorded(&fake);
    if !matches!(breaking, Breaking::LeaksToken) {
        recorded.change_review = None;
    }
    (Broken { fake, breaking }, recorded)
}

#[test]
fn an_unreported_capability_whose_method_answers_is_reported() {
    let (forge, recorded) = broken(Breaking::AnswersUnreported);

    assert_eq!(
        messages(&forge, &recorded),
        ["`change-review` is not reported, but change_review answered"]
    );
}

#[test]
fn an_unreported_capability_whose_method_fails_instead_of_refusing_is_reported() {
    let (forge, recorded) = broken(Breaking::FailsUnreported);

    let found = messages(&forge, &recorded);

    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].ends_with("instead of the typed unsupported refusal"),
        "{found:?}"
    );
}

#[test]
fn an_error_repeating_the_token_is_reported() {
    let (forge, recorded) = broken(Breaking::LeaksToken);

    let found = messages(&forge, &recorded);

    assert!(
        found.contains(&"read_issue of a missing issue's error repeats the token".to_owned()),
        "{found:?}"
    );
}

#[test]
fn a_discovered_capability_without_a_recording_is_not_reported() {
    struct Discovered(FakeForge);
    impl Forge for Discovered {
        fn name(&self) -> &ForgeName {
            self.0.name()
        }
        fn capabilities(&self) -> Capabilities {
            self.0
                .capabilities()
                .with(Capability::TokenScope, Provenance::Discovered)
        }
        fn declaration(&self) -> &Declaration {
            self.0.declaration()
        }
    }
    let fake = FakeForge::new();
    let recorded = Recorded::new(fake.repository().clone(), fake.token().clone());
    let forge = Discovered(fake);

    let found = messages(&forge, &recorded);

    assert!(
        !found
            .iter()
            .any(|violation| violation.contains("token-scope")),
        "{found:?}"
    );
}
