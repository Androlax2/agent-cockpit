//! What OpenCode needs from the sandbox (#830) and where it keeps and takes its credentials
//! (#858). Seeded from Greywall's `internal/profiles/agents/opencode.go` at `a9ccc58c`,
//! Apache-2.0: see `crates/cockpit-provider-api/NOTICE-greywall`.

use std::path::Path;

use cockpit_provider_api::{CredentialHeader, Credentials, InvalidValue, Login, Needs};

/// The hosts OpenCode reaches: the two model APIs Greywall's profile allowed, and the model
/// catalog OpenCode fetches at start (not in Greywall's profile), since the session's cache
/// starts empty.
pub const HOSTS: [&str; 3] = ["api.openai.com", "api.anthropic.com", "models.dev"];

/// Read from the owner's home: OpenCode's install folder, its program and its packages.
pub const READ: [&str; 1] = ["~/.opencode"];

/// Written into the session's own folders: OpenCode's configuration, cache, data and state,
/// and its terminal UI's data.
pub const WRITE: [&str; 6] = [
    "~/.opencode.json",
    "~/.config/opencode",
    "~/.cache/opencode",
    "~/.local/share/opencode",
    "~/.local/share/opentui",
    "~/.local/state/opencode",
];

/// Where OpenCode stores sign-ins: its providers' keys and logins, its accounts, and the
/// GitHub Copilot sign-in it reads.
pub const CREDENTIAL_FILES: [&str; 3] = [
    "~/.local/share/opencode/auth.json",
    "~/.local/share/opencode/account.json",
    "~/.config/github-copilot",
];

/// Each provider key variable OpenCode reads, with the header and host that carry it.
pub const CREDENTIAL_BINDINGS: [(&str, CredentialHeader, &[&str]); 2] = [
    (
        "ANTHROPIC_API_KEY",
        CredentialHeader::Named("x-api-key"),
        &["api.anthropic.com"],
    ),
    (
        "OPENAI_API_KEY",
        CredentialHeader::Bearer,
        &["api.openai.com"],
    ),
];

pub fn needs() -> Result<Needs, InvalidValue> {
    Needs::new(&HOSTS, &READ, &WRITE)
}

pub fn credentials() -> Result<Credentials, InvalidValue> {
    Credentials::new(&CREDENTIAL_FILES, &CREDENTIAL_BINDINGS, &needs()?)
}

/// `opencode auth login`, which saves the sign-in to [`CREDENTIAL_FILES`]' first.
pub fn login(program: &Path) -> Result<Login, InvalidValue> {
    Login::new(
        CREDENTIAL_FILES[0],
        program,
        &["auth", "login"],
        "pick the provider and sign in",
    )
}
