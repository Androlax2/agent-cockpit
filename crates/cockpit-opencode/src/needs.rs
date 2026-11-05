//! What OpenCode needs from the sandbox (#830) and where it keeps and takes its credentials
//! (#858). Seeded from Greywall's `internal/profiles/agents/opencode.go` at `a9ccc58c`,
//! Apache-2.0: see `crates/cockpit-provider-api/NOTICE-greywall`.

use cockpit_provider_api::{Credentials, InvalidValue, Needs};

/// The hosts OpenCode reaches: the two model APIs Greywall's profile allowed.
pub const HOSTS: [&str; 2] = ["api.openai.com", "api.anthropic.com"];

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

/// Each provider key variable OpenCode reads, with the host that carries it.
pub const CREDENTIAL_BINDINGS: [(&str, &[&str]); 2] = [
    ("ANTHROPIC_API_KEY", &["api.anthropic.com"]),
    ("OPENAI_API_KEY", &["api.openai.com"]),
];

pub fn needs() -> Result<Needs, InvalidValue> {
    Needs::new(&HOSTS, &READ, &WRITE)
}

pub fn credentials() -> Result<Credentials, InvalidValue> {
    Credentials::new(&CREDENTIAL_FILES, &CREDENTIAL_BINDINGS, &needs()?)
}
