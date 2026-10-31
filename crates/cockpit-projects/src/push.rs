//! Pushing a checkout's branch under the host-Git safety rules.

use cockpit_turso::Database;

use crate::git::{arg, is_github_url};
use crate::store;
use crate::{CheckoutId, CheckoutState, Error, Git, blocking};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub branch: String,
    /// The URL Git pushed to, after rewrites.
    pub remote_url: String,
}

/// Checkout settings that send a push elsewhere or change how it gets there: every HTTP
/// setting, since GitHub needs none from the checkout.
const REDIRECTS: &str =
    r"^(remote\.origin\.(pushurl|proxy|vcs)|url\..*\.(push)?insteadof|http\..*)$";

/// Pushes the checkout's current branch to the branch of the same name on origin, setting
/// it as upstream.
///
/// Refuses a detached HEAD, an origin with any URL that is not a GitHub URL or with more than
/// one URL (as configured and after rewrites), and a checkout whose own configuration redirects
/// pushes (`remote.origin.pushurl`, `.proxy` or `.vcs`, `url.*.insteadOf`,
/// `url.*.pushInsteadOf`, any `http.*` setting). History is never rewritten: the refspec has no `+`, no `--force`
/// is ever passed and a branch is never deleted, so Git refuses a non-fast-forward push.
pub async fn push(database: &Database, git: &Git, id: CheckoutId) -> Result<Pushed, Error> {
    let stored = {
        let read = database.read().await?;
        store::stored_checkout(&read, id).await?
    };
    if stored.checkout.state != CheckoutState::Live {
        return Err(Error::BeingRemoved { checkout: id });
    }
    stored.verify_on_disk()?;
    let git = git.clone();
    blocking(move || push_branch(&git, &stored.checkout.path)).await
}

fn push_branch(git: &Git, checkout: &std::path::Path) -> Result<Pushed, Error> {
    let refuse = |reason: String| Err(Error::Refused { reason });
    let head = git.output(
        checkout,
        &[
            arg("symbolic-ref"),
            arg("--quiet"),
            arg("--short"),
            arg("HEAD"),
        ],
    )?;
    if !head.status.success() {
        return refuse("HEAD is detached; switch to a branch to push it".to_owned());
    }
    let branch = String::from_utf8_lossy(&head.stdout).trim().to_owned();
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
            "origin has {} URLs ({urls:?}); Cockpit pushes only to one GitHub repository URL",
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
            "origin pushes to {} URLs ({push_urls:?}); Cockpit pushes only to one GitHub \
             repository URL",
            push_urls.len()
        ));
    };
    let push_url = (*push_url).to_owned();
    for url in [&remote_url, &push_url] {
        if !is_github_url(url) {
            return refuse(format!(
                "origin is {url:?}; Cockpit pushes only to a GitHub repository URL"
            ));
        }
    }
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
            return refuse(format!(
                "the checkout's configuration redirects pushes: {}",
                String::from_utf8_lossy(&redirects.stdout).trim()
            ));
        }
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
