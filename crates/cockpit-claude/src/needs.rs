//! What Claude Code needs from the sandbox (#830) and where it keeps and takes its
//! credentials (#858). Seeded from Greywall's `internal/profiles/agents/claude.go` at
//! `a9ccc58c`, Apache-2.0: see `crates/cockpit-provider-api/NOTICE-greywall`. Linux paths
//! only; the README lists what was left out and why.

use std::path::Path;

use cockpit_provider_api::{
    CredentialHeader, Credentials, InvalidValue, Login, LoginRewrite, Needs,
};

/// The hosts Claude Code reaches: the API, claude.ai's connector proxy, its downloads and its
/// sign-in console.
pub const HOSTS: [&str; 4] = [
    "api.anthropic.com",
    "mcp-proxy.anthropic.com",
    "downloads.claude.ai",
    "platform.claude.com",
];

/// Read from the owner's home: the installed Claude Code versions.
pub const READ: [&str; 1] = ["~/.local/share/claude"];

/// Written into the session's own folders: Claude Code's configuration folder and file, their
/// backups and lock, its cache and state, and the user-scope MCP file.
pub const WRITE: [&str; 8] = [
    "~/.claude",
    "~/.claude.json",
    "~/.claude.json.*",
    "~/.claude.lock",
    "~/.cache/claude",
    "~/.config/claude",
    "~/.local/state/claude",
    "~/.mcp.json",
];

/// Where Claude Code stores its sign-in, MCP server logins included.
pub const CREDENTIAL_FILES: [&str; 1] = ["~/.claude/.credentials.json"];

/// The variable a `claude setup-token` token travels in; in a sandbox, the placeholder the
/// session proxy swaps for the account's credential (#968). The hook bridge reads it back to
/// ask for the session's plan windows (#249).
pub const OAUTH_TOKEN_VARIABLE: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// The variables that take a bound credential: an API key, or a `claude setup-token` token.
pub const CREDENTIAL_VARIABLES: [&str; 2] =
    [crate::first_run::API_KEY_VARIABLE, OAUTH_TOKEN_VARIABLE];

/// The host that carries a bound credential.
pub const CREDENTIAL_HOSTS: [&str; 1] = ["api.anthropic.com"];

/// The header each of [`CREDENTIAL_VARIABLES`] travels in: Claude Code sends an API key as
/// `x-api-key` and a `setup-token` token as Bearer.
pub const CREDENTIAL_HEADERS: [CredentialHeader; 2] = [
    CredentialHeader::Named("x-api-key"),
    CredentialHeader::Bearer,
];

pub fn needs() -> Result<Needs, InvalidValue> {
    Needs::new(&HOSTS, &READ, &WRITE)
}

pub fn credentials() -> Result<Credentials, InvalidValue> {
    let bindings: Vec<_> = CREDENTIAL_VARIABLES
        .into_iter()
        .zip(CREDENTIAL_HEADERS)
        .map(|(variable, header)| (variable, header, &CREDENTIAL_HOSTS[..]))
        .collect();
    Credentials::new(&CREDENTIAL_FILES, &bindings, &needs()?)
}

/// Claude Code's own sign-in: its first run in a fresh home, whose prompts and `/login`
/// save the sign-in to [`CREDENTIAL_FILES`]' first.
pub fn login(program: &Path) -> Result<Login, InvalidValue> {
    Login::new(
        CREDENTIAL_FILES[0],
        program,
        &[],
        "sign in through its first-run prompts or `/login` (both take a pasted code when no browser \
     opens), then type `/exit`",
        LoginRewrite::InPlace,
    )
}
