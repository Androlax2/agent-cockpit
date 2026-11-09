//! A provider's credential for sandboxed sessions (#858, #968): one account per provider, kept
//! by the owner in a file of their configuration directory named after the variable the
//! harness reads it from, such as `provider-credentials/claude/CLAUDE_CODE_OAUTH_TOKEN`. The
//! session proxy adds it on the provider's hosts; the agent holds a placeholder. The value
//! never leaves the file except into the launcher's owner-only credentials file: errors name
//! the file and what is wrong, never its contents.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::token_file::{
    TokenFileError, TokenRule, TokenWriteError, read_token_file, save_token_file,
};
use crate::{DEFAULT_ACCOUNT, Environment, Layout};

/// The folder of [`Layout::config_dir`] that holds one folder per provider, named as
/// `session new --agent` takes it.
pub const PROVIDER_CREDENTIALS_DIR: &str = "provider-credentials";

/// API keys and `setup-token` tokens: letters, digits, `_`, `-` and `.`.
const PROVIDER_CREDENTIAL_RULE: TokenRule = TokenRule {
    max_bytes: 4096,
    too_large: "is larger than a provider credential",
    is_token_character: |character| {
        character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
    },
    not_one: "is not one credential: only letters, digits, `_`, `-` and `.`, on one line",
};

/// Where provider `provider`'s credential files live in the configuration of the owner whose
/// environment is `environment`; `None` when it names no home, so none is read.
pub fn provider_credentials_dir(environment: &Environment, provider: &str) -> Option<PathBuf> {
    let home = environment.home_dir().ok()?;
    let layout = Layout::resolve(environment, &home);
    Some(
        layout
            .config_dir
            .value
            .join(PROVIDER_CREDENTIALS_DIR)
            .join(provider),
    )
}

/// Where `account` of `provider` keeps its credential files (#59): the provider's folder for
/// [`DEFAULT_ACCOUNT`], so the single credential of #968 stays where it is, else a folder of
/// the account's name inside it. `None` when `environment` names no home.
pub fn account_credentials_dir(
    environment: &Environment,
    provider: &str,
    account: &str,
) -> Option<PathBuf> {
    let provider_dir = provider_credentials_dir(environment, provider)?;
    if account == DEFAULT_ACCOUNT {
        return Some(provider_dir);
    }
    Some(provider_dir.join(account))
}

/// A credential read from its file. It has no `Display`, and its `Debug` is redacted, so a
/// log or error that formats it shows no value.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderCredential {
    /// The variable its file is named after, which the agent finds a placeholder in.
    pub variable: String,
    pub path: PathBuf,
    value: String,
}

impl ProviderCredential {
    /// The value, for the launcher's credentials file alone.
    pub fn expose(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for ProviderCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderCredential")
            .field("variable", &self.variable)
            .field("path", &self.path)
            .field("value", &"[redacted]")
            .finish()
    }
}

/// Why a provider's credential cannot be used.
#[derive(Debug)]
pub enum ProviderCredentialError {
    File(TokenFileError),
    /// More than one of its files exists: a provider binds one account (#858).
    MoreThanOne {
        paths: Vec<PathBuf>,
    },
}

impl fmt::Display for ProviderCredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(error) => error.fmt(formatter),
            Self::MoreThanOne { paths } => {
                let names: Vec<String> = paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();
                write!(
                    formatter,
                    "{} each hold a credential; a provider binds one account, so remove all \
                     but one",
                    names.join(" and ")
                )
            }
        }
    }
}

impl std::error::Error for ProviderCredentialError {}

/// The credential in `dir` among the files named after `variables`, the adapter's
/// alternatives; `None` when none exists. Blocking: it reads the files. A file
/// [`TokenFileError`] refuses, or two present, refuse the read rather than pick one.
pub fn read_provider_credential(
    dir: &Path,
    variables: &[&str],
) -> Result<Option<ProviderCredential>, ProviderCredentialError> {
    let mut found = Vec::new();
    for variable in variables {
        let path = dir.join(variable);
        if let Some(value) = read_token_file(&path, &PROVIDER_CREDENTIAL_RULE)
            .map_err(ProviderCredentialError::File)?
        {
            found.push(ProviderCredential {
                variable: (*variable).to_owned(),
                path,
                value,
            });
        }
    }
    if found.len() > 1 {
        return Err(ProviderCredentialError::MoreThanOne {
            paths: found
                .into_iter()
                .map(|credential| credential.path)
                .collect(),
        });
    }
    Ok(found.pop())
}

