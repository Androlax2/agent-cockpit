//! `torlek account sign-in` (#59, #858): the owner signs a login account in once, through the
//! harness's own sign-in run in their terminal with a fresh folder of Torlek's as its home;
//! the daemon then moves the one file the adapter names into the account's login folder. It
//! never opens that file: it checks what kind of entry it is, renames it and sets its mode.
//! `torlek account grant` then lets one project's sessions use that login.

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_config::Source;
use cockpit_provider_api::Login;
use cockpit_sandbox::{AccountLogin, LoginGrant, OwnerClient};

use super::{Sessions, create_session_dir, remove_tree, store};
use crate::Error;

/// Where a login waits in the login folder between leaving the sign-in's home and taking its
/// name, until it is known to be a plain file and owner-only.
const INCOMING: &str = ".incoming";

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
            create_session_dir(&home)
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
            let relative: PathBuf = login.file.components().collect();
            let left = folders.sign_in.join(&relative);
            let missing = || {
                refused(format!(
                    "the sign-in left no file at {}; {again}",
                    login.file
                ))
            };
            if !is_inside(&left, &folders.sign_in)? {
                return Err(refused(format!(
                    "the folder of {} the sign-in left is outside its home",
                    login.file
                )));
            }
            match std::fs::symlink_metadata(&left) {
                Ok(metadata) if metadata.file_type().is_file() => {}
                Ok(_) => return Err(refused(not_a_plain_file(&login))),
                Err(error) if error.kind() == ErrorKind::NotFound => return Err(missing()),
                Err(source) => {
                    return Err(Error::Io {
                        action: "inspect",
                        path: left,
                        source,
                    });
                }
            }
            create_session_dir(&folders.login)?;
            // Checked again once moved, where only the daemon writes: whatever runs in the
            // sign-in's home could swap the file for a link after the check above.
            let incoming = folders.login.join(INCOMING);
            rename(&left, &incoming)?;
            let is_file = std::fs::symlink_metadata(&incoming)
                .map(|metadata| metadata.file_type().is_file())
                .unwrap_or(false);
            if !is_file {
                remove_entry(&incoming)?;
                return Err(refused(not_a_plain_file(&login)));
            }
            std::fs::set_permissions(&incoming, std::fs::Permissions::from_mode(0o600)).map_err(
                |source| Error::Io {
                    action: "make owner-only",
                    path: incoming.clone(),
                    source,
                },
            )?;
            let file_name = relative
                .file_name()
                .expect("a home path names at least one entry");
            rename(&incoming, &folders.login.join(file_name))?;
            // The login is kept: what is left of its home is the harness's other files.
            if let Err(error) = remove_tree(&folders.sign_in) {
                tracing::warn!(
                    path = %folders.sign_in.display(),
                    %error,
                    "a sign-in's home stayed after its login was kept"
                );
            }
            Ok(folders.login)
        })
        .await
    }

    /// Removes the home of a sign-in that failed, with whatever it wrote, a login included.
    pub async fn discard_sign_in(&self, provider: &str, account: &str) -> Result<(), Error> {
        let (_, folders) = self.login_of(provider, account)?;
        self.blocking("remove a sign-in home", move || {
            remove_folder(&folders.sign_in)
        })
        .await
    }

    /// The owner lets `project`'s sessions use `account` of `provider`, a login account the
    /// owner lists, from the project's next launch. The account need not be signed in yet.
    pub async fn grant_login(
        &self,
        project: &str,
        provider: &str,
        account: &str,
        through: Option<OwnerClient>,
    ) -> Result<LoginGrant, Error> {
        let refused = move |reason| Error::LoginGrantRefused {
            project: project.to_owned(),
            provider: provider.to_owned(),
            account: account.to_owned(),
            reason,
        };
        self.listed_login(provider, account, refused)?;
        let write = self.database.write().await?;
        cockpit_projects::project_named(&write, project).await?;
        let grant =
            cockpit_sandbox::grant_login(&write, project, provider, account, through, store::now())
                .await?;
        write.commit().await?;
        Ok(grant)
    }

    /// The adapter's login and the account's folders, once the owner lists the account.
    fn login_of(&self, provider: &str, account: &str) -> Result<(Login, AccountLogin), Error> {
        let refused = refusal(provider, account);
        let login = self.listed_login(provider, account, &refused)?;
        let folders = self
            .settings
            .sandbox
            .account_login(provider, account)
            .ok_or_else(|| refused("the daemon read no host paths to keep it in".to_owned()))?
            .map_err(Error::Sandbox)?;
        Ok((login, folders))
    }
}

impl Sessions {
    /// The adapter's login, once the owner lists `account` for `provider`; `refused` says why
    /// it cannot be one.
    fn listed_login(
        &self,
        provider: &str,
        account: &str,
        refused: impl Fn(String) -> Error,
    ) -> Result<Login, Error> {
        let adapter = self.provider(Some(provider))?;
        let listed = self.settings.accounts.sourced(provider);
        if !listed.value.iter().any(|name| name == account) {
            return Err(refused(match listed.source {
                Source::ProductDefault => format!(
                    "`accounts.{provider}` is not set, so the provider has the `{}` account alone",
                    cockpit_config::DEFAULT_ACCOUNT
                ),
                _ => format!(
                    "`accounts.{provider}` lists {}, not {account}",
                    listed.value.join(", ")
                ),
            }));
        }
        adapter.login()?.ok_or_else(|| {
            refused(format!(
                "the {provider} adapter has no sign-in a login account can keep"
            ))
        })
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

fn not_a_plain_file(login: &Login) -> String {
    format!(
        "the sign-in left {} as something other than a plain file",
        login.file
    )
}

/// Whether `entry`'s folder resolves inside `home`: a link on the way would move a file from
/// outside it. An absent folder counts as inside, for the caller to find the entry absent.
fn is_inside(entry: &Path, home: &Path) -> Result<bool, Error> {
    let resolve = |path: &Path| {
        path.canonicalize().map_err(|source| Error::Io {
            action: "resolve",
            path: path.to_path_buf(),
            source,
        })
    };
    let home = resolve(home)?;
    let Some(folder) = entry.parent() else {
        return Ok(false);
    };
    match folder.canonicalize() {
        Ok(real) => Ok(real.starts_with(&home)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(true),
        Err(source) => Err(Error::Io {
            action: "resolve",
            path: folder.to_path_buf(),
            source,
        }),
    }
}

fn rename(from: &Path, to: &Path) -> Result<(), Error> {
    std::fs::rename(from, to).map_err(|source| Error::Io {
        action: "move the login to",
        path: to.to_path_buf(),
        source,
    })
}

fn remove_entry(path: &Path) -> Result<(), Error> {
    let removed = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => remove_tree(path),
        _ => std::fs::remove_file(path),
    };
    removed.map_err(|source| Error::Io {
        action: "remove",
        path: path.to_path_buf(),
        source,
    })
}

/// Removes `folder` and everything in it, read-only folders a tool left included; an absent
/// one is already gone.
fn remove_folder(folder: &Path) -> Result<(), Error> {
    match remove_tree(folder) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(Error::Io {
            action: "remove",
            path: folder.to_path_buf(),
            source: error,
        }),
        _ => Ok(()),
    }
}
