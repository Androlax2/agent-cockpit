//! An in-memory forge for other crates' tests (feature `fake`): one repository, one token,
//! issues and change requests seeded by the test, every contract method but none reaching a
//! network. It passes the conformance suite like any adapter.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::{
    Access, BranchName, Capabilities, Capability, ChangeId, ChangeRequest, ChangeState, Checks,
    CommitId, Created, CredentialScheme, CredentialUse, Declaration, Forge, ForgeError, ForgeHost,
    ForgeName, HostUse, Issue, IssueFilter, IssueId, IssueState, IssueThread, NewChangeRequest,
    NewIssue, Posted, Provenance, Repository, RepositoryPath, ReviewDecision, ScopeCheck, Token,
    TokenKind,
};

/// The fake's Git host.
pub const GIT_HOST: &str = "forge.example.com";
pub const API_HOST: &str = "api.forge.example.com";
/// The one repository it serves.
pub const REPOSITORY_PATH: &str = "owner/project";
/// The one token it accepts.
pub const TOKEN: &str = "fake-forge-token";

/// The capabilities with a contract method; the fake reports each, tested.
const METHOD_CAPABILITIES: [Capability; 9] = [
    Capability::IssueList,
    Capability::IssueRead,
    Capability::IssueCreate,
    Capability::IssueComment,
    Capability::ChangeState,
    Capability::ChangeChecks,
    Capability::ChangeReview,
    Capability::ChangeOpen,
    Capability::TokenScope,
];

pub struct FakeForge {
    name: ForgeName,
    repository: Repository,
    token: Token,
    declaration: Declaration,
    capabilities: Capabilities,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    issues: Vec<IssueThread>,
    changes: Vec<ChangeRequest>,
    checks: BTreeMap<ChangeId, Checks>,
    reviews: BTreeMap<ChangeId, ReviewDecision>,
    branches: BTreeMap<BranchName, CommitId>,
    scopes: BTreeMap<TokenKind, ScopeCheck>,
    calls: Vec<&'static str>,
}

impl Default for FakeForge {
    fn default() -> FakeForge {
        FakeForge::new()
    }
}

impl FakeForge {
    pub fn new() -> FakeForge {
        let host = |name: &str| ForgeHost::new(name).expect("the fake's hosts are valid");
        let declaration = Declaration::new(vec![
            CredentialUse {
                token: TokenKind::Api,
                host: host(GIT_HOST),
                scheme: CredentialScheme::Basic,
                used_for: HostUse::GitRead,
            },
            CredentialUse {
                token: TokenKind::Push,
                host: host(GIT_HOST),
                scheme: CredentialScheme::Basic,
                used_for: HostUse::GitPush,
            },
            CredentialUse {
                token: TokenKind::Api,
                host: host(API_HOST),
                scheme: CredentialScheme::Bearer,
                used_for: HostUse::Api,
            },
        ])
        .expect("the fake's declaration is valid");
        let capabilities = METHOD_CAPABILITIES.into_iter().fold(
            Capabilities::none(),
            |capabilities, capability| {
                capabilities.with(
                    capability,
                    Provenance::Tested {
                        against: "fake".to_owned(),
                    },
                )
            },
        );
        FakeForge {
            name: ForgeName::new("fake").expect("the fake's name is valid"),
            repository: Repository {
                host: host(GIT_HOST),
                path: RepositoryPath::new(REPOSITORY_PATH).expect("the fake's path is valid"),
            },
            token: Token::new(TOKEN).expect("the fake's token is valid"),
            declaration,
            capabilities,
            state: Mutex::new(State::default()),
        }
    }

    /// Stops reporting `capability`: its method then answers unsupported.
    pub fn without(mut self, capability: Capability) -> FakeForge {
        let kept = self
            .capabilities
            .reported()
            .filter(|(reported, _)| *reported != capability)
            .fold(Capabilities::none(), |kept, (reported, provenance)| {
                kept.with(reported, provenance.clone())
            });
        self.capabilities = kept;
        self
    }

    pub fn repository(&self) -> &Repository {
        &self.repository
    }

    pub fn token(&self) -> &Token {
        &self.token
    }

