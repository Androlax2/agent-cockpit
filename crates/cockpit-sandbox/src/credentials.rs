//! Credentials bound to hosts (#832, #546 D1 stage C): the session proxy adds each one on its
//! own hosts, which it intercepts with a per-session CA the launcher generates in memory; the
//! sandbox holds a placeholder at most. Every other host stays a plain tunnel.
//!
//! The values travel apart from the launch spec: in [`CREDENTIALS_FILE`], owner-only, beside
//! it in the session's mount folder no agent reaches, which the launcher removes as soon as it
//! read it, as it does the spec (#434). A tmux pane inherits no descriptor from the daemon, so
//! a file the launcher consumes stands in for the descriptor #857's report proposed; neither
//! is the environment, which the sandbox inherits.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The file beside the launch spec that holds the bound credentials' values, in the order of
/// [`Interception::credentials`]: a JSON array of strings.
pub const CREDENTIALS_FILE: &str = "credentials.json";

/// What a placeholder variable holds in the sandbox: enough for a client such as `gh` to run
/// and send requests, which the proxy completes on the credential's hosts alone.
pub const PLACEHOLDER: &str = "torlek-placeholder-the-session-proxy-adds-the-credential";

/// The per-session CA certificate alone, for runtimes that add it to their own roots
/// (`NODE_EXTRA_CA_CERTS`), in the session's tool configuration folder.
pub const SESSION_CA_FILE: &str = "session-ca.pem";

/// The host's CA bundle with the per-session CA appended, for runtimes whose variable
/// replaces their roots (`SSL_CERT_FILE` and its kin): every tunneled host keeps verifying
/// against the host's own roots.
pub const CA_BUNDLE_FILE: &str = "ca-bundle.pem";

/// Variables that replace a runtime's roots: OpenSSL, Go (`gh`), Python's `ssl`, rustls'
/// native roots; Git's libcurl; Python Requests; the `curl` tool.
pub const BUNDLE_VARIABLES: [&str; 4] = [
    "SSL_CERT_FILE",
    "GIT_SSL_CAINFO",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
];

/// The variable Node and Bun read to add a CA to their bundled roots.
pub const EXTRA_CA_VARIABLE: &str = "NODE_EXTRA_CA_CERTS";

/// Where a launch looks for the host's CA bundle when the owner's environment sets no
/// `SSL_CERT_FILE`: Arch and Debian, Fedora, openSUSE, then Alpine.
pub const SYSTEM_BUNDLES: [&str; 4] = [
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/ca-bundle.pem",
    "/etc/ssl/cert.pem",
];

/// A credential's value. It has no `Display`, and its `Debug` is redacted, so a log or error
/// that formats it shows no value.
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialValue(String);

impl CredentialValue {
    pub fn new(value: impl Into<String>) -> CredentialValue {
        CredentialValue(value.into())
    }

    /// The value, for the credentials file alone.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialValue([redacted])")
    }
}

/// One credential a launch binds to its hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundCredential {
    /// What the evidence and the proxy's answers call it: "the project's GitHub API token".
    pub name: String,
    /// The variable the sandbox finds [`PLACEHOLDER`] in, for a client that needs one set.
    pub placeholder_variable: Option<String>,
    pub value: CredentialValue,
    pub hosts: Vec<CredentialHost>,
}

/// A host that carries a credential, on port 443, and how the proxy adds it there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialHost {
    /// A lower-case DNS name, never a pattern.
    pub host: String,
    pub scheme: CredentialScheme,
}

/// How the proxy adds a credential to the requests of one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CredentialScheme {
    /// `Authorization: Bearer` with the value on every request, replacing the client's.
    Bearer,
    /// `name: <value>` on every request, replacing the client's (#968): an `x-` header such as
    /// Anthropic's `x-api-key`, never one that frames or routes the request.
    Header { name: String },
    /// Git's smart HTTP reads (#857 decision 1): `Authorization: Basic` of `username` and the
    /// value on `info/refs?service=git-upload-pack` and `git-upload-pack` alone, replacing
    /// the client's. Both halves of a push are refused: pushes go through the daemon. Any
    /// other request passes as the client sent it.
    GitReads { username: String },
}

impl CredentialScheme {
    pub fn describe(&self) -> &'static str {
        match self {
            CredentialScheme::Bearer => "as Bearer on every request",
            CredentialScheme::Header { .. } => "in its own header on every request",
            CredentialScheme::GitReads { .. } => {
                "as Basic on Git fetches only; Git pushes over HTTPS refused"
            }
        }
    }
}

/// The interception a launch asks of its launcher, written in the spec: no value, only names,
/// hosts and where the trust files go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interception {
    /// The bound credentials, in the order of the values in [`CREDENTIALS_FILE`].
    pub credentials: Vec<SpecCredential>,
    /// The host's CA bundle: the roots the launcher verifies upstream servers against, and
    /// the start of [`Interception::bundle_file`].
    pub system_bundle: PathBuf,
    /// Where the launcher writes the per-session CA certificate before the program starts.
    pub ca_file: PathBuf,
    /// Where it writes the host's bundle with that certificate appended.
    pub bundle_file: PathBuf,
}

/// A bound credential as the spec names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecCredential {
    pub name: String,
    pub hosts: Vec<CredentialHost>,
}
