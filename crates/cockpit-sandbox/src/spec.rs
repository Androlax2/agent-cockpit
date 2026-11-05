//! The launch spec: everything `torlek-sandbox` needs to start one program inside the native
//! sandbox (#829), written by the daemon into the session's mount folder, which no agent
//! reaches, and removed by the launcher as soon as it read it (#434). It holds paths and
//! rules, never a secret: the program's environment reaches the launcher the way every
//! hosted launch's does.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::egress::{Allowlist, Target, normalized_name};

/// Bumped when a field changes meaning; the launcher refuses any other version, so a launcher
/// left from an older install never half-reads a newer daemon's spec.
pub const SPEC_VERSION: u32 = 2;

/// The file name of the spec in the session's mount folder.
pub const SPEC_FILE: &str = "launch-spec.json";

/// The lowest Landlock ABI the launcher enforces (#546 M2): under ABI 1 every rename or link
/// across folders is denied, which breaks build tools.
pub const LANDLOCK_ABI_FLOOR: u8 = 2;

/// Folders of the host every launch reads, never writes: programs, libraries and system
/// configuration. Bound when present. Nothing under `/run`, `/var` or `/home`: container
/// engines and other services keep their sockets there (#856).
pub const SYSTEM_READ_ONLY: &[&str] = &[
    "/usr", "/bin", "/sbin", "/lib", "/lib32", "/lib64", "/libx32", "/etc", "/opt", "/sys",
];

/// Who the sandboxed program runs for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Subject {
    /// An agent session's provider.
    Session { session: String },
    /// A run of a project's check (#862): no provider, agent socket, launch token or session
    /// identity.
    Check { run: String, check: String },
}

/// Where the program's declared needs came from (#861): the evidence names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum NeedsSource {
    /// A provider adapter's declaration (#830).
    Adapter { provider: String },
    /// The owner's configuration entry for an unadapted program (#84, #294), which no
    /// session reads or writes.
    OwnerEntry { program: String },
    /// Nothing declared: the floor, no folder of the home and no host.
    Floor,
}

impl NeedsSource {
    pub fn describe(&self) -> String {
        match self {
            NeedsSource::Adapter { provider } => format!("the {provider} adapter"),
            NeedsSource::OwnerEntry { program } => {
                format!("declared by owner config: the entry for {program}")
            }
            NeedsSource::Floor => "none declared: the floor".to_owned(),
        }
    }
}

/// A listener the launcher would create inside the sandbox and serve from outside (#860).
/// Always empty until host-service forwards exist; the launcher refuses a spec that has one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    pub listen: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub version: u32,
    pub subject: Subject,
    /// The bubblewrap program the daemon probed.
    pub bubblewrap: PathBuf,
    /// The Landlock ABI the daemon's probe found and the evidence records; the launcher
    /// enforces exactly this one's rights or refuses to start.
    pub landlock_abi: u8,
    /// The program to run, as its resolved file: bound read-only at this path.
    pub program: PathBuf,
    pub arguments: Vec<String>,
    /// Where the program starts, inside [`LaunchSpec::read_write`].
    pub working_dir: PathBuf,
    /// The program's `HOME`.
    pub home: PathBuf,
    /// The session's own folder shown at [`LaunchSpec::home`]: kept across its relaunches,
    /// never the owner's home (#858, #859).
    pub home_source: PathBuf,
    /// Folders and files bound read-only at their own path, beside [`SYSTEM_READ_ONLY`]:
    /// the provider's declared reads, the bridge programs, the instruction bundle.
    pub read_only: Vec<PathBuf>,
    /// Folders and files the provider declares it reads (#830), and the install roots of
    /// the checkout's toolchains (#859): bound read-only at their own path when present.
    pub read_only_declared: Vec<PathBuf>,
    /// Folders whose named sockets the program connects to, bound read-only: the agent
    /// socket's folder.
    pub sockets: Vec<PathBuf>,
    /// Folders bound read-write: the checkout, the session's directory and temp folder.
    pub read_write: Vec<PathBuf>,
    /// Folders of the session's own, in the daemon's mount sources, bound read-write at
    /// another path: Codex's socket folder (#446).
    pub folders: Vec<Folder>,
    /// Paths inside a [`LaunchSpec::read_write`] folder the program reads but never
    /// changes: Git hooks and configuration, the project's Torlek folder.
    pub protected: Vec<Protected>,
    /// Paths inside a bound folder the program never reads: Torlek's own files and
    /// credential files. A last name ending in `*` stands for every entry of its folder
    /// that starts with the rest. Absent ones stay absent.
    pub hidden: Vec<PathBuf>,
    pub needs_source: NeedsSource,
    pub forwards: Vec<Forward>,
    /// What the session proxy lets out of the sandbox (#831).
    pub egress: Allowlist,
}

