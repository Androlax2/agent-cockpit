//! The GitHub adapter of the forge contract: issues as a task source, pull requests as
//! verification evidence, a scope check of the project's two tokens and the default branch's
//! protection, through the `gh` program. The hosts and credential schemes it declares are
//! #857's.

mod answers;
mod gh;
mod protection;
mod scope;

use std::path::PathBuf;

use cockpit_forge_api::{
    Access, BranchName, Capabilities, Capability, ChangeId, ChangeRequest, Checks, Created,
    CredentialScheme, CredentialUse, Declaration, DefaultBranch, Forge, ForgeError, ForgeHost,
    ForgeName, HostUse, Issue, IssueFilter, IssueId, IssueState, IssueThread, NewChangeRequest,
    NewIssue, Posted, ProtectOutcome, Protected, ProtectionPolicy, Provenance, Repository,
    ReviewDecision, ScopeCheck, TokenKind,
};

use crate::gh::{CALL_DEADLINE, Gh};

/// Where GitHub's repositories are, and Git reaches them.
pub const GIT_HOST: &str = "github.com";
/// Where GitHub's REST and GraphQL APIs are.
pub const API_HOST: &str = "api.github.com";
/// The username Git sends beside a token in Basic credentials on [`GIT_HOST`]: GitHub reads
/// the token alone.
pub const GIT_USERNAME: &str = "x-access-token";

/// The `gh` versions the conformance suite passes against with answers recorded from a live
/// `gh`; another version reports its capabilities as discovered. Empty: the suite's answers
/// are written to `gh` 2.104.0's `--json` shape by hand, since no test reaches GitHub, and a
/// hand-written answer proves the adapter reads that shape, not that `gh` prints it.
const TESTED_GH_VERSIONS: &[&str] = &[];

/// Longest `gh --version` may take.
const VERSION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// The capabilities this adapter serves.
const SERVED: [Capability; 11] = [
    Capability::IssueList,
    Capability::IssueRead,
    Capability::IssueCreate,
    Capability::IssueComment,
    Capability::ChangeState,
    Capability::ChangeChecks,
    Capability::ChangeReview,
    Capability::ChangeOpen,
    Capability::TokenScope,
    Capability::ProtectionRead,
    Capability::ProtectionBootstrap,
];

/// Pull requests a head-branch lookup reads to find the newest one from the repository
/// itself: `gh pr list --head` also lists forks' branches of the same name.
const HEAD_LOOKUP_LIMIT: usize = 30;

/// The GitHub adapter. It runs `gh` with a configuration folder of its own, so the owner's
/// `gh` sign-in is never used: every call carries the token its [`Access`] names.
#[derive(Debug)]
pub struct GitHub {
    name: ForgeName,
    gh: Gh,
    version: Result<String, String>,
    declaration: Declaration,
}

impl GitHub {
    /// The adapter over the `gh` at `program`, which keeps its configuration in `config_dir`
    /// (a folder of Torlek's state that exists, never the owner's `~/.config/gh`). Runs
    /// `gh --version` once, blocking up to 5 s, as a provider's probe runs once at start:
    /// when it fails, every capability is withheld with the reason until a new adapter is
    /// made.
    pub fn new(program: PathBuf, config_dir: PathBuf) -> GitHub {
        let gh = Gh {
            program,
            config_dir,
        };
        let version = discover_version(&gh);
        GitHub {
            name: ForgeName::new("github").expect("a valid forge name"),
            gh,
            version,
            declaration: declaration(),
        }
    }

    fn call(
        &self,
        access: &Access<'_>,
        arguments: Vec<String>,
        input: Option<&str>,
        what: &str,
    ) -> Result<String, ForgeError> {
        let answer = self
            .gh
            .run(&arguments, Some(access.token), input, CALL_DEADLINE)?;
        if !answer.success {
            return Err(gh::failure(&arguments, &answer, Some(access.token), what));
        }
        Ok(answer.stdout)
    }

    fn require(&self, capability: Capability) -> Result<(), ForgeError> {
        self.capabilities().require(capability)
    }
}

fn discover_version(gh: &Gh) -> Result<String, String> {
    let arguments = ["--version".to_owned()];
    let answer = gh
        .run(&arguments, None, None, VERSION_DEADLINE)
        .map_err(|error| error.to_string())?;
    if !answer.success {
        return Err(format!(
            "`gh --version` failed: {}",
            answer.stderr.trim().chars().take(200).collect::<String>()
        ));
    }
    gh::parse_version(&answer.stdout)
        .map(str::to_owned)
        .ok_or_else(|| "`gh --version` printed no version".to_owned())
}