    /// The repository and token the fake accepts.
    pub fn access(&self) -> Access<'_> {
        Access {
            repository: &self.repository,
            token: &self.token,
        }
    }

    pub fn with_issue(self, thread: IssueThread) -> FakeForge {
        self.state().issues.push(thread);
        self
    }

    /// Adds a change request; a later one with the same head is newer.
    pub fn with_change(self, request: ChangeRequest) -> FakeForge {
        {
            let mut state = self.state();
            state
                .branches
                .insert(request.head.clone(), request.head_commit.clone());
            state.changes.push(request);
        }
        self
    }

    pub fn with_checks(self, id: ChangeId, checks: Checks) -> FakeForge {
        self.state().checks.insert(id, checks);
        self
    }

    pub fn with_review(self, id: ChangeId, decision: ReviewDecision) -> FakeForge {
        self.state().reviews.insert(id, decision);
        self
    }

    /// A branch on the forge at `commit`, which a change request may be opened from.
    pub fn with_branch(self, branch: BranchName, commit: CommitId) -> FakeForge {
        self.state().branches.insert(branch, commit);
        self
    }

    /// What a scope check of `kind` answers; without one it answers unverified.
    pub fn with_scope(self, kind: TokenKind, check: ScopeCheck) -> FakeForge {
        self.state().scopes.insert(kind, check);
        self
    }

    /// The contract methods called so far, in order.
    pub fn calls(&self) -> Vec<&'static str> {
        self.state().calls.clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records the call and refuses what the real forge would: a capability not reported,
    /// another repository, another token.
    fn enter(
        &self,
        call: &'static str,
        capability: Capability,
        access: &Access<'_>,
    ) -> Result<MutexGuard<'_, State>, ForgeError> {
        let mut state = self.state();
        state.calls.push(call);
        self.capabilities.require(capability)?;
        if access.repository != &self.repository {
            return Err(ForgeError::NotFound {
                what: format!("repository {}", access.repository),
            });
        }
        if access.token != &self.token {
            return Err(ForgeError::Refused {
                reason: "the token is not accepted".to_owned(),
            });
        }
        Ok(state)
    }

    fn url(&self, kind: &str, id: &str) -> String {
        format!("https://{}/{kind}/{id}", self.repository)
    }
}

