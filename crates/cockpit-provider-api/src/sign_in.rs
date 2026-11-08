use std::path::{Path, PathBuf};

use crate::{HomePath, InvalidValue, Provenance};

/// A harness's local sign-in fact, with the provenance of its probe independently of
/// other capabilities. No credential contents or account identifiers belong here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignIn {
    pub status: SignInStatus,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInStatus {
    SignedIn,
    NotSignedIn { next: String },
    Unknown { reason: String, next: String },
}

impl SignIn {
    pub fn unknown(reason: impl Into<String>, next: impl Into<String>) -> SignIn {
        SignIn {
            status: SignInStatus::Unknown {
                reason: reason.into(),
                next: next.into(),
            },
            provenance: Provenance::Discovered,
        }
    }
}

/// A sign-in that refreshes itself, which a login account keeps in a folder of Torlek's own
/// (#858, #59): the owner runs it once, in their terminal, with a fresh folder as its home,
/// and Torlek keeps only the file it leaves at [`Login::file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    /// Where the harness saves the sign-in under its home: one of the adapter's
    /// [`crate::Credentials::files`].
    pub file: HomePath,
    /// The harness's own sign-in command.
    pub program: PathBuf,
    pub arguments: Vec<String>,
    /// What the owner does in it, said before it starts.
    pub steps: String,
    /// How the harness saves a refreshed sign-in, which decides how a session sees the
    /// account's one shared login.
    pub rewrite: LoginRewrite,
}

/// How a harness writes its refreshed sign-in over [`Login::file`] (#858).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginRewrite {
    /// Truncates and rewrites the file itself, and refuses a link there (Claude Code opens
    /// it with `O_NOFOLLOW`): the session sees the login file bound at that path.
    InPlace,
    /// Writes a new file in the folder of the file's target, after following a link, and
    /// renames it over that target (Codex): a rename onto a bound file fails, so the session
    /// sees a folder holding only the login, and a link to it at that path.
    ReplacesLinkTarget,
}

impl Login {
    /// A declaration from its written form; `file` is a `~/` path.
    pub fn new(
        file: &str,
        program: &Path,
        arguments: &[&str],
        steps: &str,
        rewrite: LoginRewrite,
    ) -> Result<Login, InvalidValue> {
        Ok(Login {
            file: HomePath::new(file)?,
            program: program.to_path_buf(),
            arguments: arguments
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
            steps: steps.to_owned(),
            rewrite,
        })
    }
}