/// #857: the API token reads Git on `github.com` and calls the API on `api.github.com`; the
/// push token pushes on `github.com` alone, from the daemon. What [`GitHub`] declares, without
/// running `gh`: the daemon's push policy reads it.
pub fn declaration() -> Declaration {
    let git = ForgeHost::new(GIT_HOST).expect("a valid host");
    let api = ForgeHost::new(API_HOST).expect("a valid host");
    Declaration::new(vec![
        CredentialUse {
            token: TokenKind::Api,
            host: git.clone(),
            scheme: CredentialScheme::Basic,
            used_for: HostUse::GitRead,
        },
        CredentialUse {
            token: TokenKind::Api,
            host: api,
            scheme: CredentialScheme::Bearer,
            used_for: HostUse::Api,
        },
        CredentialUse {
            token: TokenKind::Push,
            host: git,
            scheme: CredentialScheme::Basic,
            used_for: HostUse::GitPush,
        },
    ])
    .expect("GitHub's declaration is valid")
}

/// `owner/name` for `--repo`, once the repository is on GitHub.
fn repository_argument(repository: &Repository) -> Result<String, ForgeError> {
    if repository.host.as_str() != GIT_HOST {
        return Err(ForgeError::Refused {
            reason: format!("{} is not on {GIT_HOST}", repository.host),
        });
    }
    let names: Vec<&str> = repository.path.names().collect();
    if names.len() != 2 {
        return Err(ForgeError::Refused {
            reason: format!(
                "a GitHub repository is `owner/name`, not {}",
                repository.path
            ),
        });
    }
    Ok(repository.path.as_str().to_owned())
}

/// `gh` splits a list flag's value on commas, so a label with one would reach GitHub as two.
fn label_arguments(labels: &[String]) -> Result<Vec<String>, ForgeError> {
    labels
        .iter()
        .map(|label| {
            if label.contains(',') {
                return Err(ForgeError::Refused {
                    reason: format!("`gh` cannot pass a label with a comma: {label:?}"),
                });
            }
            Ok(format!("--label={label}"))
        })
        .collect()
}

impl Forge for GitHub {
    fn name(&self) -> &ForgeName {
        &self.name
    }

    fn version(&self) -> Option<&str> {
        self.version.as_deref().ok()
    }

    fn capabilities(&self) -> Capabilities {
        let version = match &self.version {
            Ok(version) => version,
            Err(reason) => {
                return SERVED
                    .iter()
                    .fold(Capabilities::none(), |capabilities, capability| {
                        capabilities.withholding(*capability, reason.clone())
                    });
            }
        };
        let provenance = if TESTED_GH_VERSIONS.contains(&version.as_str()) {
            Provenance::Tested {
                against: format!("gh {version}"),
            }
        } else {
            Provenance::Discovered
        };
        SERVED
            .iter()
            .fold(Capabilities::none(), |capabilities, capability| {
                capabilities.with(*capability, provenance.clone())
            })
    }

    fn declaration(&self) -> &Declaration {
        &self.declaration
    }

    fn check_repository(&self, repository: &Repository) -> Result<(), ForgeError> {
        repository_argument(repository).map(|_| ())
    }

    fn list_issues(
        &self,
        access: &Access<'_>,
        filter: &IssueFilter,
    ) -> Result<Vec<Issue>, ForgeError> {
        self.require(Capability::IssueList)?;
        let state = match filter.state {
            None => "all",
            Some(IssueState::Open) => "open",
            Some(IssueState::Closed) => "closed",
        };
        let mut arguments = vec![
            "issue".to_owned(),
            "list".to_owned(),
            format!("--repo={}", repository_argument(access.repository)?),
            format!("--state={state}"),
            format!("--limit={}", filter.limit.get()),
            format!("--json={}", answers::ISSUE_FIELDS),
        ];
        arguments.extend(label_arguments(&filter.labels)?);
        let stdout = self.call(access, arguments, None, "the repository")?;
        let listed = answers::parse(&stdout)?;
        let issues = answers::array(&listed, "the issue list")?
            .iter()
            .map(answers::issue)
            .collect::<Result<Vec<_>, _>>()?;
        if issues.len() > filter.limit.get() {
            return Err(ForgeError::Malformed {
                reason: format!(
                    "`gh` listed {} issues, more than the {} asked",
                    issues.len(),
                    filter.limit.get()
                ),
            });
        }
        Ok(issues)
    }