/// One more than the highest numbered issue or change request: they share one sequence, as
/// on most forges.
fn next_id(state: &State) -> String {
    let issues = state.issues.iter().map(|thread| thread.issue.id.as_str());
    let changes = state.changes.iter().map(|request| request.id.as_str());
    let highest = issues
        .chain(changes)
        .filter_map(|id| id.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    (highest + 1).to_string()
}

impl Forge for FakeForge {
    fn name(&self) -> &ForgeName {
        &self.name
    }

    fn version(&self) -> Option<&str> {
        Some("fake")
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }

    fn declaration(&self) -> &Declaration {
        &self.declaration
    }

    fn check_repository(&self, repository: &Repository) -> Result<(), ForgeError> {
        if repository.path.names().count() != 2 {
            return Err(ForgeError::Refused {
                reason: "a repository path is `owner/name`".to_owned(),
            });
        }
        Ok(())
    }

    fn list_issues(
        &self,
        access: &Access<'_>,
        filter: &IssueFilter,
    ) -> Result<Vec<Issue>, ForgeError> {
        let state = self.enter("list_issues", Capability::IssueList, access)?;
        Ok(state
            .issues
            .iter()
            .rev()
            .map(|thread| &thread.issue)
            .filter(|issue| filter.state.is_none_or(|state| issue.state == state))
            .filter(|issue| {
                filter
                    .labels
                    .iter()
                    .all(|label| issue.labels.contains(label))
            })
            .take(filter.limit.get())
            .cloned()
            .collect())
    }

    fn read_issue(&self, access: &Access<'_>, id: &IssueId) -> Result<IssueThread, ForgeError> {
        let state = self.enter("read_issue", Capability::IssueRead, access)?;
        state
            .issues
            .iter()
            .find(|thread| &thread.issue.id == id)
            .cloned()
            .ok_or_else(|| ForgeError::NotFound {
                what: format!("issue {id}"),
            })
    }

    fn create_issue(
        &self,
        access: &Access<'_>,
        issue: &NewIssue,
    ) -> Result<Created<IssueId>, ForgeError> {
        let mut state = self.enter("create_issue", Capability::IssueCreate, access)?;
        let number = next_id(&state);
        let id = IssueId::new(number.clone()).expect("a number is an issue id");
        let url = self.url("issues", &number);
        state.issues.push(IssueThread {
            issue: Issue {
                id: id.clone(),
                title: issue.title.clone(),
                state: IssueState::Open,
                labels: issue.labels.clone(),
                author: Some("torlek".to_owned()),
                url: url.clone(),
            },
            body: issue.body.clone(),
            comments: Vec::new(),
        });
        Ok(Created { id, url })
    }

    fn comment_on_issue(
        &self,
        access: &Access<'_>,
        id: &IssueId,
        body: &str,
    ) -> Result<Posted, ForgeError> {
        let mut state = self.enter("comment_on_issue", Capability::IssueComment, access)?;
        let Some(thread) = state
            .issues
            .iter_mut()
            .find(|thread| &thread.issue.id == id)
        else {
            return Err(ForgeError::NotFound {
                what: format!("issue {id}"),
            });
        };
        thread.comments.push(crate::Comment {
            author: Some("torlek".to_owned()),
            body: body.to_owned(),
        });
        let url = format!("{}#comment-{}", thread.issue.url, thread.comments.len());
        Ok(Posted { url })
    }

    fn change_request(
        &self,
        access: &Access<'_>,
        id: &ChangeId,
    ) -> Result<ChangeRequest, ForgeError> {
        let state = self.enter("change_request", Capability::ChangeState, access)?;
        find_change(&state, id).cloned()
    }

    fn change_request_for(
        &self,
        access: &Access<'_>,
        branch: &BranchName,
    ) -> Result<Option<ChangeRequest>, ForgeError> {
        let state = self.enter("change_request_for", Capability::ChangeState, access)?;
        Ok(state
            .changes
            .iter()
            .rev()
            .find(|request| &request.head == branch)
            .cloned())
    }

    fn change_checks(&self, access: &Access<'_>, id: &ChangeId) -> Result<Checks, ForgeError> {
        let state = self.enter("change_checks", Capability::ChangeChecks, access)?;
        let request = find_change(&state, id)?;
        Ok(state.checks.get(id).cloned().unwrap_or_else(|| Checks {
            commit: request.head_commit.clone(),
            checks: Vec::new(),
        }))
    }

    fn change_review(
        &self,
        access: &Access<'_>,
        id: &ChangeId,
    ) -> Result<ReviewDecision, ForgeError> {
        let state = self.enter("change_review", Capability::ChangeReview, access)?;
        find_change(&state, id)?;
        Ok(state
            .reviews
            .get(id)
            .copied()
            .unwrap_or(ReviewDecision::NotRequired))
    }

    fn open_change_request(
        &self,
        access: &Access<'_>,
        request: &NewChangeRequest,
    ) -> Result<Created<ChangeId>, ForgeError> {
        let mut state = self.enter("open_change_request", Capability::ChangeOpen, access)?;
        let Some(head_commit) = state.branches.get(&request.head).cloned() else {
            return Err(ForgeError::Refused {
                reason: format!("the forge has no branch {}", request.head),
            });
        };
        let number = next_id(&state);
        let id = ChangeId::new(number.clone()).expect("a number is a change request id");
        let url = self.url("pulls", &number);
        state.changes.push(ChangeRequest {
            id: id.clone(),
            title: request.title.clone(),
            url: url.clone(),
            state: ChangeState::Open,
            draft: request.draft,
            head: request.head.clone(),
            head_commit,
            base: request.base.clone(),
        });
        Ok(Created { id, url })
    }

    fn check_token(&self, access: &Access<'_>, kind: TokenKind) -> Result<ScopeCheck, ForgeError> {
        let state = self.enter("check_token", Capability::TokenScope, access)?;
        Ok(state
            .scopes
            .get(&kind)
            .cloned()
            .unwrap_or_else(|| ScopeCheck::Unverified {
                reason: "the fake forge was told nothing about this token".to_owned(),
            }))
    }
}

fn find_change<'state>(
    state: &'state State,
    id: &ChangeId,
) -> Result<&'state ChangeRequest, ForgeError> {
    state
        .changes
        .iter()
        .find(|request| &request.id == id)
        .ok_or_else(|| ForgeError::NotFound {
            what: format!("change request {id}"),
        })
}
