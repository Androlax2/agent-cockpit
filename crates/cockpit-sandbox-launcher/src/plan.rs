//! From the daemon's launch spec to bubblewrap's arguments and stage 2's policy. The mount
//! view holds only what the spec names; everything else of the host, the owner's home
//! included, is not there to read.

use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use cockpit_sandbox::{LaunchSpec, Protected, SYSTEM_READ_ONLY, WhenAbsent};
use serde::{Deserialize, Serialize};

/// What stage 2 enforces inside the namespaces, passed as its one argument: paths and rules,
/// never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub landlock_abi: u8,
    /// Read and run, never write.
    pub read: Vec<PathBuf>,
    /// Read, and connect to the named sockets inside.
    pub sockets: Vec<PathBuf>,
    /// Every file right.
    pub write: Vec<PathBuf>,
    /// Folders and files bound from inside a writable folder, which the program of an
    /// earlier launch, or another session in the same checkout, could have replaced by a
    /// link before bubblewrap mounted them: each must still be the file the launcher saw.
    pub verify: Vec<Expected>,
    /// The program's resolved file, bound read-only at its own path.
    pub program: PathBuf,
    /// The name the spec gave it, its `argv[0]`: a link's name selects some programs'
    /// behaviour.
    pub program_name: PathBuf,
    pub arguments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expected {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug)]
pub struct Plan {
    pub bubblewrap_arguments: Vec<OsString>,
    pub policy: Policy,
}

impl Plan {
    pub fn policy_argument(&self) -> Result<String, String> {
        serde_json::to_string(&self.policy)
            .map_err(|error| format!("cannot write stage 2's policy: {error}"))
    }
}

/// The namespaces every launch gets: the network one holds only `lo`.
const NAMESPACES: [&str; 7] = [
    "--unshare-user",
    "--unshare-pid",
    "--unshare-net",
    "--unshare-ipc",
    "--unshare-uts",
    "--unshare-cgroup-try",
    "--die-with-parent",
];

/// Reads the spec at `path` and removes the file before anything else, so no spec outlives
/// its launch (#434); then refuses one the launcher cannot enforce.
pub fn read_spec(path: &Path) -> Result<LaunchSpec, String> {
    let text = std::fs::read(path)
        .map_err(|error| format!("cannot read the launch spec {}: {error}", path.display()))?;
    std::fs::remove_file(path)
        .map_err(|error| format!("cannot remove the launch spec {}: {error}", path.display()))?;
    let spec: LaunchSpec = serde_json::from_slice(&text)
        .map_err(|error| format!("the launch spec {} is malformed: {error}", path.display()))?;
    spec.validate()
        .map_err(|error| format!("the launch spec is refused: {error}"))?;
    Ok(spec)
}

/// One mount of the view, applied parents first.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Mount {
    ReadOnly(PathBuf),
    ReadWrite(PathBuf),
    /// A folder of the session's own at another path: its home, a provider's fixed folder.
    Elsewhere {
        source: PathBuf,
        destination: PathBuf,
    },
}

impl Mount {
    fn destination(&self) -> &Path {
        match self {
            Mount::ReadOnly(path) | Mount::ReadWrite(path) => path,
            Mount::Elsewhere { destination, .. } => destination,
        }
    }

    fn arguments(&self) -> Vec<OsString> {
        match self {
            Mount::ReadOnly(path) => words("--ro-bind", path, path),
            Mount::ReadWrite(path) => words("--bind", path, path),
            Mount::Elsewhere {
                source,
                destination,
            } => words("--bind", source, destination),
        }
    }
}

fn words(flag: &str, source: &Path, destination: &Path) -> Vec<OsString> {
    vec![
        flag.into(),
        source.as_os_str().to_owned(),
        destination.as_os_str().to_owned(),
    ]
}

