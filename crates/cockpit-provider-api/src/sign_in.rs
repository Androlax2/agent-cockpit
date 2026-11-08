use std::path::PathBuf;

use crate::{HomePath, Provenance};

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
}
