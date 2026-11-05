//! What Claude Code needs from the sandbox (#830) and where it keeps and takes its
//! credentials (#858). Seeded from Greywall's `internal/profiles/agents/claude.go` at
//! `a9ccc58c`, Apache-2.0: see `crates/cockpit-provider-api/NOTICE-greywall`. Linux paths
//! only; the README lists what was left out and why.

use cockpit_provider_api::{Credentials, InvalidValue, Needs};

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

/// The variables that take a bound credential: an API key, or a `claude setup-token` token.
pub const CREDENTIAL_VARIABLES: [&str; 2] = ["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"];

/// The host that carries a bound credential.
pub const CREDENTIAL_HOSTS: [&str; 1] = ["api.anthropic.com"];

pub fn needs() -> Result<Needs, InvalidValue> {
    Needs::new(&HOSTS, &READ, &WRITE)
}

pub fn credentials() -> Result<Credentials, InvalidValue> {
    let bindings = CREDENTIAL_VARIABLES.map(|variable| (variable, &CREDENTIAL_HOSTS[..]));
    Credentials::new(&CREDENTIAL_FILES, &bindings, &needs()?)
}
