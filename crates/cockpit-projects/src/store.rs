//! The projects tables and every query on them.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_turso::sql::{Connection, IntoParams, Row, Value};
use cockpit_turso::{Migration, MigrationSet, WriteTransaction};

use crate::{
    Checkout, CheckoutId, CheckoutKind, CheckoutState, Error, Holder, Occupancy, Project,
    ProjectId, ProjectName, blocking,
};

/// The projects tables. A removed checkout keeps its row, so an id is never reused and a stale
/// reference reads as removed. `git_dir_marker` names the marker file Cockpit wrote into the
/// working copy's `.git` directory: a copy replaced outside Cockpit at the same path lacks it.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "projects",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE projects (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE CHECK (name <> ''),
                root TEXT NOT NULL UNIQUE CHECK (root <> ''),
                registered_at INTEGER NOT NULL
            );
            CREATE TABLE project_checkouts (
                id INTEGER PRIMARY KEY,
                project_id INTEGER NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN ('main', 'parallel', 'review')),
                branch TEXT CHECK ((kind = 'parallel') = (branch IS NOT NULL)),
                pull_request INTEGER CHECK ((kind = 'review') = (pull_request IS NOT NULL)),
                path TEXT NOT NULL CHECK (path <> ''),
                incarnation INTEGER NOT NULL CHECK (incarnation >= 1),
                git_dir_marker TEXT NOT NULL CHECK (git_dir_marker <> ''),
                state TEXT NOT NULL CHECK (state IN ('live', 'removing', 'deleting', 'removed')),
                created_at INTEGER NOT NULL,
                removed_at INTEGER CHECK ((state = 'removed') = (removed_at IS NOT NULL))
            );
            CREATE UNIQUE INDEX project_checkouts_one_per_path
                ON project_checkouts (path) WHERE state <> 'removed';
            CREATE UNIQUE INDEX project_checkouts_one_main
                ON project_checkouts (project_id) WHERE kind = 'main';
            CREATE TABLE checkout_occupancy (
                checkout_id INTEGER PRIMARY KEY,
                holder TEXT NOT NULL CHECK (holder <> ''),
                claimed_at INTEGER NOT NULL
            );",
    }],
};

/// A random marker file Cockpit writes into a working copy's `.git` directory when it records
/// the copy, `.git/cockpit-checkout-<token>`. A device and inode pair would not do: ext4 reuses
/// the inode of a deleted directory, and btrfs device numbers change across mounts.
pub(crate) const GIT_DIR_MARKER_PREFIX: &str = "cockpit-checkout-";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitDirIdentity {
    token: String,
}

impl GitDirIdentity {
    /// Writes a new marker into `<checkout>/.git`, which must be a real directory. Each record
    /// gets its own file, so registering a path again never disturbs an existing record.
    pub(crate) fn create(checkout: &Path) -> Result<GitDirIdentity, String> {
        let git_dir = real_git_dir(checkout)?;
        let mut random = [0_u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut source| std::io::Read::read_exact(&mut source, &mut random))
            .map_err(|error| format!("/dev/urandom: {error}"))?;
        let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let identity = GitDirIdentity { token };
        let marker = identity.marker(&git_dir);
        // `create_new` refuses an existing file or symbolic link instead of writing through it.
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
            .map_err(|error| format!("{}: {error}", marker.display()))?;
        Ok(identity)
    }

    /// Whether `<checkout>/.git` is a real directory holding this marker as a regular file.
    pub(crate) fn verify(&self, checkout: &Path) -> Result<(), String> {
        let git_dir = real_git_dir(checkout)?;
        let is_marked = std::fs::symlink_metadata(self.marker(&git_dir))
            .is_ok_and(|metadata| metadata.file_type().is_file());
        if !is_marked {
            return Err("its .git directory is not the one recorded".to_owned());
        }
        Ok(())
    }

