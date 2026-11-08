//! What Codex needs from the sandbox (#830) and where it keeps and takes its credentials
//! (#858). Seeded from Greywall's `internal/profiles/agents/codex.go` at `a9ccc58c`,
//! Apache-2.0: see `crates/cockpit-provider-api/NOTICE-greywall`. Linux paths only.

use std::path::Path;

use cockpit_provider_api::{CredentialHeader, Credentials, HomePath, InvalidValue, Login, Needs};

/// The hosts Codex reaches: the API, and for a ChatGPT sign-in ChatGPT's backend and
/// experiments, and the sign-in host that refreshes its tokens (not in Greywall's profile).
pub const HOSTS: [&str; 4] = [
    "api.openai.com",
    "chatgpt.com",
    "ab.chatgpt.com",
    "auth.openai.com",
];

/// Written into the session's own folders: `CODEX_HOME`'s default and Codex's cache.
pub const WRITE: [&str; 2] = ["~/.codex", "~/.cache/codex"];

/// Where Codex stores its sign-in.
pub const CREDENTIAL_FILES: [&str; 1] = ["~/.codex/auth.json"];

/// The variables Codex 0.166.0 names as its supported API-key variables. Whether its TUI
/// takes a placeholder in one is #832's conformance test; when it does not, #832 writes a
/// session-own `auth.json` that holds only the placeholder (#858).
pub const CREDENTIAL_VARIABLES: [&str; 2] = ["OPENAI_API_KEY", "CODEX_API_KEY"];

/// The host that carries a bound API key, as Bearer.
pub const CREDENTIAL_HOSTS: [&str; 1] = ["api.openai.com"];

pub fn needs() -> Result<Needs, InvalidValue> {
    Needs::new(&HOSTS, &[], &WRITE)
}

pub fn credentials() -> Result<Credentials, InvalidValue> {
    let bindings = CREDENTIAL_VARIABLES
        .map(|variable| (variable, CredentialHeader::Bearer, &CREDENTIAL_HOSTS[..]));
    Credentials::new(&CREDENTIAL_FILES, &bindings, &needs()?)
}

/// `codex login`, told to save the sign-in to [`CREDENTIAL_FILES`]' first rather than the
/// system keyring, as the quota sign-in does.
pub fn login(program: &Path) -> Result<Login, InvalidValue> {
    let arguments: &[&str] = &["login", "-c", "cli_auth_credentials_store=\"file\""];
    Ok(Login {
        file: HomePath::new(CREDENTIAL_FILES[0])?,
        program: program.to_path_buf(),
        arguments: arguments
            .iter()
            .map(|argument| (*argument).to_owned())
            .collect(),
        steps: "finish the sign-in it opens in your browser".to_owned(),
    })
}
