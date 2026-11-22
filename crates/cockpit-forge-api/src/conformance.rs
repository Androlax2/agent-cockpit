//! The conformance suite every forge adapter runs (feature `conformance`), as #51 does for
//! providers. An adapter's test points it at recorded answers of the real forge (a fake
//! command-line program replaying them, a local server) and says what those answers mean in
//! the contract's words; [`check`] calls every contract method and lists each way the adapter
//! breaks the contract.
//!
//! - A capability reported `tested` has a recorded answer, and the method returns it.
//! - A capability reported `discovered` is checked when an answer was recorded.
//! - A capability not reported answers with the typed unsupported refusal, never a failed
//!   call, and has no recorded answer (a stale fixture is a violation too).
//! - The repository's host is one the adapter declares for Git, the adapter accepts the
//!   repository, and [`crate::resolve`] finds it from the repository's HTTPS URL.
//! - No error repeats the token.
//!
//! Recorded reads run first, then recorded writes, then the calls to methods whose capability
//! is not reported, so recorded reads see the repository as recorded.

use std::fmt;

use crate::{
    Access, BranchName, Capabilities, Capability, ChangeId, ChangeRequest, Checks, Created, Forge,
    ForgeError, Issue, IssueFilter, IssueId, IssueThread, ListLimit, NewChangeRequest, NewIssue,
    Posted, Provenance, Repository, ReviewDecision, ScopeCheck, Token, TokenKind, resolve,
};

/// What the recorded answers hold, in the contract's words. Leave out what was not recorded.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub repository: Repository,
    /// The token the calls are made with.
    pub token: Token,
    /// [`Capability::IssueList`]: a filter and the issues it lists.
    pub list_issues: Option<(IssueFilter, Vec<Issue>)>,
    /// [`Capability::IssueRead`]: an issue read by its id, and an id with no issue.
    pub read_issue: Option<(IssueThread, IssueId)>,
    /// [`Capability::IssueCreate`].
    pub create_issue: Option<(NewIssue, Created<IssueId>)>,
    /// [`Capability::IssueComment`]: the issue, the comment and where it was posted.
    pub comment_on_issue: Option<(IssueId, String, Posted)>,
    /// [`Capability::ChangeState`]: a change request read by its id and by its head, an id
    /// with no change request, and a branch with none.
    pub change_request: Option<(ChangeRequest, ChangeId, BranchName)>,
    /// [`Capability::ChangeChecks`].
    pub change_checks: Option<(ChangeId, Checks)>,
    /// [`Capability::ChangeReview`].
    pub change_review: Option<(ChangeId, ReviewDecision)>,
    /// [`Capability::ChangeOpen`].
    pub open_change_request: Option<(NewChangeRequest, Created<ChangeId>)>,
    /// [`Capability::TokenScope`].
    pub check_token: Option<(TokenKind, ScopeCheck)>,
}

impl Recorded {
    /// Nothing recorded but the repository and the token.
    pub fn new(repository: Repository, token: Token) -> Recorded {
        Recorded {
            repository,
            token,
            list_issues: None,
            read_issue: None,
            create_issue: None,
            comment_on_issue: None,
            change_request: None,
            change_checks: None,
            change_review: None,
            open_change_request: None,
            check_token: None,
        }
    }

    fn has_answer(&self, capability: Capability) -> Option<bool> {
        Some(match capability {
            Capability::IssueList => self.list_issues.is_some(),
            Capability::IssueRead => self.read_issue.is_some(),
            Capability::IssueCreate => self.create_issue.is_some(),
            Capability::IssueComment => self.comment_on_issue.is_some(),
            Capability::ChangeState => self.change_request.is_some(),
            Capability::ChangeChecks => self.change_checks.is_some(),
            Capability::ChangeReview => self.change_review.is_some(),
            Capability::ChangeOpen => self.open_change_request.is_some(),
            Capability::TokenScope => self.check_token.is_some(),
            // No contract method yet (#695): nothing to record or call.
            Capability::ProtectionRead | Capability::ProtectionBootstrap => return None,
        })
    }
}

/// One way the adapter breaks the contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation(pub String);

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Every way `forge` breaks the contract against `recorded`; empty when it conforms.
pub fn check(forge: &dyn Forge, recorded: &Recorded) -> Vec<Violation> {
    let capabilities = forge.capabilities();
    let mut suite = Suite {
        violations: Vec::new(),
        token: recorded.token.expose().to_owned(),
        capabilities: capabilities.clone(),
    };
    suite.check_declaration(forge, &recorded.repository);
    let access = Access {
        repository: &recorded.repository,
        token: &recorded.token,
    };
    let mut unreported = Vec::new();
    for capability in Capability::ALL {
        let Some(has_answer) = recorded.has_answer(capability) else {
            continue;
        };
        match capabilities.provenance(capability) {
            None => {
                if has_answer {
                    suite.fail(format!(
                        "`{capability}` is not reported, but an answer was recorded for it"
                    ));
                }
                unreported.push(capability);
            }
            Some(Provenance::Tested { .. }) if !has_answer => suite.fail(format!(
                "`{capability}` is reported tested, but no answer was recorded for it"
            )),
            Some(_) => {}
        }
    }
    suite.check_reads(forge, &access, recorded);
    suite.check_writes(forge, &access, recorded);
    for capability in unreported {
        suite.check_unsupported(forge, &access, capability);
    }
    suite.violations
}