    /// Deletes the marker of a record that was never committed.
    pub(crate) fn discard(&self, checkout: &Path) -> Result<(), Error> {
        let marker = self.marker(&checkout.join(".git"));
        std::fs::remove_file(&marker).map_err(Error::io("delete", &marker))
    }

    fn marker(&self, git_dir: &Path) -> PathBuf {
        git_dir.join(format!("{GIT_DIR_MARKER_PREFIX}{}", self.token))
    }
}

fn real_git_dir(checkout: &Path) -> Result<PathBuf, String> {
    let git_dir = checkout.join(".git");
    let metadata = std::fs::symlink_metadata(&git_dir)
        .map_err(|error| format!("{}: {error}", git_dir.display()))?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory", git_dir.display()));
    }
    Ok(git_dir)
}

/// A checkout row with what the crate needs beyond the public view.
#[derive(Clone)]
pub(crate) struct StoredCheckout {
    pub(crate) checkout: Checkout,
    identity: GitDirIdentity,
}

impl StoredCheckout {
    /// Refuses when the working copy on disk is not the recorded incarnation. Reads the
    /// disk off the async workers, since the checkout may sit on a slow mount.
    pub(crate) async fn verify_on_disk(&self) -> Result<(), Error> {
        let stored = self.clone();
        blocking(move || stored.verify_on_disk_now()).await
    }

    /// [`Self::verify_on_disk`] for code already off the async workers.
    pub(crate) fn verify_on_disk_now(&self) -> Result<(), Error> {
        let replaced = |reason: String| Error::Replaced {
            checkout: self.checkout.id,
            path: self.checkout.path.clone(),
            reason,
        };
        self.identity.verify(&self.checkout.path).map_err(replaced)
    }
}

pub(crate) fn now() -> i64 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs();
    i64::try_from(seconds).expect("seconds since 1970 fit in an i64")
}

/// A pull request number as stored: SQLite integers are signed.
pub(crate) fn pull_request_number(pull_request: u64) -> Result<i64, Error> {
    i64::try_from(pull_request).map_err(|_| Error::InvalidPullRequest { pull_request })
}

pub(crate) fn path_text(path: &Path) -> Result<&str, Error> {
    path.to_str().ok_or_else(|| Error::InvalidPath {
        path: path.to_path_buf(),
        reason: "the path is not valid UTF-8".to_owned(),
    })
}

/// Every registered project, by name.
pub async fn projects(connection: &Connection) -> Result<Vec<Project>, Error> {
    let mut rows = connection
        .query("SELECT id, name, root FROM projects ORDER BY name", ())
        .await?;
    let mut projects = Vec::new();
    while let Some(row) = rows.next().await? {
        projects.push(project_from_row(&row)?);
    }
    Ok(projects)
}

pub(crate) async fn project_named(connection: &Connection, name: &str) -> Result<Project, Error> {
    let mut rows = connection
        .query(
            "SELECT id, name, root FROM projects WHERE name = ?1",
            [name],
        )
        .await?;
    match rows.next().await? {
        Some(row) => project_from_row(&row),
        None => Err(Error::UnknownProject {
            name: name.to_owned(),
        }),
    }
}

const CHECKOUT_COLUMNS: &str = "c.id, p.name, p.root, c.kind, c.branch, c.pull_request, c.path,
    c.incarnation, c.state, c.git_dir_marker, o.holder, o.claimed_at";
const CHECKOUT_FROM: &str = "project_checkouts c
    JOIN projects p ON p.id = c.project_id
    LEFT JOIN checkout_occupancy o ON o.checkout_id = c.id";

/// Checkout rows matching `filter`, an SQL condition over `c` (checkout) and `p` (project),
/// by id.
async fn query_checkouts(
    connection: &Connection,
    filter: &str,
    params: impl IntoParams,
) -> Result<Vec<StoredCheckout>, Error> {
    let mut rows = connection
        .query(
            &format!("SELECT {CHECKOUT_COLUMNS} FROM {CHECKOUT_FROM} WHERE {filter} ORDER BY c.id"),
            params,
        )
        .await?;
    let mut checkouts = Vec::new();
    while let Some(row) = rows.next().await? {
        checkouts.push(checkout_from_row(&row)?);
    }
    Ok(checkouts)
}