/// Saves `value` in `dir`'s file named after `variable` as the user guide says to by hand
/// (#1251): alone, mode `0600`, the folder `0700` when created. Blocking. A value
/// [`read_provider_credential`] would refuse is refused here; which variable, and whether
/// another file already holds the provider's account, is the caller's to decide.
pub fn save_provider_credential(
    dir: &Path,
    variable: &str,
    value: &str,
) -> Result<PathBuf, TokenWriteError> {
    let path = dir.join(variable);
    save_token_file(&path, value, &PROVIDER_CREDENTIAL_RULE)?;
    Ok(path)
}

/// Which of `variables`, the provider's alternatives, a new credential in `dir` is saved
/// under (#1251): `wanted` when given, else the one whose file exists, else the only one.
/// Blocking: it looks for the files. Refused, with what to do, when that would leave two files
/// holding the provider's one account, or when the choice is the owner's to make.
pub fn credential_variable_to_save<'variable>(
    dir: &Path,
    provider: &str,
    variables: &[&'variable str],
    wanted: Option<&str>,
) -> Result<&'variable str, String> {
    let present: Vec<&str> = variables
        .iter()
        .copied()
        .filter(|variable| std::fs::symlink_metadata(dir.join(variable)).is_ok())
        .collect();
    let chosen = match wanted {
        Some(wanted) => variables
            .iter()
            .copied()
            .find(|variable| *variable == wanted)
            .ok_or_else(|| {
                format!(
                    "{provider} reads no credential from {wanted}; --variable takes {}",
                    variables.join(" or ")
                )
            })?,
        None => match (present.as_slice(), variables) {
            ([one], _) | ([], [one]) => one,
            ([], _) => {
                return Err(format!(
                    "{provider} takes its credential from {}: name one with --variable",
                    variables.join(" or ")
                ));
            }
            _ => {
                return Err(ProviderCredentialError::MoreThanOne {
                    paths: present.iter().map(|variable| dir.join(variable)).collect(),
                }
                .to_string());
            }
        },
    };
    if let Some(other) = present.iter().find(|variable| **variable != chosen) {
        return Err(format!(
            "{} already holds {provider}'s credential, and a provider binds one account: \
             clear it first with `torlek provider credential clear {provider}`",
            dir.join(other).display()
        ));
    }
    Ok(chosen)
}

/// Whether a provider has a usable credential, for what Torlek shows: never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCredentialState {
    Configured {
        variable: String,
        path: PathBuf,
    },
    /// None of its files exists: each is where one may go.
    Missing {
        files: Vec<PathBuf>,
    },
    Unusable {
        reason: String,
    },
}

/// One account's [`ProviderCredentialState`] and signed-in login (#59).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCredentialState {
    pub account: String,
    pub state: ProviderCredentialState,
    pub login: AccountLoginState,
}

/// Whether an account holds the login `torlek account sign-in` keeps (#59), for what Torlek
/// shows: the file's place, never its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountLoginState {
    /// The provider's adapter keeps no login, so the account cannot hold one.
    NotKept,
    /// The owner has not signed the account in.
    NotSignedIn,
    SignedIn {
        file: PathBuf,
        /// The projects holding an active grant for it, the only ones whose sessions use it.
        granted_to: Vec<String>,
    },
    /// What is kept is no plain file, or cannot be read: a launch reaching it is refused.
    Unusable { reason: String },
}

/// [`read_provider_credential`] as its state. Blocking.
pub fn provider_credential_state(dir: &Path, variables: &[&str]) -> ProviderCredentialState {
    match read_provider_credential(dir, variables) {
        Ok(Some(credential)) => ProviderCredentialState::Configured {
            variable: credential.variable,
            path: credential.path,
        },
        Ok(None) => ProviderCredentialState::Missing {
            files: variables
                .iter()
                .map(|variable| dir.join(variable))
                .collect(),
        },
        Err(error) => ProviderCredentialState::Unusable {
            reason: error.to_string(),
        },
    }
}