    fn read_issue(&self, access: &Access<'_>, id: &IssueId) -> Result<IssueThread, ForgeError> {
        self.require(Capability::IssueRead)?;
        let number = issue_number(id)?;
        let arguments = vec![
            "issue".to_owned(),
            "view".to_owned(),
            number.clone(),
            format!("--repo={}", repository_argument(access.repository)?),
            format!("--json={}", answers::ISSUE_THREAD_FIELDS),
        ];
        let stdout = self.call(access, arguments, None, &format!("issue {number}"))?;
        let thread = answers::issue_thread(&answers::parse(&stdout)?)?;
        if answers::github_url(&thread.issue.url, "issues").is_none() {
            return Err(not_an_issue(&number));
        }
        if thread.issue.id != *id {
            return Err(ForgeError::Malformed {
                reason: format!("asked for issue {id}, `gh` answered {}", thread.issue.id),
            });
        }
        Ok(thread)
    }

    fn create_issue(
        &self,
        access: &Access<'_>,
        issue: &NewIssue,
    ) -> Result<Created<IssueId>, ForgeError> {
        self.require(Capability::IssueCreate)?;
        let repository = repository_argument(access.repository)?;
        let mut arguments = vec![
            "issue".to_owned(),
            "create".to_owned(),
            format!("--repo={repository}"),
            format!("--title={}", issue.title),
            "--body-file=-".to_owned(),
        ];
        arguments.extend(label_arguments(&issue.labels)?);
        let stdout = self.call(access, arguments, Some(&issue.body), "the repository")?;
        let (number, url) = answers::created_number(&stdout, "issues")?;
        Ok(Created {
            id: IssueId::new(number).map_err(malformed)?,
            url,
        })
    }

    fn comment_on_issue(
        &self,
        access: &Access<'_>,
        id: &IssueId,
        body: &str,
    ) -> Result<Posted, ForgeError> {
        self.require(Capability::IssueComment)?;
        let number = issue_number(id)?;
        let repository = repository_argument(access.repository)?;
        self.require_issue(access, &number, &repository)?;
        let arguments = vec![
            "issue".to_owned(),
            "comment".to_owned(),
            number.clone(),
            format!("--repo={repository}"),
            "--body-file=-".to_owned(),
        ];
        let stdout = self.call(access, arguments, Some(body), &format!("issue {number}"))?;
        let url = answers::printed_url(&stdout)?;
        let is_comment_url = answers::github_url(url, "issues").is_some_and(|(posted_on, rest)| {
            posted_on == number
                && rest.strip_prefix("#issuecomment-").is_some_and(|id| {
                    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
                })
        });
        if !is_comment_url {
            return Err(ForgeError::Malformed {
                reason: format!("`gh` printed {url:?}, not a comment URL of issue {number}"),
            });
        }
        Ok(Posted {
            url: url.to_owned(),
        })
    }

    fn change_request(
        &self,
        access: &Access<'_>,
        id: &ChangeId,
    ) -> Result<ChangeRequest, ForgeError> {
        self.require(Capability::ChangeState)?;
        let stdout = self.view_pull(access, id, answers::PULL_FIELDS)?;
        let (request, _) = answers::pull(&answers::parse(&stdout)?)?;
        if request.id != *id {
            return Err(ForgeError::Malformed {
                reason: format!("asked for pull request {id}, `gh` answered {}", request.id),
            });
        }
        Ok(request)
    }

    fn change_request_for(
        &self,
        access: &Access<'_>,
        branch: &BranchName,
    ) -> Result<Option<ChangeRequest>, ForgeError> {
        self.require(Capability::ChangeState)?;
        let arguments = vec![
            "pr".to_owned(),
            "list".to_owned(),
            format!("--repo={}", repository_argument(access.repository)?),
            format!("--head={branch}"),
            "--state=all".to_owned(),
            format!("--limit={HEAD_LOOKUP_LIMIT}"),
            format!("--json={}", answers::PULL_FIELDS),
        ];
        let stdout = self.call(access, arguments, None, "the repository")?;
        let listed = answers::parse(&stdout)?;
        let listed = answers::array(&listed, "the pull request list")?;
        for value in listed {
            let (request, is_from_fork) = answers::pull(value)?;
            if request.head != *branch {
                return Err(ForgeError::Malformed {
                    reason: format!(
                        "asked for pull requests from {branch}, `gh` listed one from {}",
                        request.head
                    ),
                });
            }
            if !is_from_fork {
                return Ok(Some(request));
            }
        }
        if listed.len() >= HEAD_LOOKUP_LIMIT {
            return Err(ForgeError::Unavailable {
                reason: format!(
                    "the newest {HEAD_LOOKUP_LIMIT} pull requests from {branch} are all forks': \
                     the repository's own may be older"
                ),
            });
        }
        Ok(None)
    }

