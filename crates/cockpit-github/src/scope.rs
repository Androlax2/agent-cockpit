//! What GitHub shows of a token's scope. It shows whether the token is accepted and reaches
//! the repository, and its kind by the prefix GitHub documents; it never shows a fine-grained
//! token's permissions or repository list, so those are reported unverified.

use cockpit_forge_api::{Permission, ScopeCheck, ScopeProblem, Token, TokenKind};

/// What reading the repository with the token gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    Reached,
    /// GitHub answers "not found" for a private repository a token cannot read.
    Hidden,
    NotAccepted,
}

/// A classic personal access token reaches every repository its owner can.
const CLASSIC_PREFIX: &str = "ghp_";
const FINE_GRAINED_PREFIX: &str = "github_pat_";

pub(crate) fn judge(token: &Token, kind: TokenKind, reach: Reach) -> ScopeCheck {
    let mut problems = Vec::new();
    match reach {
        Reach::Reached => {}
        Reach::Hidden => problems.push(ScopeProblem::Missing(Permission::ContentsRead)),
        Reach::NotAccepted => problems.push(ScopeProblem::NotAccepted),
    }
    let value = token.expose();
    if value.starts_with(CLASSIC_PREFIX) {
        problems.push(ScopeProblem::OtherRepositories);
    }
    if !problems.is_empty() {
        return ScopeCheck::Refused { problems };
    }
    let reason = if value.starts_with(FINE_GRAINED_PREFIX) {
        format!(
            "GitHub does not show a fine-grained token's permissions or the repositories it \
             reaches: check on its settings page that it has {} and nothing more, on this \
             repository alone",
            listed(kind.required())
        )
    } else {
        "not a personal access token GitHub documents: its scope cannot be read".to_owned()
    };
    ScopeCheck::Unverified { reason }
}

fn listed(permissions: &[Permission]) -> String {
    permissions
        .iter()
        .map(|permission| permission.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