/// Every checkout not yet removed, of `project` or of all projects, by id.
pub async fn checkouts(
    connection: &Connection,
    project: Option<&ProjectName>,
) -> Result<Vec<Checkout>, Error> {
    let project = project.map_or(Value::Null, |name| Value::Text(name.as_str().to_owned()));
    let stored = query_checkouts(
        connection,
        "c.state <> 'removed' AND (?1 IS NULL OR p.name = ?1)",
        [project],
    )
    .await?;
    Ok(stored.into_iter().map(|stored| stored.checkout).collect())
}

/// The checkout `id`, refusing one that was removed.
pub(crate) async fn stored_checkout(
    connection: &Connection,
    id: CheckoutId,
) -> Result<StoredCheckout, Error> {
    query_checkouts(connection, "c.id = ?1 AND c.state <> 'removed'", [id.0])
        .await?
        .pop()
        .ok_or(Error::UnknownCheckout { checkout: id })
}

/// The main checkout of `project`.
pub(crate) async fn main_checkout(
    connection: &Connection,
    project: ProjectId,
) -> Result<StoredCheckout, Error> {
    query_checkouts(
        connection,
        "c.project_id = ?1 AND c.kind = 'main'",
        [project.0],
    )
    .await?
    .pop()
    .ok_or_else(|| Error::InvalidStoredRow {
        reason: format!("project {} has no main checkout", project.0),
    })
}

/// Every clone (parallel or review) still live or being removed, by id.
pub(crate) async fn stored_clones(connection: &Connection) -> Result<Vec<StoredCheckout>, Error> {
    query_checkouts(connection, "c.state <> 'removed' AND c.kind <> 'main'", ()).await
}

/// Paths of every project root and checkout not yet removed, with their project.
pub(crate) async fn occupied_paths(
    connection: &Connection,
) -> Result<Vec<(ProjectName, PathBuf)>, Error> {
    let mut rows = connection
        .query(
            "SELECT p.name, p.root FROM projects p
             UNION ALL
             SELECT p.name, c.path FROM project_checkouts c JOIN projects p ON p.id = c.project_id
             WHERE c.state <> 'removed'",
            (),
        )
        .await?;
    let mut paths = Vec::new();
    while let Some(row) = rows.next().await? {
        let name = stored_name(row.get(0)?)?;
        paths.push((name, PathBuf::from(row.get::<String>(1)?)));
    }
    Ok(paths)
}

pub(crate) async fn insert_project(
    write: &WriteTransaction<'_>,
    name: &ProjectName,
    root: &Path,
) -> Result<Project, Error> {
    let mut taken = write
        .query("SELECT 1 FROM projects WHERE name = ?1", [name.as_str()])
        .await?;
    if taken.next().await?.is_some() {
        return Err(Error::NameTaken { name: name.clone() });
    }
    write
        .execute(
            "INSERT INTO projects (name, root, registered_at) VALUES (?1, ?2, ?3)",
            (name.as_str(), path_text(root)?, now()),
        )
        .await?;
    Ok(Project {
        id: ProjectId(write.last_insert_rowid()),
        name: name.clone(),
        root: root.to_path_buf(),
    })
}

