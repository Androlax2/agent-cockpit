//! Pushing a checkout's branch under the host-Git safety rules.

use cockpit_forge_api::{ForgeHost, Token};
use cockpit_turso::sql::{Connection, params};
use cockpit_turso::{Database, Migration, WriteTransaction};

use crate::git::{arg, git_failure};
use crate::push_range::{self, RemoteAccess};
use crate::store::{self, now};
use crate::{CheckoutId, CheckoutState, Error, Git, ProjectName, blocking};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub branch: String,
    /// The commit sent: the branch's tip when the push began, whatever it became since.
    pub commit: String,
    /// The URL Git pushed to, after rewrites.
    pub remote_url: String,
    /// Whether Git forced the update, under a lease: `false` for a fast-forward, lease or not.
    pub forced: bool,
}

/// The table that records the commit each daemon push sent, per
/// checkout and branch: the expected tip of a lease (#857 decision 2). Only a push that
/// succeeded is recorded, so a record never names a commit the remote did not take.
pub(crate) const MIGRATION: Migration = Migration {
    version: 5,
    sql: "CREATE TABLE checkout_pushes (
        checkout_id INTEGER NOT NULL,
        branch TEXT NOT NULL CHECK (branch <> ''),
        commit_id TEXT NOT NULL CHECK (commit_id <> ''),
        remote_url TEXT NOT NULL CHECK (remote_url <> ''),
        pushed_at INTEGER NOT NULL,
        PRIMARY KEY (checkout_id, branch)
    );",
};

/// Checkout settings that send a push, or the sweep's question to origin, elsewhere or change
/// how it gets there: every HTTP setting, since GitHub needs none from the checkout.
const REDIRECTS: &str =
    r"^(remote\.origin\.(pushurl|proxy|vcs)|url\..*\.(push)?insteadof|http\..*)$";

/// How long a push may take before Git is killed and the push fails: a stalled connection to
/// the forge must not hold the request for ever.
const PUSH_LIMIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// The variable a push's credential helper reads the push token from.
const PUSH_TOKEN_VARIABLE: &str = "TORLEK_PUSH_TOKEN";

/// What a push authenticates with: the project's push token (#857), sent by Git as Basic
/// credentials under `username` to the push host alone.
#[derive(Debug, Clone)]
pub struct PushCredential {
    username: String,
    token: Token,
}

impl PushCredential {
    /// Refuses a `username` that is not one plain word, since it is written into Git's
    /// configuration for the push.
    pub fn new(username: &str, token: Token) -> Result<PushCredential, Error> {
        let is_word = !username.is_empty()
            && username
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte));
        if !is_word {
            return Err(Error::Refused {
                reason: format!(
                    "{username:?} is no username a push sends: letters, digits, `-`, `_` and \
                     `.` only"
                ),
            });
        }
        Ok(PushCredential {
            username: username.to_owned(),
            token,
        })
    }
}

/// Whether a push may rewrite the remote branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Force {
    /// Never: Git refuses anything but a fast-forward.
    Never,
    /// A lease-protected force on `own_branch` alone (#857 decision 2): the remote branch must
    /// still be at the commit the daemon last pushed there from this checkout.
    Lease { own_branch: String },
}

/// What a push may do and with what.
#[derive(Debug, Clone)]
pub struct PushRules {
    /// The hosts the checkout's project may push to (#153, #857 decision 3): those the owner
    /// bound the push right on that a forge adapter declares for Git pushes. The daemon finds
    /// them; none until the owner binds one, since no grant was given the right on upgrade.
    pub hosts: Vec<ForgeHost>,
    /// What the push authenticates with, or why the project has none: that refusal comes once
    /// every check on the checkout passed, so it never hides why its origin is refused.
    pub credential: Result<PushCredential, String>,
    pub force: Force,
    pub workflows: WorkflowCheck,
}

/// Whether a push's range is scanned for CI workflow changes (#857): an agent's push needs the
/// owner's approval for one, the owner's own does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowCheck {
    /// The owner pushes: no scan.
    Owner,
    /// An agent pushes: the range is read against the remote, and one that changes a workflow
    /// file is refused as [`Error::WorkflowChange`] unless the owner approved exactly the
    /// commit sent.
    Agent { approved_commit: Option<String> },
}