/// The bubblewrap arguments and stage 2 policy for `spec`, run by `launcher` inside.
pub fn plan(spec: &LaunchSpec, launcher: &Path) -> Result<Plan, String> {
    let program = spec
        .program
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", spec.program.display()))?;
    let mut arguments: Vec<OsString> = NAMESPACES.iter().map(OsString::from).collect();
    arguments.extend(["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"].map(OsString::from));
    let system: Vec<PathBuf> = SYSTEM_READ_ONLY
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect();
    for path in &system {
        arguments.extend(words("--ro-bind", path, path));
    }
    let mut mounts = vec![Mount::Elsewhere {
        source: spec.home_source.clone(),
        destination: spec.home.clone(),
    }];
    mounts.extend(spec.folders.iter().map(|folder| Mount::Elsewhere {
        source: folder.source.clone(),
        destination: folder.destination.clone(),
    }));
    mounts.extend(spec.read_write.iter().cloned().map(Mount::ReadWrite));
    // A provider's declared read it did not install is not there to bind.
    let declared: Vec<PathBuf> = spec
        .read_only_declared
        .iter()
        .filter(|path| path.exists())
        .cloned()
        .collect();
    mounts.extend(
        spec.read_only
            .iter()
            .chain(&declared)
            .chain(&spec.sockets)
            .chain([&program, &launcher.to_path_buf()])
            .cloned()
            .map(Mount::ReadOnly),
    );
    for mount in &mounts {
        require_present(mount)?;
    }
    // Parents first, so a folder bound inside another lands on top of it.
    mounts.sort_by_key(|mount| mount.destination().components().count());
    mounts.dedup();
    for mount in &mounts {
        arguments.extend(mount.arguments());
    }
    let masks = protect(spec)?;
    arguments.extend(masks.arguments);
    arguments.extend(hide(spec, &mounts)?);
    arguments.push("--chdir".into());
    arguments.push(spec.working_dir.as_os_str().to_owned());
    let mut read: Vec<PathBuf> = system;
    read.push("/proc".into());
    read.extend(spec.read_only.iter().cloned());
    read.extend(declared);
    read.push(program.clone());
    read.push(launcher.to_path_buf());
    let mut write: Vec<PathBuf> = vec!["/dev".into(), "/tmp".into(), spec.home.clone()];
    write.extend(spec.folders.iter().map(|folder| folder.destination.clone()));
    write.extend(spec.read_write.iter().cloned());
    Ok(Plan {
        bubblewrap_arguments: arguments,
        policy: Policy {
            landlock_abi: spec.landlock_abi,
            read,
            sockets: spec.sockets.clone(),
            write,
            verify: masks.verify,
            program,
            program_name: spec.program.clone(),
            arguments: spec.arguments.clone(),
        },
    })
}

/// A bound folder or file must exist: a missing one fails the launch, never narrows it.
fn require_present(mount: &Mount) -> Result<(), String> {
    let source = match mount {
        Mount::ReadOnly(path) | Mount::ReadWrite(path) => path,
        Mount::Elsewhere { source, .. } => source,
    };
    std::fs::metadata(source)
        .map(drop)
        .map_err(|error| format!("cannot bind {}: {error}", source.display()))
}

struct Masks {
    arguments: Vec<OsString>,
    verify: Vec<Expected>,
}