/// A folder bound read-write at another path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// One path the program must not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Protected {
    pub path: PathBuf,
    pub when_absent: WhenAbsent,
}

/// What a protected path absent at launch gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhenAbsent {
    /// An empty read-only folder at its first absent name, so nothing can be created there.
    /// Bubblewrap creates that folder in the checkout, where it stays: for paths whose
    /// contents Torlek or Git would act on outside the sandbox.
    EmptyFolder,
    /// Nothing: the program may create it. For files an editor or shell tool reads only
    /// once the owner trusts them, where an empty folder would break the owner's tools.
    Unprotected,
}

/// Why a spec is refused, before anything starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSpec(pub String);

impl std::fmt::Display for InvalidSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InvalidSpec {}

impl LaunchSpec {
    /// Refuses a spec the launcher cannot enforce as written: another version, a relative
    /// path, a Landlock ABI below the floor, a protected or hidden path outside every bound
    /// folder, a working directory outside the writable ones, a forward, or an allowlist entry
    /// that is no host name or address and a port.
    pub fn validate(&self) -> Result<(), InvalidSpec> {
        if self.version != SPEC_VERSION {
            return Err(InvalidSpec(format!(
                "spec version {} is not this launcher's {SPEC_VERSION}",
                self.version
            )));
        }
        if self.landlock_abi < LANDLOCK_ABI_FLOOR {
            return Err(InvalidSpec(format!(
                "Landlock ABI {} is below the floor, {LANDLOCK_ABI_FLOOR}",
                self.landlock_abi
            )));
        }
        let single = [
            &self.bubblewrap,
            &self.program,
            &self.working_dir,
            &self.home,
            &self.home_source,
        ];
        let lists = [
            &self.read_only,
            &self.read_only_declared,
            &self.sockets,
            &self.read_write,
            &self.hidden,
        ];
        let protected = self.protected.iter().map(|protected| &protected.path);
        let folders = self
            .folders
            .iter()
            .flat_map(|folder| [&folder.source, &folder.destination]);
        for path in single
            .into_iter()
            .chain(lists.into_iter().flatten())
            .chain(protected)
            .chain(folders)
        {
            check_plain(path)?;
        }
        if !self
            .read_write
            .iter()
            .any(|folder| self.working_dir.starts_with(folder))
        {
            return Err(InvalidSpec(format!(
                "the working directory {} is in no writable folder",
                self.working_dir.display()
            )));
        }
        for Protected { path, .. } in &self.protected {
            if !self
                .read_write
                .iter()
                .any(|folder| path.starts_with(folder) && path != folder)
            {
                return Err(InvalidSpec(format!(
                    "protected path {} is inside no writable folder",
                    path.display()
                )));
            }
        }
        check_allowlist(&self.egress)?;
        if let Some(forward) = self.forwards.first() {
            return Err(InvalidSpec(format!(
                "forward {} to {}: host-service forwards do not exist yet (#860)",
                forward.listen, forward.target
            )));
        }
        Ok(())
    }
}

/// Every entry names a port, and a name entry a lower-case DNS name of at least two labels,
/// or `*.` and one: the proxy matches what the spec says, never a pattern it widened.
fn check_allowlist(allowlist: &Allowlist) -> Result<(), InvalidSpec> {
    for entry in &allowlist.entries {
        let is_valid = entry.port != 0
            && match &entry.target {
                Target::Name(pattern) => {
                    let name = pattern.strip_prefix("*.").unwrap_or(pattern);
                    normalized_name(name).as_deref() == Some(name) && name.contains('.')
                }
                Target::Address(_) => true,
            };
        if !is_valid {
            return Err(InvalidSpec(format!(
                "allowlist entry {entry} ({}) is not a host name or address and a port",
                entry.source
            )));
        }
    }
    Ok(())
}

/// An absolute path without `.` or `..`: what the launcher binds is what the spec names.
fn check_plain(path: &Path) -> Result<(), InvalidSpec> {
    let is_plain = path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        });
    if is_plain {
        Ok(())
    } else {
        Err(InvalidSpec(format!(
            "{} is not an absolute path without `.` or `..`",
            path.display()
        )))
    }
}
