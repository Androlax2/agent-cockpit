//! The owner's secret files, saved, shown and cleared through the daemon (#1251): a provider's
//! credential for sandboxed sessions and a project's GitHub tokens. A value crosses the
//! owner's socket once, inward, to be written; no reply carries one back.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A secret on its way to its file. Its `Debug` is redacted and it has no `Display`, so a
/// log or panic that formats the command holding it shows no value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretText(String);

impl SecretText {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// The value, for the file it is saved in alone.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// The value is overwritten before its memory is freed (#1386). `black_box` keeps the
/// compiler from dropping the writes; the workspace denies `unsafe`, so no volatile write.
impl Drop for SecretText {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.fill(0);
        std::hint::black_box(&mut bytes);
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretText([redacted])")
    }
}

/// Which secret a credential command names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CredentialTarget {
    /// Provider `provider`'s one account for sandboxed sessions (#968). `variable` names the
    /// file, after the variable the harness reads; without it, the one the provider has.
    Provider {
        provider: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        variable: Option<String>,
    },
    /// Project `project`'s GitHub token given as `GH_TOKEN` (#754) or, `api`, the one the
    /// session proxy adds on GitHub's hosts for a sandboxed session (#857).
    GithubToken {
        project: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        api: bool,
    },
}

/// A secret as Torlek shows it: where it lives and with which mode, never what it holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialView {
    /// Each file a launch looks in, in its order.
    pub files: Vec<CredentialFileView>,
    pub state: CredentialStateView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialFileView {
    pub path: String,
    /// Its permission bits; none when the file does not exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CredentialStateView {
    /// A launch uses the secret in `path`.
    Usable { path: String },
    /// None of its files exists.
    Missing,
    /// A launch refuses it, so the sessions that need it do not launch: `reason` says why.
    Unusable { reason: String },
}