/// Pushes the commit the checkout's current branch points at to the branch of the same name on
/// origin, setting it as upstream, authenticated with the rules' credential on the push host
/// alone, and records the commit sent.
///
/// The branch is resolved to one commit id first and the push is `<id>:refs/heads/<branch>`,
/// so moving the branch meanwhile changes nothing that is sent. No tag and no deletion can be
/// expressed. Refuses a detached HEAD, an origin with more than one URL (as configured and
/// after rewrites), any of those URLs that is not an `https` repository URL on one of the
/// rules' hosts (an SSH URL among them, named as such), and a checkout whose own configuration
/// redirects pushes (`remote.origin.pushurl`, `.proxy` or `.vcs`, `url.*.insteadOf`,
/// `url.*.pushInsteadOf`, any `http.*` setting). A refusal for a host without the right names
/// the owner's command that binds it. A non-fast-forward is refused by Git, unless the rules
/// allow a lease on this branch and a push from this checkout to the same URL is recorded for
/// it: then the push is forced only while the remote branch is still at that commit.
pub async fn push(
    database: &Database,
    git: &Git,
    id: CheckoutId,
    rules: PushRules,
) -> Result<Pushed, Error> {
    let stored = {
        let read = database.read().await?;
        store::stored_checkout(&read, id).await?
    };
    if stored.checkout.state != CheckoutState::Live {
        return Err(Error::BeingRemoved { checkout: id });
    }
    let checkout = stored.checkout.path.clone();
    let target = {
        let git = git.clone();
        let hosts = rules.hosts.clone();
        blocking(move || {
            stored.verify_on_disk_now()?;
            push_target(
                &git,
                &stored.checkout.path,
                &stored.checkout.project,
                &hosts,
            )
        })
        .await?
    };
    let lease = match &rules.force {
        Force::Never => None,
        Force::Lease { own_branch } => {
            let read = database.read().await?;
            Some(lease(&read, id, &target, own_branch).await?)
        }
    };
    let credential = rules
        .credential
        .map_err(|reason| Error::Refused { reason })?;
    if let WorkflowCheck::Agent { approved_commit } = &rules.workflows {
        let git = git.clone();
        let (scanned_checkout, scanned, scan_credential) =
            (checkout.clone(), target.clone(), credential.clone());
        let paths =
            blocking(move || workflow_changes(&git, &scanned_checkout, &scanned, &scan_credential))
                .await?;
        let is_approved = approved_commit.as_deref() == Some(target.commit.as_str());
        if !paths.is_empty() && !is_approved {
            return Err(Error::WorkflowChange {
                branch: target.branch,
                commit: target.commit,
                paths,
            });
        }
    }
    let git = git.clone();
    let sent = target.clone();
    let forced =
        blocking(move || send(&git, &checkout, &sent, &credential, lease.as_deref())).await?;
    let write = database.write().await?;
    record_push(&write, id, &target).await?;
    write.commit().await?;
    Ok(Pushed {
        branch: target.branch,
        commit: target.commit,
        remote_url: target.remote_url,
        forced,
    })
}

/// The commit the daemon last pushed to `branch` from `checkout`, with the URL it went to;
/// `None` before any.
pub async fn last_pushed(
    connection: &Connection,
    checkout: CheckoutId,
    branch: &str,
) -> Result<Option<(String, String)>, Error> {
    let mut rows = connection
        .query(
            "SELECT commit_id, remote_url FROM checkout_pushes
             WHERE checkout_id = ?1 AND branch = ?2",
            params![checkout.0, branch],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some((row.get(0)?, row.get(1)?))),
        None => Ok(None),
    }
}

/// The lease's expected tip: the commit last pushed to the target's branch from `checkout`,
/// once the branch is `own_branch` and the record names the same URL. Never the remote's
/// current tip, which would make the lease an unconditional force.
async fn lease(
    connection: &Connection,
    checkout: CheckoutId,
    target: &PushTarget,
    own_branch: &str,
) -> Result<String, Error> {
    let refuse = |reason: String| Err(Error::Refused { reason });
    let branch = &target.branch;
    if branch != own_branch {
        return refuse(format!(
            "a force push goes only to the session's own branch {own_branch}, and the \
             checkout is on {branch}"
        ));
    }
    match last_pushed(connection, checkout, branch).await? {
        Some((commit, url)) if url == target.remote_url => Ok(commit),
        Some((_, url)) => refuse(format!(
            "the last push of {branch} recorded from this checkout went to {url:?}, not \
             {:?}, so there is no lease to force against; push without force",
            target.remote_url
        )),
        None => refuse(format!(
            "no push of {branch} from this checkout is recorded, so there is no lease to \
             force against; push without force"
        )),
    }
}

