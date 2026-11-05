//! What Codex needs from the sandbox (#830) and where it keeps and takes its credentials
//! (#858). Seeded from Greywall's `internal/profiles/agents/codex.go` at `a9ccc58c`,
//! Apache-2.0: see `crates/cockpit-provider-api/NOTICE-greywall`. Linux paths only.

use cockpit_provider_api::{Credentials, InvalidValue, Needs};

/// The hosts Codex reaches: the API, and ChatGPT's backend and experiments for a ChatGPT
/// sign-in.
pub const HOSTS: [&str; 3] = ["api.openai.com", "chatgpt.com", "ab.chatgpt.com"];

/// Written into the session's own folders: `CODEX_HOME`'s default and Codex's cache.
pub const WRITE: [&str; 2] = ["~/.codex", "~/.cache/codex"];

/// Where Codex stores its sign-in.
pub const CREDENTIAL_FILES: [&str; 1] = ["~/.codex/auth.json"];

/// The variables Codex 0.166.0 names as its supported API-key variables. Whether its TUI
/// takes a placeholder in one is #832's conformance test; when it does not, #832 writes a
/// session-own `auth.json` that holds only the placeholder (#858).
pub const CREDENTIAL_VARIABLES: [&str; 2] = ["OPENAI_API_KEY", "CODEX_API_KEY"];

/// The host that carries a bound API key.
pub const CREDENTIAL_HOSTS: [&str; 1] = ["api.openai.com"];

pub fn needs() -> Result<Needs, InvalidValue> {
    Needs::new(&HOSTS, &[], &WRITE)
}

pub fn credentials() -> Result<Credentials, InvalidValue> {
    let bindings = CREDENTIAL_VARIABLES.map(|variable| (variable, &CREDENTIAL_HOSTS[..]));
    Credentials::new(&CREDENTIAL_FILES, &bindings, &needs()?)
}
