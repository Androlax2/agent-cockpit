use crate::Provenance;

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