    fn change_checks(&self, access: &Access<'_>, id: &ChangeId) -> Result<Checks, ForgeError> {
        self.require(Capability::ChangeChecks)?;
        let stdout = self.view_pull(access, id, answers::CHECK_FIELDS)?;
        answers::checks(&answers::parse(&stdout)?)
    }

    fn change_review(
        &self,
        access: &Access<'_>,
        id: &ChangeId,
    ) -> Result<ReviewDecision, ForgeError> {
        self.require(Capability::ChangeReview)?;
        let stdout = self.view_pull(access, id, "reviewDecision")?;
        answers::review(&answers::parse(&stdout)?)
    }

    fn open_change_request(
        &self,
        access: &Access<'_>,
        request: &NewChangeRequest,
    ) -> Result<Created<ChangeId>, ForgeError> {
        self.require(Capability::ChangeOpen)?;
        let repository = repository_argument(access.repository)?;
        let mut arguments = vec![
            "pr".to_owned(),
            "create".to_owned(),
            format!("--repo={repository}"),
            format!("--title={}", request.title),
            "--body-file=-".to_owned(),
            format!("--head={}", request.head),
            format!("--base={}", request.base),
        ];
        if request.draft {
            arguments.push("--draft".to_owned());
        }
        let stdout = self.call(access, arguments, Some(&request.body), "the repository")?;
        let (number, url) = answers::created_number(&stdout, "pull")?;
        Ok(Created {
            id: ChangeId::new(number).map_err(malformed)?,
            url,
        })
    }

    fn check_token(&self, access: &Access<'_>, kind: TokenKind) -> Result<ScopeCheck, ForgeError> {
        self.require(Capability::TokenScope)?;
        let arguments = vec![
            "api".to_owned(),
            format!("repos/{}", repository_argument(access.repository)?),
            "--jq=.full_name".to_owned(),
        ];
        let answer = self
            .gh
            .run(&arguments, Some(access.token), None, CALL_DEADLINE)?;
        // Only a 401 says the token is not accepted: a 403 is also a rate limit or an
        // organization's sign-on rule, which say nothing of the token's scope.
        let reach = if answer.success {
            scope::Reach::Reached
        } else if answer.stderr.contains("HTTP 401") {
            scope::Reach::NotAccepted
        } else {
            match gh::failure(&arguments, &answer, Some(access.token), "the repository") {
                ForgeError::NotFound { .. } => scope::Reach::Hidden,
                error => return Err(error),
            }
        };
        Ok(scope::judge(access.token, kind, reach))
    }

    fn default_branch_protection(&self, access: &Access<'_>) -> Result<DefaultBranch, ForgeError> {
        self.require(Capability::ProtectionRead)?;
        let repository = repository_argument(access.repository)?;
        self.read_protection(access, &repository)
    }

    fn protect_default_branch(
        &self,
        access: &Access<'_>,
        policy: &ProtectionPolicy,
    ) -> Result<Protected, ForgeError> {
        self.require(Capability::ProtectionBootstrap)?;
        let repository = repository_argument(access.repository)?;
        let branch = self.read_default_branch(access, &repository)?;
        let listed = self.call(
            access,
            vec![
                "api".to_owned(),
                format!(
                    "repos/{repository}/rulesets?includes_parents=false&per_page={}",
                    protection::PAGE
                ),
            ],
            None,
            "the repository's rulesets",
        )?;
        let own = protection::own_ruleset(&answers::parse(&listed)?)?;
        if let Some(id) = own {
            let current = self.call(
                access,
                vec![
                    "api".to_owned(),
                    format!("repos/{repository}/rulesets/{id}"),
                ],
                None,
                "Torlek's ruleset",
            )?;
            if protection::says_policy(&answers::parse(&current)?, policy)? {
                return Ok(Protected {
                    branch,
                    outcome: ProtectOutcome::Unchanged,
                });
            }
        }
        let (method, path, outcome) = match own {
            Some(id) => (
                "PUT",
                format!("repos/{repository}/rulesets/{id}"),
                ProtectOutcome::Updated,
            ),
            None => (
                "POST",
                format!("repos/{repository}/rulesets"),
                ProtectOutcome::Created,
            ),
        };
        let arguments = vec![
            "api".to_owned(),
            format!("--method={method}"),
            path,
            "--input=-".to_owned(),
        ];
        let body = protection::ruleset_body(policy);
        let written = self.call(access, arguments, Some(&body), "the repository's rulesets")?;
        protection::check_written(&answers::parse(&written)?, own)?;
        Ok(Protected { branch, outcome })
    }
}

