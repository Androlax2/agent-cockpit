//! From the daemon's launch spec to bubblewrap's arguments and stage 2's policy. The mount
//! view holds only what the spec names; everything else of the host, the owner's home
//! included, is not there to read.

use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use cockpit_sandbox::{EgressDecisions, LaunchSpec, Protected, SYSTEM_READ_ONLY, WhenAbsent};
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
    /// The launcher's hand-off socket, where stage 2 sends the proxy listener before
    /// anything else (#831). Its folder is bound read-only and granted to no one.
    pub proxy_handoff: PathBuf,
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
/// its launch (#434); then refuses one the launcher cannot enforce, given the owner's
/// `decisions` at launch (#674).
pub fn read_spec(path: &Path, decisions: &EgressDecisions) -> Result<LaunchSpec, String> {
    let text = std::fs::read(path)
        .map_err(|error| format!("cannot read the launch spec {}: {error}", path.display()))?;
    std::fs::remove_file(path)
        .map_err(|error| format!("cannot remove the launch spec {}: {error}", path.display()))?;
    let spec: LaunchSpec = serde_json::from_slice(&text)
        .map_err(|error| format!("the launch spec {} is malformed: {error}", path.display()))?;
    spec.validate(decisions)
        .map_err(|error| format!("the launch spec is refused: {error}"))?;
    Ok(spec)
}

/// Reads and removes `mount_dir`'s credentials file (#832), as the spec is (#434): first of
/// all, so no refusal of the spec leaves a value behind. None when there is none.
pub fn take_credentials(mount_dir: &Path) -> Result<Option<Vec<u8>>, String> {
    let path = mount_dir.join(cockpit_sandbox::CREDENTIALS_FILE);
    let text = match std::fs::read(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot read the credentials file {}: {error}",
                path.display()
            ));
        }
    };
    std::fs::remove_file(&path).map_err(|error| {
        format!(
            "cannot remove the credentials file {}: {error}",
            path.display()
        )
    })?;
    Ok(Some(text))
}

/// The bound credentials' values in the credentials file's `text`, one per credential `spec`
/// binds. A file beside a spec that binds none is refused, as is a missing one beside a spec
/// that binds some.
pub fn credential_values(text: Option<Vec<u8>>, spec: &LaunchSpec) -> Result<Vec<String>, String> {
    let file = cockpit_sandbox::CREDENTIALS_FILE;
    let bound = spec
        .interception
        .as_ref()
        .map_or(0, |interception| interception.credentials.len());
    let Some(text) = text else {
        if bound > 0 {
            return Err(format!(
                "the spec binds {bound} credentials, and {file} is missing"
            ));
        }
        return Ok(Vec::new());
    };
    if bound == 0 {
        return Err(format!(
            "{file} is present, and the spec binds no credential"
        ));
    }
    // The values never appear in an error: serde's message could quote them.
    let values: Vec<String> = serde_json::from_slice(&text)
        .map_err(|_| format!("the credentials file {file} is malformed"))?;
    if values.len() != bound {
        return Err(format!(
            "the credentials file holds {} values for {bound} bound credentials",
            values.len()
        ));
    }
    Ok(values)
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

/// The bubblewrap arguments and stage 2 policy for `spec`, run by `launcher` inside, which
/// hands the proxy listener over on `handoff`, a socket in a folder of the launcher's own.
pub fn plan(spec: &LaunchSpec, launcher: &Path, handoff: &Path) -> Result<Plan, String> {
    let handoff_folder = handoff
        .parent()
        .ok_or("the proxy hand-off socket has no folder")?;
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
    mounts.extend(
        spec.folders
            .iter()
            .chain(&spec.login)
            .map(|folder| Mount::Elsewhere {
                source: folder.source.clone(),
                destination: folder.destination.clone(),
            }),
    );
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
            .chain([
                &program,
                &launcher.to_path_buf(),
                &handoff_folder.to_path_buf(),
            ])
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
    write.extend(
        spec.folders
            .iter()
            .chain(&spec.login)
            .map(|folder| folder.destination.clone()),
    );
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
            proxy_handoff: handoff.to_path_buf(),
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
        // Pinning the folders of a path left unprotected when absent would only make them
        // impossible to remove.
        if *when_absent == WhenAbsent::Unprotected
            && std::fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            continue;
        }
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
                    // A file where the path needs a folder, such as a worktree's `.git` that
                    // names its Git folder: nothing exists under it, and rewriting it would
                    // point the owner's Git at a folder of the agent's, so it is read-only.
                    let is_file_on_the_way = !is_last && !metadata.is_dir();
                    if is_last || is_file_on_the_way {
                        if !read_only.contains(&current) {
                            read_only.push(current.clone());
                        }
                    } else if !pinned.contains(&current) {
                        pinned.push(current.clone());
                    } else {
                        continue;
                    }
                    if !verify.contains(&expected) {
                        verify.push(expected);
                    }
                    if is_file_on_the_way {
                        break;
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
/// the view at all. A link is masked where it leads, when that is in the view: a `.env`
/// that links to `config/secrets.env` hides the secrets, not only the link's name.
fn hide(spec: &LaunchSpec, mounts: &[Mount]) -> Result<Vec<OsString>, String> {
    let is_bound = |path: &Path| {
        mounts.iter().any(|mount| match mount {
            Mount::ReadOnly(folder) | Mount::ReadWrite(folder) => path.starts_with(folder),
            // Shows the session's own folder, not the host's at that path.
            Mount::Elsewhere { .. } => false,
        })
    };
    let mut masked: Vec<PathBuf> = Vec::new();
    for pattern in &spec.hidden {
        if !is_bound(pattern) {
            continue;
        }
        for path in matching(pattern)? {
            let target = match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    match std::fs::canonicalize(&path) {
                        Ok(target) if is_bound(&target) => target,
                        _ => continue,
                    }
                }
                Ok(_) => path,
                Err(_) => continue,
            };
            if !masked.contains(&target) {
                masked.push(target);
            }
        }
    }
    let mut arguments = Vec::new();
    for path in masked {
        let is_folder = std::fs::metadata(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?
            .is_dir();
        if is_folder {
            arguments.extend(["--perms", "0000", "--tmpfs"].map(OsString::from));
            arguments.push(path.as_os_str().to_owned());
            arguments.push("--remount-ro".into());
            arguments.push(path.into_os_string());
        } else {
            arguments.extend(words("--ro-bind", Path::new("/dev/null"), &path));
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