struct Suite {
    violations: Vec<Violation>,
    token: String,
    capabilities: Capabilities,
}

impl Suite {
    /// The recorded answer of `capability`, when the adapter reports it: a recording for one
    /// it does not report was already a violation.
    fn recorded<'a, T>(&self, capability: Capability, answer: &'a Option<T>) -> Option<&'a T> {
        answer
            .as_ref()
            .filter(|_| self.capabilities.supports(capability))
    }

    /// Records a violation with the token masked, so a leak the suite finds never reaches
    /// the test's output.
    fn fail(&mut self, violation: String) {
        self.violations
            .push(Violation(violation.replace(&self.token, "<token>")));
    }

    /// The answer, when the call succeeded; a failure, with a leaked token, is recorded.
    fn answer<T>(&mut self, call: &str, result: Result<T, ForgeError>) -> Option<T> {
        match result {
            Ok(answer) => Some(answer),
            Err(error) => {
                self.check_error(call, &error);
                self.fail(format!("{call} failed: {error}"));
                None
            }
        }
    }

    fn check_error(&mut self, call: &str, error: &ForgeError) {
        if format!("{error} {error:?}").contains(&self.token) {
            self.fail(format!("{call}'s error repeats the token"));
        }
    }

    fn expect<T: PartialEq + fmt::Debug>(&mut self, call: &str, actual: Option<T>, expected: &T) {
        if let Some(actual) = actual
            && &actual != expected
        {
            self.fail(format!(
                "{call} answered {actual:?}, the recording means {expected:?}"
            ));
        }
    }

    fn expect_not_found<T: fmt::Debug>(&mut self, call: &str, result: Result<T, ForgeError>) {
        match result {
            Err(ForgeError::NotFound { .. }) => {}
            Err(error) => {
                self.check_error(call, &error);
                self.fail(format!("{call} answered {error:?}, not `NotFound`"));
            }
            Ok(answer) => self.fail(format!("{call} answered {answer:?}, not `NotFound`")),
        }
    }

    fn check_declaration(&mut self, forge: &dyn Forge, repository: &Repository) {
        if !forge.declaration().git_hosts().contains(&repository.host) {
            self.fail(format!(
                "the recorded repository's host {} is not declared for Git",
                repository.host
            ));
            return;
        }
        if let Err(error) = forge.check_repository(repository) {
            self.check_error("check_repository", &error);
            self.fail(format!(
                "the adapter refuses the recorded repository: {error}"
            ));
            return;
        }
        let origin = format!("https://{repository}.git");
        match resolve(&[forge], &origin) {
            Ok((_, resolved)) if &resolved == repository => {}
            Ok((_, resolved)) => self.fail(format!("{origin} resolves to {resolved}")),
            Err(unresolved) => self.fail(format!("{origin} does not resolve: {unresolved}")),
        }
    }

    fn check_unsupported(
        &mut self,
        forge: &dyn Forge,
        access: &Access<'_>,
        capability: Capability,
    ) {
        for (method, result) in call_with_placeholders(forge, access, capability) {
            let error = match result {
                Ok(()) => {
                    self.fail(format!(
                        "`{capability}` is not reported, but {method} answered"
                    ));
                    continue;
                }
                Err(error) => error,
            };
            self.check_error(method, &error);
            let names_capability = matches!(
                &error,
                ForgeError::Unsupported { capability: named }
                    | ForgeError::Withheld { capability: named, .. } if *named == capability
            );
            if !names_capability {
                self.fail(format!(
                    "`{capability}` is not reported, but {method} answered {error:?} instead \
                     of the typed unsupported refusal"
                ));
            }
        }
    }

    fn check_reads(&mut self, forge: &dyn Forge, access: &Access<'_>, recorded: &Recorded) {
        if let Some((filter, listed)) = self.recorded(Capability::IssueList, &recorded.list_issues)
        {
            let answer = self.answer("list_issues", forge.list_issues(access, filter));
            if let Some(issues) = &answer
                && issues.len() > filter.limit.get()
            {
                self.fail(format!(
                    "list_issues returned {} issues, over its limit of {}",
                    issues.len(),
                    filter.limit.get()
                ));
            }
            self.expect("list_issues", answer, listed);
        }
        if let Some((thread, missing)) = self.recorded(Capability::IssueRead, &recorded.read_issue)
        {
            let answer = self.answer("read_issue", forge.read_issue(access, &thread.issue.id));
            self.expect("read_issue", answer, thread);
            self.expect_not_found(
                "read_issue of a missing issue",
                forge.read_issue(access, missing),
            );
        }
        if let Some((request, missing, no_change_branch)) =
            self.recorded(Capability::ChangeState, &recorded.change_request)
        {
            let answer = self.answer("change_request", forge.change_request(access, &request.id));
            self.expect("change_request", answer, request);
            let by_branch = self.answer(
                "change_request_for",
                forge.change_request_for(access, &request.head),
            );
            self.expect("change_request_for", by_branch, &Some(request.clone()));
            let none = self.answer(
                "change_request_for a branch with none",
                forge.change_request_for(access, no_change_branch),
            );
            self.expect("change_request_for a branch with none", none, &None);
            self.expect_not_found(
                "change_request of a missing change request",
                forge.change_request(access, missing),
            );
        }
        if let Some((id, checks)) = self.recorded(Capability::ChangeChecks, &recorded.change_checks)
        {
            let answer = self.answer("change_checks", forge.change_checks(access, id));
            self.expect("change_checks", answer, checks);
        }
        if let Some((id, decision)) =
            self.recorded(Capability::ChangeReview, &recorded.change_review)
        {
            let answer = self.answer("change_review", forge.change_review(access, id));
            self.expect("change_review", answer, decision);
        }
        if let Some((kind, scope)) = self.recorded(Capability::TokenScope, &recorded.check_token) {
            let answer = self.answer("check_token", forge.check_token(access, *kind));
            self.expect("check_token", answer, scope);
        }
    }

    fn check_writes(&mut self, forge: &dyn Forge, access: &Access<'_>, recorded: &Recorded) {
        if let Some((issue, created)) =
            self.recorded(Capability::IssueCreate, &recorded.create_issue)
        {
            let answer = self.answer("create_issue", forge.create_issue(access, issue));
            self.expect("create_issue", answer, created);
        }
        if let Some((id, body, posted)) =
            self.recorded(Capability::IssueComment, &recorded.comment_on_issue)
        {
            let answer = self.answer("comment_on_issue", forge.comment_on_issue(access, id, body));
            self.expect("comment_on_issue", answer, posted);
        }
        if let Some((request, created)) =
            self.recorded(Capability::ChangeOpen, &recorded.open_change_request)
        {
            let answer = self.answer(
                "open_change_request",
                forge.open_change_request(access, request),
            );
            self.expect("open_change_request", answer, created);
        }
    }
}