/// Each protected path read-only inside its writable folder. Its folders inside the
/// writable one are bound onto themselves first, so the program cannot rename one away
/// with the read-only mount inside it and put a writable one in its place. A path absent
/// at launch gets what its [`WhenAbsent`] says.
fn protect(spec: &LaunchSpec) -> Result<Masks, String> {
    let mut pinned: Vec<PathBuf> = Vec::new();
    let mut read_only: Vec<PathBuf> = Vec::new();
    let mut emptied: Vec<PathBuf> = Vec::new();
    let mut verify = Vec::new();
    for Protected { path, when_absent } in &spec.protected {
        let folder = spec
            .read_write
            .iter()
            .filter(|folder| path.starts_with(folder))
            .max_by_key(|folder| folder.components().count())
            .ok_or_else(|| format!("protected path {} is in no writable folder", path.display()))?;
        let mut current = folder.clone();
        let relative = path.strip_prefix(folder).map_err(|_| {
            format!(
                "protected path {} is outside {}",
                path.display(),
                folder.display()
            )
        })?;
        let names: Vec<_> = relative.components().collect();
        for (index, name) in names.iter().enumerate() {
            current.push(name);
            let is_last = index + 1 == names.len();
            match std::fs::symlink_metadata(&current) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if *when_absent == WhenAbsent::EmptyFolder && !emptied.contains(&current) {
                        emptied.push(current.clone());
                    }
                    break;
                }
                Err(error) => {
                    return Err(format!("cannot read {}: {error}", current.display()));
                }
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "{} is a symbolic link where the sandbox protects {}; replace it with \
                         the folder or file itself",
                        current.display(),
                        path.display()
                    ));
                }
                Ok(metadata) => {
                    let expected = Expected {
                        path: current.clone(),
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    };
                    if is_last {
                        read_only.push(current.clone());
                    } else if metadata.is_dir() && !pinned.contains(&current) {
                        pinned.push(current.clone());
                    } else if !metadata.is_dir() {
                        // A file where the path needs a folder: nothing can be created under
                        // it, and the file itself stays writable.
                        break;
                    } else {
                        continue;
                    }
                    if !verify.contains(&expected) {
                        verify.push(expected);
                    }
                }
            }
        }
    }
    pinned.sort_by_key(|path| path.components().count());
    let mut arguments = Vec::new();
    for path in &pinned {
        arguments.extend(words("--bind", path, path));
    }
    for path in &read_only {
        arguments.extend(words("--ro-bind", path, path));
    }
    for path in &emptied {
        arguments.extend(["--perms", "0555", "--tmpfs"].map(OsString::from));
        arguments.push(path.as_os_str().to_owned());
        arguments.push("--remount-ro".into());
        arguments.push(path.as_os_str().to_owned());
    }
    Ok(Masks { arguments, verify })
}

/// Each hidden path that exists inside a bound folder, masked: a folder by an empty
/// read-only one, anything else by `/dev/null`. One outside every bound folder is not in
/// the view at all; a link resolves inside the view, where its target is bound or not on its
/// own terms.
fn hide(spec: &LaunchSpec, mounts: &[Mount]) -> Result<Vec<OsString>, String> {
    let mut arguments = Vec::new();
    for pattern in &spec.hidden {
        let is_bound = mounts.iter().any(|mount| match mount {
            Mount::ReadOnly(folder) | Mount::ReadWrite(folder) => pattern.starts_with(folder),
            // Shows the session's own folder, not the host's at that path.
            Mount::Elsewhere { .. } => false,
        });
        if !is_bound {
            continue;
        }
        for path in matching(pattern)? {
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {
                    arguments.extend(["--perms", "0000", "--tmpfs"].map(OsString::from));
                    arguments.push(path.as_os_str().to_owned());
                    arguments.push("--remount-ro".into());
                    arguments.push(path.into_os_string());
                }
                Ok(metadata) if !metadata.file_type().is_symlink() => {
                    arguments.extend(words("--ro-bind", Path::new("/dev/null"), &path));
                }
                _ => {}
            }
        }
    }
    Ok(arguments)
}

/// `pattern` itself, or, when its last name ends in `*`, every entry of its folder whose
/// name starts with the rest.
fn matching(pattern: &Path) -> Result<Vec<PathBuf>, String> {
    let (Some(folder), Some(name)) = (pattern.parent(), pattern.file_name()) else {
        return Ok(vec![pattern.to_path_buf()]);
    };
    let Some(prefix) = name.to_str().and_then(|name| name.strip_suffix('*')) else {
        return Ok(vec![pattern.to_path_buf()]);
    };
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot list {}: {error}", folder.display())),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot list {}: {error}", folder.display()))?;
        if entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(prefix.as_bytes())
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}
