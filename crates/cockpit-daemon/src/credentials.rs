//! The owner's credential commands (#1251): save, show and clear a provider's credential for
//! sandboxed sessions and a project's GitHub tokens, in the files and modes the user guide
//! names. The rules live in `cockpit-config`; this file resolves the target and maps.
//! `routes.rs` sends these from the owner's socket alone; no reply carries a value.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cockpit_protocol::{
    Command, CredentialFileView, CredentialStateView, CredentialTarget, CredentialView, ErrorCode,
    Outcome, ProtocolError, Reply, SecretText,
};
use cockpit_turso::Database;

use crate::routes::{Route, WorkClass};

pub(crate) struct Context {
    pub(crate) sessions: Arc<cockpit_sessions::Sessions>,
    pub(crate) database: Arc<Database>,
    pub(crate) environment: cockpit_config::Environment,
}

/// The route for one of the credential commands; `routes.rs` sends only those here.
pub(crate) fn route(command: Command, context: Context) -> Route {
    Route::Run {
        class: WorkClass::Control,
        job: Box::pin(async move {
            let answer = match command {
                Command::CredentialSet { target, value } => set(&context, target, value).await,
                Command::CredentialShow { target } => show(&context, target).await,
                Command::CredentialClear { target } => clear(&context, target).await,
                other => unreachable!("routes.rs sends only credential commands: {other:?}"),
            };
            answer.map_or_else(Outcome::Error, Outcome::Ok)
        }),
    }
}

/// Where a target's secret lives, resolved against the daemon's providers and projects.
enum Files {
    /// A provider's folder and the variables its files are named after, in order.
    Provider {
        provider: String,
        dir: PathBuf,
        variables: Vec<String>,
        variable: Option<String>,
    },
    GithubToken {
        path: PathBuf,
    },
}

/// Whether a GitHub token's project must still be registered: a new token goes to a project
/// that exists, while a removed project's leftover file can still be shown and cleared.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Registered {
    Required,
    NotRequired,
}

async fn resolve(
    context: &Context,
    target: CredentialTarget,
    registered: Registered,
) -> Result<Files, ProtocolError> {
    match target {
        CredentialTarget::Provider { provider, variable } => {
            let variables = context
                .sessions
                .provider_credential_variables(&provider)
                .map_err(|error| crate::sessions::protocol_error(&error))?;
            if variables.is_empty() {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    format!("{provider} takes no credential for sandboxed sessions"),
                ));
            }
            if let Some(variable) = &variable
                && !variables.contains(variable)
            {
                return Err(ProtocolError::new(
                    ErrorCode::InvalidArgument,
                    format!(
                        "{provider} reads no credential from {variable}; --variable takes {}",
                        variables.join(" or ")
                    ),
                ));
            }
            let dir = cockpit_config::provider_credentials_dir(&context.environment, &provider)
                .ok_or_else(unknown_home)?;
            Ok(Files::Provider {
                provider,
                dir,
                variables,
                variable,
            })
        }
        CredentialTarget::GithubToken { project, api } => {
            let name = project_name(&context.database, project, registered)
                .await
                .map_err(|error| {
                    let code = crate::projects::error_code(&error);
                    if code == ErrorCode::Internal {
                        tracing::error!(%error, "a credential command could not read the projects");
                    }
                    ProtocolError::new(code, error.to_string())
                })?;
            let name = name.as_str();
            let path = if api {
                cockpit_config::github_api_token_file(&context.environment, name)
            } else {
                cockpit_config::github_token_file(&context.environment, name)
            };
            Ok(Files::GithubToken {
                path: path.ok_or_else(unknown_home)?,
            })
        }
    }
}

/// `name` as the registry spells project names, which never hold a `/` or start with `.`.
async fn project_name(
    database: &Database,
    name: String,
    registered: Registered,
) -> Result<cockpit_projects::ProjectName, cockpit_projects::Error> {
    let read = database
        .read()
        .await
        .map_err(cockpit_projects::Error::from)?;
    match cockpit_projects::project_named(&read, &name).await {
        Ok(project) => Ok(project.name),
        Err(cockpit_projects::Error::UnknownProject { .. })
            if registered == Registered::NotRequired =>
        {
            cockpit_projects::ProjectName::new(name)
        }
        Err(error) => Err(error),
    }
}

/// Held from choosing a provider's file to writing it, so two saves at once cannot each see
/// no file and leave the provider two accounts.
static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());