impl GitHub {
    /// Refuses a pull request's number before a write: `gh issue` commands take one too.
    fn require_issue(
        &self,
        access: &Access<'_>,
        number: &str,
        repository: &str,
    ) -> Result<(), ForgeError> {
        let arguments = vec![
            "issue".to_owned(),
            "view".to_owned(),
            number.to_owned(),
            format!("--repo={repository}"),
            "--json=url".to_owned(),
        ];
        let stdout = self.call(access, arguments, None, &format!("issue {number}"))?;
        let viewed = answers::parse(&stdout)?;
        let url = viewed
            .get("url")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| ForgeError::Malformed {
                reason: "the answer has no `url`".to_owned(),
            })?;
        match answers::github_url(url, "issues") {
            Some((viewed_number, "")) if viewed_number == number => Ok(()),
            Some(_) => Err(ForgeError::Malformed {
                reason: format!("asked for issue {number}, `gh` answered {url:?}"),
            }),
            None => Err(not_an_issue(number)),
        }
    }

    fn read_default_branch(
        &self,
        access: &Access<'_>,
        repository: &str,
    ) -> Result<BranchName, ForgeError> {
        let named = self.call(
            access,
            vec![
                "api".to_owned(),
                format!("repos/{repository}"),
                "--jq=.default_branch".to_owned(),
            ],
            None,
            "the repository",
        )?;
        protection::default_branch(&named)
    }

    /// The default branch, the rules its rulesets apply, and whether GitHub marks it protected,
    /// which rulesets do and classic branch protection does too. Classic protection's rules
    /// take administration to read, so only a marked branch no ruleset rule applies to is
    /// reported with protection the read cannot see: one that has both is reported by its
    /// ruleset rules alone, and may show gaps classic protection closes.
    fn read_protection(
        &self,
        access: &Access<'_>,
        repository: &str,
    ) -> Result<DefaultBranch, ForgeError> {
        let branch = self.read_default_branch(access, repository)?;
        let segment = protection::path_segment(&branch);
        let what = format!("branch {branch}");
        let listed = self.call(
            access,
            vec![
                "api".to_owned(),
                format!(
                    "repos/{repository}/rules/branches/{segment}?per_page={}",
                    protection::PAGE
                ),
            ],
            None,
            &what,
        )?;
        let listed = answers::parse(&listed)?;
        let rules = protection::rules(&listed)?;
        let has_ruleset_rules = listed.as_array().is_some_and(|rules| !rules.is_empty());
        let marked = self.call(
            access,
            vec![
                "api".to_owned(),
                format!("repos/{repository}/branches/{segment}"),
                "--jq=.protected".to_owned(),
            ],
            None,
            &what,
        )?;
        Ok(DefaultBranch {
            branch,
            rules,
            has_unread_protection: protection::is_protected(&marked)? && !has_ruleset_rules,
        })
    }

    fn view_pull(
        &self,
        access: &Access<'_>,
        id: &ChangeId,
        fields: &str,
    ) -> Result<String, ForgeError> {
        let number = pull_number(id)?;
        let arguments = vec![
            "pr".to_owned(),
            "view".to_owned(),
            number.clone(),
            format!("--repo={}", repository_argument(access.repository)?),
            format!("--json={fields}"),
        ];
        self.call(access, arguments, None, &format!("pull request {number}"))
    }
}

/// What a pull request's number is to the issue calls: no issue.
fn not_an_issue(number: &str) -> ForgeError {
    ForgeError::NotFound {
        what: format!("issue {number} (it is a pull request)"),
    }
}

fn malformed(error: cockpit_forge_api::InvalidValue) -> ForgeError {
    ForgeError::Malformed {
        reason: error.to_string(),
    }
}

/// GitHub numbers issues and pull requests; another id names none of them.
fn issue_number(id: &IssueId) -> Result<String, ForgeError> {
    number(id.as_str()).ok_or_else(|| ForgeError::NotFound {
        what: format!("issue {id}"),
    })
}

fn pull_number(id: &ChangeId) -> Result<String, ForgeError> {
    number(id.as_str()).ok_or_else(|| ForgeError::NotFound {
        what: format!("pull request {id}"),
    })
}

fn number(id: &str) -> Option<String> {
    let is_number =
        !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()) && !id.starts_with('0');
    is_number.then(|| id.to_owned())
}