async fn record_push(
    write: &WriteTransaction<'_>,
    checkout: CheckoutId,
    target: &PushTarget,
) -> Result<(), Error> {
    write
        .execute(
            "INSERT INTO checkout_pushes (checkout_id, branch, commit_id, remote_url, pushed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (checkout_id, branch) DO UPDATE SET commit_id = excluded.commit_id,
                 remote_url = excluded.remote_url, pushed_at = excluded.pushed_at",
            params![
                checkout.0,
                target.branch.as_str(),
                target.commit.as_str(),
                target.remote_url.as_str(),
                now()
            ],
        )
        .await?;
    Ok(())
}

/// The checkout's own settings that [`REDIRECTS`] names, as Git lists them; `None` when it has
/// none.
pub(crate) fn configured_redirects(
    git: &Git,
    checkout: &std::path::Path,
) -> Result<Option<String>, Error> {
    // `--worktree` reads `.git/config.worktree` when that is enabled, `.git/config` otherwise.
    for scope in ["--local", "--worktree"] {
        let redirects = git.output(
            checkout,
            &[
                arg("config"),
                arg(scope),
                arg("--includes"),
                arg("--get-regexp"),
                arg(REDIRECTS),
            ],
        )?;
        if redirects.status.success() {
            return Ok(Some(
                String::from_utf8_lossy(&redirects.stdout).trim().to_owned(),
            ));
        }
    }
    Ok(None)
}

/// Why `url` is no URL Torlek pushes `project` to; `None` when it is one.
fn refused_push_url(url: &str, project: &ProjectName, push_hosts: &[ForgeHost]) -> Option<String> {
    let repository = match cockpit_forge_api::parse_origin(url) {
        Ok(repository) => repository,
        // An origin naming a credential is refused for holding it, so it is not repeated.
        Err(unresolved @ cockpit_forge_api::Unresolved::UserInfo) => {
            return Some(format!(
                "origin: {unresolved}; Torlek pushes only to an https repository URL"
            ));
        }
        Err(unresolved) => {
            return Some(format!(
                "origin is {url:?}: {unresolved}; Torlek pushes only to an https repository URL"
            ));
        }
    };
    if push_hosts.contains(&repository.host) {
        return None;
    }
    let host = &repository.host;
    Some(format!(
        "origin is {url:?}, on {host}, where {project} has no push right; the owner binds it \
         with `torlek network grant {project} {host} --push`"
    ))
}

/// What a push sends and where: the branch, the commit it pointed at when read, and the one
/// URL Git pushes to, on `host`.
#[derive(Debug, Clone)]
struct PushTarget {
    branch: String,
    commit: String,
    remote_url: String,
    host: ForgeHost,
}

fn push_target(
    git: &Git,
    checkout: &std::path::Path,
    project: &ProjectName,
    push_hosts: &[ForgeHost],
) -> Result<PushTarget, Error> {
    let refuse = |reason: String| Err(Error::Refused { reason });
    let Some(branch) = git.current_branch(checkout)? else {
        return refuse("HEAD is detached; switch to a branch to push it".to_owned());
    };
    let urls = git.run(
        checkout,
        &[
            arg("config"),
            arg("--includes"),
            arg("--get-all"),
            arg("remote.origin.url"),
        ],
    )?;
    // Git pushes to every URL a remote has, so each one must pass.
    let urls: Vec<&str> = urls.lines().collect();
    let [remote_url] = urls.as_slice() else {
        return refuse(format!(
            "origin has {} URLs ({urls:?}); Torlek pushes only to one repository URL",
            urls.len()
        ));
    };
    let remote_url = (*remote_url).to_owned();
    // The URLs Git will push to: push URLs, `insteadOf` and `pushInsteadOf` from every
    // configuration scope applied.
    let push_urls = git.run(
        checkout,
        &[
            arg("remote"),
            arg("get-url"),
            arg("--push"),
            arg("--all"),
            arg("origin"),
        ],
    )?;
    let push_urls: Vec<&str> = push_urls.lines().collect();
    let [push_url] = push_urls.as_slice() else {
        return refuse(format!(
            "origin pushes to {} URLs ({push_urls:?}); Torlek pushes only to one repository \
             URL",
            push_urls.len()
        ));
    };
    let push_url = (*push_url).to_owned();
    for url in [&remote_url, &push_url] {
        if let Some(reason) = refused_push_url(url, project, push_hosts) {
            return refuse(reason);
        }
    }
    if let Some(redirects) = configured_redirects(git, checkout)? {
        return refuse(format!(
            "the checkout's configuration redirects pushes: {redirects}"
        ));
    }
    let host = cockpit_forge_api::parse_origin(&push_url)
        .map_err(|unresolved| Error::Refused {
            reason: format!("origin is {push_url:?}: {unresolved}"),
        })?
        .host;
    let commit = git
        .run(
            checkout,
            &[
                arg("rev-parse"),
                arg("--verify"),
                arg("--end-of-options"),
                arg(&format!("refs/heads/{branch}^{{commit}}")),
            ],
        )?
        .trim()
        .to_owned();
    Ok(PushTarget {
        branch,
        commit,
        remote_url: push_url,
        host,
    })
}