/// Records a new working copy at `path`, with the incarnation after the last one there;
/// refused while another record still holds the path.
pub(crate) async fn insert_checkout(
    write: &WriteTransaction<'_>,
    project: &Project,
    kind: &CheckoutKind,
    path: &Path,
    identity: &GitDirIdentity,
) -> Result<Checkout, Error> {
    let path_value = path_text(path)?;
    let (kind_name, branch, pull_request) = match kind {
        CheckoutKind::Main => ("main", Value::Null, Value::Null),
        CheckoutKind::Parallel { branch } => ("parallel", Value::Text(branch.clone()), Value::Null),
        CheckoutKind::Review { pull_request } => (
            "review",
            Value::Null,
            Value::Integer(pull_request_number(*pull_request)?),
        ),
    };
    let mut holders = write
        .query(
            "SELECT 1 FROM project_checkouts WHERE path = ?1 AND state <> 'removed'",
            [path_value],
        )
        .await?;
    if holders.next().await?.is_some() {
        return Err(Error::PathTaken {
            path: path.to_path_buf(),
        });
    }
    drop(holders);
    let incarnation: i64 = write
        .prepare("SELECT coalesce(max(incarnation), 0) + 1 FROM project_checkouts WHERE path = ?1")
        .await?
        .query_row([path_value])
        .await?
        .get(0)?;
    write
        .execute(
            "INSERT INTO project_checkouts
                (project_id, kind, branch, pull_request, path, incarnation,
                 git_dir_marker, state, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'live', ?8)",
            (
                project.id.0,
                kind_name,
                branch,
                pull_request,
                path_value,
                incarnation,
                identity.token.as_str(),
                now(),
            ),
        )
        .await?;
    Ok(Checkout {
        id: CheckoutId(write.last_insert_rowid()),
        project: project.name.clone(),
        project_root: project.root.clone(),
        kind: kind.clone(),
        path: path.to_path_buf(),
        incarnation,
        state: CheckoutState::Live,
        occupancy: None,
    })
}

/// Gives `holder` the checkout `id` and returns it claimed. Claiming a checkout one already
/// holds leaves the occupancy unchanged; another holder's checkout is refused, naming that
/// holder. The working copy must still be the recorded incarnation.
pub async fn claim(
    write: &WriteTransaction<'_>,
    id: CheckoutId,
    holder: &Holder,
) -> Result<Checkout, Error> {
    let stored = stored_checkout(write, id).await?;
    let mut checkout = stored.checkout.clone();
    if checkout.state != CheckoutState::Live {
        return Err(Error::BeingRemoved { checkout: id });
    }
    if let Some(occupancy) = &checkout.occupancy {
        if &occupancy.holder == holder {
            return Ok(checkout);
        }
        return Err(Error::Occupied {
            checkout: id,
            holder: occupancy.holder.clone(),
        });
    }
    stored.verify_on_disk().await?;
    let claimed_at = now();
    write
        .execute(
            "INSERT INTO checkout_occupancy (checkout_id, holder, claimed_at) VALUES (?1, ?2, ?3)",
            (id.0, holder.as_str(), claimed_at),
        )
        .await?;
    checkout.occupancy = Some(Occupancy {
        holder: holder.clone(),
        claimed_at,
    });
    Ok(checkout)
}

/// Ends `holder`'s occupancy of the checkout `id`; only the occupant may.
pub async fn release(
    write: &WriteTransaction<'_>,
    id: CheckoutId,
    holder: &Holder,
) -> Result<(), Error> {
    stored_checkout(write, id).await?;
    let released = write
        .execute(
            "DELETE FROM checkout_occupancy WHERE checkout_id = ?1 AND holder = ?2",
            (id.0, holder.as_str()),
        )
        .await?;
    if released == 0 {
        return Err(Error::NotOccupant {
            checkout: id,
            holder: holder.clone(),
        });
    }
    Ok(())
}

/// Marks an unoccupied live clone as being removed; `false` when it is occupied by now.
pub(crate) async fn mark_removing(
    write: &WriteTransaction<'_>,
    id: CheckoutId,
) -> Result<bool, Error> {
    let marked = write
        .execute(
            "UPDATE project_checkouts SET state = 'removing'
             WHERE id = ?1 AND state = 'live'
               AND NOT EXISTS (SELECT 1 FROM checkout_occupancy WHERE checkout_id = ?1)",
            [id.0],
        )
        .await?;
    Ok(marked == 1)
}

