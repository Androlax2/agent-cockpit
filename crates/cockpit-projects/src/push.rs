//! Pushing a checkout's branch under the host-Git safety rules.

use cockpit_forge_api::ForgeHost;
use cockpit_turso::Database;

use crate::git::arg;
use crate::store;
use crate::{CheckoutId, CheckoutState, Error, Git, ProjectName, blocking};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub branch: String,
    /// The URL Git pushed to, after rewrites.
    pub remote_url: String,
}

/// Checkout settings that send a push, or the sweep's question to origin, elsewhere or change
/// how it gets there: every HTTP setting, since GitHub needs none from the checkout.
const REDIRECTS: &str =
    r"^(remote\.origin\.(pushurl|proxy|vcs)|url\..*\.(push)?insteadof|http\..*)$";

/// Pushes the checkout's current branch to the branch of the same name on origin, setting
/// it as upstream.
///
/// `push_hosts` are the hosts the checkout's project may push to (#153, #857 decision 3): those
/// the owner bound the push right on that a forge adapter declares for Git pushes. The daemon
/// finds them; none until the owner binds one, since no grant was given the right on upgrade.
///
/// Refuses a detached HEAD, an origin with more than one URL (as configured and after
/// rewrites), any of those URLs that is not an `https` repository URL on one of `push_hosts`
/// (an SSH URL among them, named as such), and a checkout whose own configuration redirects
/// pushes (`remote.origin.pushurl`, `.proxy` or `.vcs`, `url.*.insteadOf`,
/// `url.*.pushInsteadOf`, any `http.*` setting). A refusal for a host without the right names
/// the owner's command that binds it. History is never rewritten: the refspec has no `+`, no
/// `--force` is ever passed and a branch is never deleted, so Git refuses a non-fast-forward
/// push.
pub async fn push(
    database: &Database,
    git: &Git,
    id: CheckoutId,
    push_hosts: Vec<ForgeHost>,
) -> Result<Pushed, Error> {
    let stored = {
        let read = database.read().await?;
        store::stored_checkout(&read, id).await?
    };
    if stored.checkout.state != CheckoutState::Live {
        return Err(Error::BeingRemoved { checkout: id });
    }
    let git = git.clone();
    blocking(move || {
        stored.verify_on_disk_now()?;
        push_branch(
            &git,
            &stored.checkout.path,
            &stored.checkout.project,
            &push_hosts,
        )
    })
    .await
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

fn push_branch(
    git: &Git,
    checkout: &std::path::Path,
    project: &ProjectName,
    push_hosts: &[ForgeHost],
) -> Result<Pushed, Error> {
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
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    git.run(
        checkout,
        &[
            arg("push"),
            arg("--quiet"),
            arg("--set-upstream"),
            arg("origin"),
            arg(&refspec),
        ],
    )?;
    Ok(Pushed {
        branch,
        remote_url: push_url,
    })
}