async fn set(
    context: &Context,
    target: CredentialTarget,
    value: SecretText,
) -> Result<Reply, ProtocolError> {
    let files = resolve(context, target, Registered::Required).await?;
    blocking(move || {
        let _saving = SAVING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &files {
            Files::Provider {
                provider,
                dir,
                variables,
                variable,
            } => {
                let variables: Vec<&str> = variables.iter().map(String::as_str).collect();
                let chosen = cockpit_config::credential_variable_to_save(
                    dir,
                    provider,
                    &variables,
                    variable.as_deref(),
                )
                .map_err(|reason| ProtocolError::new(ErrorCode::Refused, reason))?;
                cockpit_config::save_provider_credential(dir, chosen, value.expose())
                    .map_err(write_error)?;
            }
            Files::GithubToken { path } => {
                cockpit_config::save_github_token(path, value.expose()).map_err(write_error)?;
            }
        }
        Ok(Reply::Credential {
            credential: view(&files),
        })
    })
    .await
}

async fn show(context: &Context, target: CredentialTarget) -> Result<Reply, ProtocolError> {
    let files = resolve(context, target, Registered::NotRequired).await?;
    blocking(move || {
        Ok(Reply::Credential {
            credential: view(&files),
        })
    })
    .await
}

async fn clear(context: &Context, target: CredentialTarget) -> Result<Reply, ProtocolError> {
    let files = resolve(context, target, Registered::NotRequired).await?;
    blocking(move || {
        let paths = match files {
            Files::Provider {
                dir,
                variable: Some(variable),
                ..
            } => vec![dir.join(variable)],
            Files::Provider { dir, variables, .. } => variables
                .iter()
                .map(|variable| dir.join(variable))
                .collect(),
            Files::GithubToken { path } => vec![path],
        };
        let mut removed = Vec::new();
        for path in paths {
            if cockpit_config::remove_secret_file(&path).map_err(write_error)? {
                removed.push(crate::projects::text(&path));
            }
        }
        Ok(Reply::CredentialCleared { removed })
    })
    .await
}

/// The target's files and state as a launch would read them now. Blocking.
fn view(files: &Files) -> CredentialView {
    match files {
        Files::Provider { dir, variables, .. } => {
            let names: Vec<&str> = variables.iter().map(String::as_str).collect();
            let state = match cockpit_config::provider_credential_state(dir, &names) {
                cockpit_config::ProviderCredentialState::Configured { path, .. } => {
                    CredentialStateView::Usable {
                        path: crate::projects::text(&path),
                    }
                }
                cockpit_config::ProviderCredentialState::Missing { .. } => {
                    CredentialStateView::Missing
                }
                cockpit_config::ProviderCredentialState::Unusable { reason } => {
                    CredentialStateView::Unusable { reason }
                }
            };
            CredentialView {
                files: variables
                    .iter()
                    .map(|variable| file_view(&dir.join(variable)))
                    .collect(),
                state,
            }
        }
        Files::GithubToken { path } => CredentialView {
            files: vec![file_view(path)],
            state: match cockpit_config::read_github_token(path) {
                Ok(Some(_)) => CredentialStateView::Usable {
                    path: crate::projects::text(path),
                },
                Ok(None) => CredentialStateView::Missing,
                Err(error) => CredentialStateView::Unusable {
                    reason: error.to_string(),
                },
            },
        },
    }
}

fn file_view(path: &Path) -> CredentialFileView {
    CredentialFileView {
        path: crate::projects::text(path),
        mode: cockpit_config::token_file_mode(path),
    }
}

async fn blocking(
    work: impl FnOnce() -> Result<Reply, ProtocolError> + Send + 'static,
) -> Result<Reply, ProtocolError> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| {
            tracing::error!(%error, "a credential command failed");
            Err(ProtocolError::new(ErrorCode::Internal, error.to_string()))
        })
}

fn write_error(error: cockpit_config::TokenWriteError) -> ProtocolError {
    let code = match error {
        cockpit_config::TokenWriteError::NotOne { .. } => ErrorCode::InvalidArgument,
        cockpit_config::TokenWriteError::NotPlain { .. } => ErrorCode::Refused,
        cockpit_config::TokenWriteError::Io { .. } => ErrorCode::Internal,
    };
    ProtocolError::new(code, error.to_string())
}

fn unknown_home() -> ProtocolError {
    ProtocolError::new(
        ErrorCode::Refused,
        "the owner's home is unknown, so their configuration folder is too",
    )
}
