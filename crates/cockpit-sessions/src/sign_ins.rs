//! `torlek account sign-in` (#59, #858): the owner signs a login account in once, through the
//! harness's own sign-in run in their terminal with a fresh folder of Torlek's as its home;
//! the daemon then moves the one file the adapter names into the account's login folder. It
//! never opens that file: it checks what kind of entry it is, sets its mode and renames it.

use std::fs::DirBuilder;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use cockpit_config::Source;
use cockpit_provider_api::Login;
use cockpit_sandbox::AccountLogin;

use super::Sessions;
use crate::Error;

/// What the owner's terminal runs to sign an account in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInPlan {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    /// The fresh folder the sign-in runs with as its home.
    pub home: PathBuf,
    /// What the owner does in the sign-in.
    pub steps: String,
}

impl Sessions {
    /// Gives `account` of `provider` a fresh sign-in home, replacing what an unfinished
    /// sign-in left there, and says what to run in it.
    pub async fn begin_sign_in(&self, provider: &str, account: &str) -> Result<SignInPlan, Error> {
        let (login, folders) = self.login_of(provider, account)?;
        let home = folders.sign_in.clone();
        self.blocking("prepare a sign-in home", move || {
            remove_folder(&home)?;
            create_owner_folder(&home)
        })
        .await?;
        Ok(SignInPlan {
            program: login.program,
            arguments: login.arguments,
            home: folders.sign_in,
            steps: login.steps,
        })
    }

    /// Moves the login the sign-in left in its home into the account's login folder,
    /// replacing the one kept before, then removes the rest of that home. Returns the login
    /// folder.
    pub async fn finish_sign_in(&self, provider: &str, account: &str) -> Result<PathBuf, Error> {
        let (login, folders) = self.login_of(provider, account)?;
        let refused = refusal(provider, account);
        let again = format!("run `torlek account sign-in {provider} {account}` again");
        self.blocking("keep a sign-in's login", move || {
            let left = folders.sign_in.join(relative(&login));
            let missing = || {
                refused(format!(
                    "the sign-in left no file at {}; {again}",
                    login.file
                ))
            };
            let Some(parent) = left.parent() else {
                return Err(missing());
            };
            let home = folders.sign_in.canonicalize().map_err(|source| Error::Io {
                action: "resolve",
                path: folders.sign_in.clone(),
                source,
            })?;
            // A link on the way would move a file from outside the sign-in's home.
            match parent.canonicalize() {
                Ok(real) if real.starts_with(&home) => {}
                Ok(_) => {
                    return Err(refused(format!(
                        "the folder of {} the sign-in left is outside its home",
                        login.file
                    )));
                }
                Err(error) if error.kind() == ErrorKind::NotFound => return Err(missing()),
                Err(source) => {
                    return Err(Error::Io {
                        action: "resolve",
                        path: parent.to_path_buf(),
                        source,
                    });
                }
            }
            match std::fs::symlink_metadata(&left) {
                Ok(metadata) if metadata.file_type().is_file() => {}
                Ok(_) => {
                    return Err(refused(format!(
                        "the sign-in left {} as something other than a plain file",
                        login.file
                    )));
                }
                Err(error) if error.kind() == ErrorKind::NotFound => return Err(missing()),
                Err(source) => {
                    return Err(Error::Io {
                        action: "inspect",
                        path: left,
                        source,
                    });
                }
            }
            // Owner-only before it moves, so the login folder never holds it readable by others.
            std::fs::set_permissions(&left, std::fs::Permissions::from_mode(0o600)).map_err(
                |source| Error::Io {
                    action: "make owner-only",
                    path: left.clone(),
                    source,
                },
            )?;
            create_owner_folder(&folders.login)?;
            let file_name = left.file_name().ok_or_else(missing)?;
            let kept = folders.login.join(file_name);
            std::fs::rename(&left, &kept).map_err(|source| Error::Io {
                action: "move the login to",
                path: kept,
                source,
            })?;
            remove_folder(&folders.sign_in)?;
            Ok(folders.login)
        })
        .await
    }

    /// The adapter's login and the account's folders, once the owner lists the account.
    fn login_of(&self, provider: &str, account: &str) -> Result<(Login, AccountLogin), Error> {
        let refused = refusal(provider, account);
        let adapter = self.provider(Some(provider))?;
        let listed = self.settings.accounts.sourced(provider);
        if !listed.value.iter().any(|name| name == account) {
            let reason = match listed.source {
                Source::ProductDefault => format!(
                    "`accounts.{provider}` is not set, so the provider has the `{}` account alone",
                    cockpit_config::DEFAULT_ACCOUNT
                ),
                _ => format!(
                    "`accounts.{provider}` lists {}, not {account}",
                    listed.value.join(", ")
                ),
            };
            return Err(refused(reason));
        }
        let login = adapter.login()?.ok_or_else(|| {
            refused(format!(
                "the {provider} adapter has no sign-in a login account can keep"
            ))
        })?;
        let folders = self
            .settings
            .sandbox
            .account_login(provider, account)
            .ok_or_else(|| refused("the daemon read no host paths to keep it in".to_owned()))?
            .map_err(Error::Sandbox)?;
        Ok((login, folders))
    }
}

fn refusal(provider: &str, account: &str) -> impl Fn(String) -> Error + use<> {
    let (provider, account) = (provider.to_owned(), account.to_owned());
    move |reason| Error::SignIn {
        provider: provider.clone(),
        account: account.clone(),
        reason,
    }
}

/// `login`'s file under a home, as a relative path.
fn relative(login: &Login) -> PathBuf {
    login.file.components().collect()
}

fn create_owner_folder(folder: &Path) -> Result<(), Error> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .map_err(|source| Error::Io {
            action: "create",
            path: folder.to_path_buf(),
            source,
        })
}

fn remove_folder(folder: &Path) -> Result<(), Error> {
    match std::fs::remove_dir_all(folder) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(Error::Io {
            action: "remove",
            path: folder.to_path_buf(),
            source: error,
        }),
        _ => Ok(()),
    }
}