/// Puts a checkout the sweep marked back to live, when it found work in it after all.
pub(crate) async fn mark_live(write: &WriteTransaction<'_>, id: CheckoutId) -> Result<(), Error> {
    write
        .execute(
            "UPDATE project_checkouts SET state = 'live' WHERE id = ?1 AND state = 'removing'",
            [id.0],
        )
        .await?;
    Ok(())
}

/// Records that the sweep's checks passed and deletion starts; `false` when another sweep
/// changed the state first.
pub(crate) async fn mark_deleting(
    write: &WriteTransaction<'_>,
    id: CheckoutId,
) -> Result<bool, Error> {
    let marked = write
        .execute(
            "UPDATE project_checkouts SET state = 'deleting' WHERE id = ?1 AND state = 'removing'",
            [id.0],
        )
        .await?;
    Ok(marked == 1)
}

pub(crate) async fn mark_removed(
    write: &WriteTransaction<'_>,
    id: CheckoutId,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE project_checkouts SET state = 'removed', removed_at = ?2
             WHERE id = ?1 AND state = 'deleting'",
            (id.0, now()),
        )
        .await?;
    Ok(())
}

fn stored_name(name: String) -> Result<ProjectName, Error> {
    ProjectName::new(name).map_err(|error| Error::InvalidStoredRow {
        reason: error.to_string(),
    })
}

fn project_from_row(row: &Row) -> Result<Project, Error> {
    Ok(Project {
        id: ProjectId(row.get(0)?),
        name: stored_name(row.get(1)?)?,
        root: PathBuf::from(row.get::<String>(2)?),
    })
}

/// Reads a row of [`CHECKOUT_COLUMNS`], refusing values this crate never writes.
fn checkout_from_row(row: &Row) -> Result<StoredCheckout, Error> {
    let id = CheckoutId(row.get(0)?);
    let invalid = |reason: String| Error::InvalidStoredRow {
        reason: format!("checkout {id}: {reason}"),
    };
    let kind = match (
        row.get::<String>(3)?.as_str(),
        row.get::<Option<String>>(4)?,
        row.get::<Option<i64>>(5)?,
    ) {
        ("main", None, None) => CheckoutKind::Main,
        ("parallel", Some(branch), None) => CheckoutKind::Parallel { branch },
        ("review", None, Some(number)) => CheckoutKind::Review {
            pull_request: u64::try_from(number)
                .map_err(|_| invalid(format!("pull request {number}")))?,
        },
        (kind, branch, number) => {
            return Err(invalid(format!(
                "kind {kind:?} with branch {branch:?} and pull request {number:?}"
            )));
        }
    };
    let state = match row.get::<String>(8)?.as_str() {
        "live" => CheckoutState::Live,
        "removing" => CheckoutState::Removing,
        "deleting" => CheckoutState::Deleting,
        other => return Err(invalid(format!("state {other:?}"))),
    };
    let occupancy = match (row.get::<Option<String>>(10)?, row.get::<Option<i64>>(11)?) {
        (Some(holder), Some(claimed_at)) => Some(Occupancy {
            holder: Holder::new(holder).map_err(|error| invalid(error.to_string()))?,
            claimed_at,
        }),
        (None, None) => None,
        (holder, claimed_at) => {
            return Err(invalid(format!(
                "occupancy {holder:?} claimed at {claimed_at:?}"
            )));
        }
    };
    Ok(StoredCheckout {
        checkout: Checkout {
            id,
            project: stored_name(row.get(1)?)?,
            project_root: PathBuf::from(row.get::<String>(2)?),
            kind,
            path: PathBuf::from(row.get::<String>(6)?),
            incarnation: row.get(7)?,
            state,
            occupancy,
        },
        identity: GitDirIdentity { token: row.get(9)? },
    })
}