/// Calls each method of `capability` with placeholder arguments, to see how it refuses.
fn call_with_placeholders(
    forge: &dyn Forge,
    access: &Access<'_>,
    capability: Capability,
) -> Vec<(&'static str, Result<(), ForgeError>)> {
    let issue = IssueId::new("1").expect("a placeholder issue id");
    let change = ChangeId::new("1").expect("a placeholder change request id");
    let branch = BranchName::new("main").expect("a placeholder branch");
    match capability {
        Capability::IssueList => {
            let filter = IssueFilter {
                state: None,
                labels: Vec::new(),
                limit: ListLimit::new(1).expect("a placeholder limit"),
            };
            vec![("list_issues", forge.list_issues(access, &filter).map(drop))]
        }
        Capability::IssueRead => vec![("read_issue", forge.read_issue(access, &issue).map(drop))],
        Capability::IssueCreate => {
            let new_issue = NewIssue {
                title: "conformance".to_owned(),
                body: String::new(),
                labels: Vec::new(),
            };
            vec![(
                "create_issue",
                forge.create_issue(access, &new_issue).map(drop),
            )]
        }
        Capability::IssueComment => vec![(
            "comment_on_issue",
            forge
                .comment_on_issue(access, &issue, "conformance")
                .map(drop),
        )],
        Capability::ChangeState => vec![
            (
                "change_request",
                forge.change_request(access, &change).map(drop),
            ),
            (
                "change_request_for",
                forge.change_request_for(access, &branch).map(drop),
            ),
        ],
        Capability::ChangeChecks => vec![(
            "change_checks",
            forge.change_checks(access, &change).map(drop),
        )],
        Capability::ChangeReview => vec![(
            "change_review",
            forge.change_review(access, &change).map(drop),
        )],
        Capability::ChangeOpen => {
            let request = NewChangeRequest {
                title: "conformance".to_owned(),
                body: String::new(),
                head: branch.clone(),
                base: branch,
                draft: true,
            };
            vec![(
                "open_change_request",
                forge.open_change_request(access, &request).map(drop),
            )]
        }
        Capability::TokenScope => vec![(
            "check_token",
            forge.check_token(access, TokenKind::Api).map(drop),
        )],
        Capability::ProtectionRead | Capability::ProtectionBootstrap => Vec::new(),
    }
}
