//! A project's GitHub token (#754): a fine-grained token the owner keeps in a file of their
//! configuration directory, one per project, which the daemon reads at each launch and gives
//! that project's agents as `GH_TOKEN`. The value never leaves the file except into a
//! launch's private environment: errors name the file and what is wrong, never its contents.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::token_file::{
    TokenFileError, TokenRule, TokenWriteError, read_token_file, save_token_file,
};
use crate::{Environment, Layout};

/// The folder of [`Layout::config_dir`] that holds one token file per project, named as the
/// project is.
pub const GITHUB_TOKENS_DIR: &str = "github-tokens";

/// The folder of [`Layout::config_dir`] that holds one GitHub API token file per project
/// (#857, #832): the credential the session proxy adds on GitHub's hosts, with no Contents
/// write, read under the same rules as the token beside it but never given to an agent.
pub const GITHUB_API_TOKENS_DIR: &str = "github-api-tokens";

/// The variable a launch sets to its project's token, the one `gh` and its Git credential
/// helper read first.
pub const GITHUB_TOKEN_VARIABLE: &str = "GH_TOKEN";

/// Every variable `gh` reads a token from. A launch never inherits one: it sets
/// [`GITHUB_TOKEN_VARIABLE`] to its project's token, or none of them.
pub const GITHUB_TOKEN_VARIABLES: [&str; 4] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

/// Far above a fine-grained token's 93 characters; a larger file is not a token.
const MAX_TOKEN_BYTES: u64 = 1024;

/// Where project `project`'s GitHub token lives in the configuration of the owner whose
/// environment is `environment`; `None` when it names no home, so no token is read. The project
/// name is the registry's, which never holds a `/` or starts with `.`.
pub fn github_token_file(environment: &Environment, project: &str) -> Option<PathBuf> {
    token_file(environment, GITHUB_TOKENS_DIR, project)
}

/// Where project `project`'s GitHub API token lives, as [`github_token_file`] says for the
/// other; read it with [`read_github_token`].
pub fn github_api_token_file(environment: &Environment, project: &str) -> Option<PathBuf> {
    token_file(environment, GITHUB_API_TOKENS_DIR, project)
}

fn token_file(environment: &Environment, folder: &str, project: &str) -> Option<PathBuf> {
    let home = environment.home_dir().ok()?;
    let layout = Layout::resolve(environment, &home);
    Some(layout.config_dir.value.join(folder).join(project))
}

/// A token read from its file. It has no `Display`, and its `Debug` is redacted, so a log or
/// error that formats it shows no value.
#[derive(Clone, PartialEq, Eq)]
pub struct GithubToken(String);

impl GithubToken {
    /// The value, for the launch's environment alone.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for GithubToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GithubToken([redacted])")
    }
}

/// A GitHub token (`github_pat_…`, `ghp_…`): letters, digits and `_`.
const GITHUB_TOKEN_RULE: TokenRule = TokenRule {
    max_bytes: MAX_TOKEN_BYTES,
    too_large: "is larger than a GitHub token",
    is_token_character: |character| character.is_ascii_alphanumeric() || character == '_',
    not_one: "is not one GitHub token: only letters, digits and `_`, on one line",
};

/// The token in `path`, or `None` when there is no such file: that project has no token.
/// Blocking: it reads the file. A symbolic link, a file other users may reach or a value
/// that cannot be a GitHub token is refused, never used.
pub fn read_github_token(path: &Path) -> Result<Option<GithubToken>, TokenFileError> {
    Ok(read_token_file(path, &GITHUB_TOKEN_RULE)?.map(GithubToken))
}

/// Saves `token` in `path` as the user guide says to by hand (#1251): alone, mode `0600`,
/// its folder `0700` when created. Blocking. A value [`read_github_token`] would refuse is
/// refused here.
pub fn save_github_token(path: &Path, token: &str) -> Result<(), TokenWriteError> {
    save_token_file(path, token, &GITHUB_TOKEN_RULE)
}

/// Whether a project has a usable token, for what Torlek shows: never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubTokenState {
    Configured { path: PathBuf },
    Missing { path: PathBuf },
    Unusable { reason: String },
}

/// [`read_github_token`] for project `project`, as its state; `None` when `environment`
/// names no home. Blocking.
pub fn github_token_state(environment: &Environment, project: &str) -> Option<GithubTokenState> {
    let path = github_token_file(environment, project)?;
    Some(match read_github_token(&path) {
        Ok(Some(_)) => GithubTokenState::Configured { path },
        Ok(None) => GithubTokenState::Missing { path },
        Err(error) => GithubTokenState::Unusable {
            reason: error.to_string(),
        },
    })
}
