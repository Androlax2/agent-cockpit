//! What every daemon read of a project's forge shares (#60): the project's API token, its
//! `origin` resolved to an adapter, and the refusals that name the next step. The daemon
//! reads with the API token alone and never writes with it (#857 as amended by Theo on
//! 2026-11-22); a project with no API token is not read.

use std::path::Path;

use cockpit_forge_api::{Access, Forge, ForgeError, Token, resolve};
use cockpit_projects::Git;
use cockpit_protocol::{ErrorCode, ProtocolError, Unblock, UnblockedBy};

use crate::approvals::Failed;
use crate::forges::Adapters;

/// What a read needs from the daemon.
pub(crate) struct Reader<'reader> {
    pub(crate) git: &'reader Git,
    pub(crate) environment: &'reader cockpit_config::Environment,
    pub(crate) adapters: &'reader Adapters,
}

impl Reader<'_> {
    /// Runs `read` on the forge `project`'s checkout at `root` names as `origin`, with the
    /// project's API token; `what` names what is read in its refusals (`app's open issues`)
    /// and `again` the command to run once the cause is fixed. Blocking: it reads the
    /// token file, asks Git for the origin and runs the forge's program.
    pub(crate) fn read<T>(
        &self,
        project: &str,
        root: &Path,
        what: &str,
        again: &str,
        read: impl FnOnce(&dyn Forge, &Access<'_>) -> Result<T, ForgeError>,
    ) -> Result<T, Failed> {
        let token = api_token(self.environment, project, what)?;
        self.reach(project, root, what, &token, read)?
            .map_err(|error| forge_failed(project, what, again, &error))
    }

    /// Runs `call` with `token` on the forge `project`'s checkout at `root` names as
    /// `origin`, and hands back the forge's own answer for the caller to word; a project that
    /// names no forge is refused here, `what` naming what the call was for. Blocking: it asks
    /// Git for the origin and runs the forge's program.
    pub(crate) fn reach<T>(
        &self,
        project: &str,
        root: &Path,
        what: &str,
        token: &Token,
        call: impl FnOnce(&dyn Forge, &Access<'_>) -> Result<T, ForgeError>,
    ) -> Result<Result<T, ForgeError>, Failed> {
        let origin = self
            .git
            .origin_url(root)
            .map_err(|error| internal(&error))?
            .ok_or_else(|| {
                refused(
                    ErrorCode::Refused,
                    format!("{project} has no `origin` remote, so {what} cannot be read"),
                    format!("add the repository as `origin` in {}", root.display()),
                )
            })?;
        self.adapters
            .read(self.environment, |adapters| {
                // The origin itself is not repeated: one naming a credential is refused for
                // holding it.
                let (forge, repository) = resolve(adapters, &origin).map_err(|unresolved| {
                    refused(
                        ErrorCode::Refused,
                        format!("{project}'s origin names no forge Torlek reads: {unresolved}"),
                        "point `origin` at the project's https repository URL".to_owned(),
                    )
                })?;
                let access = Access {
                    repository: &repository,
                    token,
                };
                Ok(call(forge, &access))
            })
            .map_err(|error| internal(&error))?
    }
}

/// The project's API token: the one a daemon read may use (never its other token).
fn api_token(
    environment: &cockpit_config::Environment,
    project: &str,
    what: &str,
) -> Result<Token, Failed> {
    let set_it = format!("`torlek project token set {project} --api`");
    let path = cockpit_config::github_api_token_file(environment, project)
        .ok_or_else(|| internal(&"the daemon's environment names no home, so no token folder"))?;
    let stored = cockpit_config::read_github_token(&path)
        .map_err(|error| {
            refused(
                ErrorCode::Refused,
                format!("{project}'s API token is unusable: {error}"),
                set_it.clone(),
            )
        })?
        .ok_or_else(|| {
            refused(
                ErrorCode::Refused,
                format!("{project} has no API token, so {what} cannot be read"),
                set_it.clone(),
            )
        })?;
    // The file's rule is stricter than the contract's, so a stored token always converts.
    Token::new(stored.expose()).map_err(|error| internal(&error))
}

fn forge_failed(project: &str, what: &str, again: &str, error: &ForgeError) -> Failed {
    let (code, action) = match error {
        ForgeError::NotFound { .. } => (
            ErrorCode::NotFound,
            format!("check that {project}'s API token may read the repository"),
        ),
        ForgeError::Refused { .. } => (
            ErrorCode::Refused,
            format!("check {project}'s API token: `torlek project token show {project} --api`"),
        ),
        ForgeError::Unsupported { .. }
        | ForgeError::Withheld { .. }
        | ForgeError::Unavailable { .. }
        | ForgeError::Malformed { .. } => (
            ErrorCode::Refused,
            format!("fix what the message names, then {again}"),
        ),
    };
    refused(code, format!("{what} could not be read: {error}"), action)
}

pub(crate) fn refused(code: ErrorCode, message: String, action: String) -> Failed {
    Failed(ProtocolError {
        code,
        message,
        unblock: Some(Unblock {
            action,
            by: UnblockedBy::Owner,
        }),
    })
}

pub(crate) fn internal(error: &dyn std::fmt::Display) -> Failed {
    tracing::error!(%error, "reading a project's forge failed");
    Failed(ProtocolError::new(
        ErrorCode::Internal,
        "the daemon failed to read the project's forge",
    ))
}