/// The setting that has Git answer the push host alone with the credential, read from
/// [`PUSH_TOKEN_VARIABLE`]. It comes after `SAFE_CONFIG`'s `credential.helper=`, which drops
/// every helper configured before, so this one alone answers.
fn credential_helper(target: &PushTarget, credential: &PushCredential) -> String {
    format!(
        "credential.https://{host}.helper=!f() {{ test \"$1\" = get || exit 0; printf \
         'username=%s\\npassword=%s\\n' '{username}' \"${PUSH_TOKEN_VARIABLE}\"; }}; f",
        host = target.host,
        username = credential.username,
    )
}

/// The CI paths whose content the push would newly bring to the remote, read against it with
/// the push's credential.
fn workflow_changes(
    git: &Git,
    checkout: &std::path::Path,
    target: &PushTarget,
    credential: &PushCredential,
) -> Result<Vec<String>, Error> {
    let helper = credential_helper(target, credential);
    let remote = RemoteAccess {
        git,
        checkout,
        extra_config: &[&helper],
        secret: (PUSH_TOKEN_VARIABLE, credential.token.expose()),
    };
    let tips = remote.tips(&target.branch)?;
    let changed = push_range::changed_ci_paths(&remote, &tips, &target.commit)?;
    Ok(changed.into_iter().map(str::to_owned).collect())
}

/// Sets the branch's upstream, which a push of a commit id does not, before anything is sent,
/// so the push stays the last step that can fail; then pushes the target's commit, forced only
/// under `lease`, with the credential offered to the push host alone. Returns whether Git
/// forced the update.
fn send(
    git: &Git,
    checkout: &std::path::Path,
    target: &PushTarget,
    credential: &PushCredential,
    lease: Option<&str>,
) -> Result<bool, Error> {
    let branch = &target.branch;
    let remote_ref = format!("refs/heads/{branch}");
    for (key, value) in [
        (format!("branch.{branch}.remote"), "origin"),
        (format!("branch.{branch}.merge"), remote_ref.as_str()),
    ] {
        git.run(
            checkout,
            &[arg("config"), arg("--local"), arg(&key), arg(value)],
        )?;
    }
    let helper = credential_helper(target, credential);
    let refspec = format!("{}:{remote_ref}", target.commit);
    let lease_option = lease.map(|expected| format!("--force-with-lease={remote_ref}:{expected}"));
    let mut arguments = vec![arg("push"), arg("--porcelain")];
    if let Some(option) = &lease_option {
        arguments.push(arg(option));
    }
    arguments.extend([arg("--end-of-options"), arg("origin"), arg(&refspec)]);
    let output = git.output_within(
        checkout,
        &[&helper],
        Some((PUSH_TOKEN_VARIABLE, credential.token.expose())),
        &arguments,
        PUSH_LIMIT,
    )?;
    let reported = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        // `--porcelain` reports each ref's rejection (`non-fast-forward`, `stale info`) on
        // standard output, which the failure carries beside Git's own error.
        return Err(match git_failure(&arguments, &output) {
            Error::Git {
                arguments,
                status,
                stderr,
            } => Error::Git {
                arguments,
                status,
                stderr: format!("{reported}{stderr}"),
            },
            other => other,
        });
    }
    // `--porcelain` flags each ref's line: `+` is a forced update.
    Ok(reported.lines().any(|line| line.starts_with("+\t")))
}
