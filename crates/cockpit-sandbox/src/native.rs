//! The native backend (#829): `torlek-sandbox` starts each sandboxed program inside
//! bubblewrap's namespaces, then applies Landlock and a seccomp filter itself. The daemon
//! probes it once at its start; each launch under `agents.sandbox = "required"` writes a
//! [`LaunchSpec`] the launcher reads, and runs the launcher in the agent's place.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::spec::{
    LANDLOCK_ABI_FLOOR, LaunchSpec, NeedsSource, Protected, SPEC_FILE, SPEC_VERSION,
    SYSTEM_READ_ONLY, Subject, WhenAbsent,
};
use crate::{
    AppliedRule, Enforcement, Evidence, HostPaths, LaunchRequest, Prepared, Profile, ProviderReach,
    Refusal, SecretHandling,
};

/// The backend name the evidence records.
const BACKEND_NAME: &str = "torlek-sandbox";

/// Protected paths of the checkout, relative to its root, that Torlek or Git act on outside
/// the sandbox: an absent one gets an empty read-only folder (#829).
const PROTECTED_FOLDERS: &[&str] = &[".git/hooks", ".git/config", ".torlek", ".cockpit"];

/// Protected paths of the checkout an editor or shell tool runs only once the owner trusts
/// them: protected when present at launch, left creatable when absent, where an empty folder
/// would break the owner's tools.
const PROTECTED_WHEN_PRESENT: &[&str] = &[
    // Submodules' Git folders, with their own hooks and configuration.
    ".git/modules",
    ".envrc",
    ".vscode/tasks.json",
    ".vscode/launch.json",
    ".idea/runConfigurations",
];

/// Files of the checkout no agent reads: environment files, which hold the project's
/// secrets (`BASELINE_DENIED_READ`).
const HIDDEN_IN_CHECKOUT: &[&str] = &[".env", ".env.*"];

/// What the daemon's probe found: the sandbox launches under `required` run through, or why
/// none can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    Native(NativeBackend),
    /// Every launch under `required` is refused with this reason.
    Missing {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeBackend {
    /// `torlek-sandbox`, beside the daemon.
    pub launcher: PathBuf,
    /// The bubblewrap program found on the daemon's `PATH`.
    pub bubblewrap: PathBuf,
    pub bubblewrap_version: String,
    /// The Landlock ABI a sandboxed process got in the probe; every launch enforces exactly
    /// this one's rights or fails.
    pub landlock_abi: u8,
}

impl NativeBackend {
    /// One line for the daemon's log and readiness.
    pub fn describe(&self) -> String {
        format!(
            "{BACKEND_NAME} with bubblewrap {} and Landlock ABI {}",
            self.bubblewrap_version, self.landlock_abi
        )
    }
}

#[derive(Deserialize)]
struct ProbeReport {
    bubblewrap: String,
    landlock_abi: u8,
}

impl Backend {
    /// Runs the launcher's probe launch with the first `bwrap` on `search_path`: the native
    /// backend when a sandboxed process starts and gets Landlock at or above the floor;
    /// otherwise why not, which every launch under `required` then names.
    pub fn probe(launcher: &Path, search_path: Option<&OsStr>) -> Backend {
        match probe_native(launcher, search_path) {
            Ok(native) => Backend::Native(native),
            Err(reason) => Backend::Missing { reason },
        }
    }

    /// The reason for [`Backend::Missing`], None for a usable backend.
    pub fn missing_reason(&self) -> Option<&str> {
        match self {
            Backend::Native(_) => None,
            Backend::Missing { reason } => Some(reason),
        }
    }
}

fn probe_native(launcher: &Path, search_path: Option<&OsStr>) -> Result<NativeBackend, String> {
    let bubblewrap = find_program("bwrap", search_path).ok_or_else(|| {
        "bubblewrap (`bwrap`) is not on the daemon's PATH; install it (package `bubblewrap`) \
         and restart the daemon"
            .to_owned()
    })?;
    if !launcher.is_file() {
        return Err(format!(
            "the sandbox launcher {} is missing beside the daemon",
            launcher.display()
        ));
    }
    let output = Command::new(launcher)
        .arg("probe")
        .arg(&bubblewrap)
        .output()
        .map_err(|error| format!("cannot run {}: {error}", launcher.display()))?;
    if !output.status.success() {
        return Err(format!(
            "the sandbox probe failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let report: ProbeReport = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "the sandbox probe printed {:?}, not its report: {error}",
            String::from_utf8_lossy(&output.stdout)
        )
    })?;
    if report.landlock_abi < LANDLOCK_ABI_FLOOR {
        return Err(format!(
            "the kernel gives a sandboxed process Landlock ABI {}, below the floor, {}; \
             a kernel of Linux 5.19 or later with Landlock enabled is needed",
            report.landlock_abi, LANDLOCK_ABI_FLOOR
        ));
    }
    Ok(NativeBackend {
        launcher: launcher.to_path_buf(),
        bubblewrap,
        bubblewrap_version: report.bubblewrap,
        landlock_abi: report.landlock_abi,
    })
}

fn find_program(name: &str, search_path: Option<&OsStr>) -> Option<PathBuf> {
    std::env::split_paths(search_path?)
        .filter(|folder| folder.is_absolute())
        .map(|folder| folder.join(name))
        .find(|candidate| candidate.is_file())
}

/// What the provider adapter declares for the sandbox (#830, #858), as the launch request
/// carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderNeeds {
    pub source: NeedsSource,
    /// Home folders the harness reads, `~/` paths as declared: the owner's, bound read-only
    /// when present.
    pub read: Vec<String>,
    /// Where the harness keeps credentials, `~/` paths: never shown, even inside a folder it
    /// reads.
    pub credential_files: Vec<String>,
    /// Variables a credential reaches the harness in: never passed into the sandbox.
    pub credential_variables: Vec<String>,
}

impl ProviderNeeds {
    /// Nothing declared: the floor.
    pub fn floor() -> ProviderNeeds {
        ProviderNeeds {
            source: NeedsSource::Floor,
            read: Vec::new(),
            credential_files: Vec::new(),
            credential_variables: Vec::new(),
        }
    }
}

/// What must exist on disk before the launcher runs: created by [`Staged::write`], the one
/// place the daemon creates a sandboxed session's mount sources (#482 removes them).
#[derive(Debug, Clone)]
pub struct Staged {
    /// The session's folder in the daemon's mount sources: the spec and folder mounts.
    pub mount_dir: PathBuf,
    /// The session's own home folders' parent, then its own home.
    pub homes: PathBuf,
    pub home_source: PathBuf,
    pub folder_sources: Vec<PathBuf>,
    pub spec: LaunchSpec,
}

impl Staged {
    pub fn spec_path(&self) -> PathBuf {
        self.mount_dir.join(SPEC_FILE)
    }

    /// Creates the owner-only folders and writes the spec, owner-only, replacing one a failed
    /// launch left.
    pub fn write(&self) -> std::io::Result<()> {
        let mut folders = std::fs::DirBuilder::new();
        folders.recursive(true).mode(0o700);
        folders.create(&self.mount_dir)?;
        folders.create(&self.homes)?;
        folders.create(&self.home_source)?;
        for source in &self.folder_sources {
            folders.create(source)?;
        }
        let text = serde_json::to_vec(&self.spec).map_err(std::io::Error::other)?;
        let path = self.spec_path();
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(&text)?;
        file.sync_all()
    }
}

/// The launch of `program` with `arguments` for `request` through `native`: the launcher in
/// the program's place, the spec it reads, and the evidence.
#[allow(
    clippy::too_many_arguments,
    reason = "one launch's facts and the host's, each placed into the spec"
)]
pub(crate) fn prepare(
    native: &NativeBackend,
    host: &HostPaths,
    profile: &Profile,
    request: &LaunchRequest,
    mount_dir: PathBuf,
    home_source: PathBuf,
    program: PathBuf,
    arguments: Vec<OsString>,
) -> Result<Prepared, Refusal> {
    if let Some(endpoint) = profile.desktop_endpoints.first() {
        return Err(Refusal::new(
            "desktop session shared",
            format!(
                "the native sandbox does not show the owner's desktop yet ({} names {}); \
                 set agents.desktop_session = \"withheld\" to launch under the sandbox",
                endpoint.variable,
                endpoint.path.display()
            ),
        ));
    }
    if let Some(secret) = profile
        .secrets
        .iter()
        .find(|secret| secret.handling == SecretHandling::Placeholder)
    {
        return Err(Refusal::new(
            format!("secret {} as a placeholder", secret.variable),
            "no proxy swaps placeholders in this release (#831)",
        ));
    }
    let home = home_folder(&request.environment)?;
    let arguments = arguments
        .into_iter()
        .map(|argument| {
            argument.into_string().map_err(|argument| {
                Refusal::new(
                    "program arguments",
                    format!("argument {argument:?} is not UTF-8"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let read_only_declared = request
        .needs
        .read
        .iter()
        .map(|path| in_home(&home, path))
        .collect::<Result<Vec<_>, _>>()?;
    let credential_files = request
        .needs
        .credential_files
        .iter()
        .map(|path| in_home(&home, path))
        .collect::<Result<Vec<_>, _>>()?;
    let checkout = &profile.working_dir;
    let protected = PROTECTED_FOLDERS
        .iter()
        .map(|path| (path, WhenAbsent::EmptyFolder))
        .chain(
            PROTECTED_WHEN_PRESENT
                .iter()
                .map(|path| (path, WhenAbsent::Unprotected)),
        )
        .map(|(path, when_absent)| Protected {
            path: checkout.join(path),
            when_absent,
        })
        .collect();
    let hidden = HIDDEN_IN_CHECKOUT
        .iter()
        .map(|name| checkout.join(name))
        .chain(profile.denied.iter().cloned())
        .chain(credential_files)
        .collect();
    let folder_mounts = profile.folder_mounts.clone();
    let spec = LaunchSpec {
        version: SPEC_VERSION,
        subject: Subject::Session {
            session: request.session.clone(),
        },
        bubblewrap: native.bubblewrap.clone(),
        landlock_abi: native.landlock_abi,
        program,
        arguments,
        working_dir: request.start_dir.clone(),
        home,
        home_source: home_source.clone(),
        read_only: host.read_only.clone(),
        read_only_declared,
        sockets: host.sockets.clone(),
        read_write: vec![
            profile.working_dir.clone(),
            profile.session_dir.clone(),
            profile.temp_dir.clone(),
        ],
        folders: folder_mounts
            .iter()
            .map(|mount| crate::spec::Folder {
                source: mount.host.clone(),
                destination: mount.sandbox.clone(),
            })
            .collect(),
        protected,
        hidden,
        needs_source: request.needs.source.clone(),
        forwards: Vec::new(),
    };
    spec.validate()
        .map_err(|error| Refusal::new("launch spec", error.to_string()))?;
    let environment = request
        .environment
        .iter()
        .filter(|(name, _)| {
            !request
                .needs
                .credential_variables
                .iter()
                .any(|variable| name == variable.as_str())
        })
        .cloned()
        .collect();
    let staged = Staged {
        mount_dir,
        homes: host.homes.clone(),
        home_source,
        folder_sources: folder_mounts
            .iter()
            .map(|mount| mount.host.clone())
            .collect(),
        spec,
    };
    let evidence = evidence(native, profile, request, &staged);
    let reach = if folder_mounts.is_empty() {
        ProviderReach::Direct
    } else {
        ProviderReach::Mounted(folder_mounts)
    };
    Ok(Prepared {
        program: native.launcher.clone(),
        arguments: vec!["run".into(), staged.spec_path().into_os_string()],
        environment,
        evidence,
        reach,
        staged: Some(staged),
    })
}

/// The program's home: the owner's `HOME`, which the session's own folder stands in for.
fn home_folder(environment: &[(OsString, OsString)]) -> Result<PathBuf, Refusal> {
    let home = environment
        .iter()
        .rev()
        .find(|(name, _)| name == "HOME")
        .map(|(_, value)| PathBuf::from(value))
        .ok_or_else(|| Refusal::new("home", "the launch environment sets no HOME"))?;
    if !home.is_absolute() {
        return Err(Refusal::new(
            "home",
            format!("HOME {} is not an absolute path", home.display()),
        ));
    }
    Ok(home)
}

/// `~/x` under `home`; a pattern or any other shape is refused rather than narrowed.
fn in_home(home: &Path, declared: &str) -> Result<PathBuf, Refusal> {
    let relative = declared
        .strip_prefix("~/")
        .filter(|relative| !relative.contains('*') && !relative.is_empty())
        .ok_or_else(|| {
            Refusal::new(
                format!("declared path {declared}"),
                "the native sandbox binds `~/` paths without patterns only",
            )
        })?;
    Ok(home.join(relative))
}

fn evidence(
    native: &NativeBackend,
    profile: &Profile,
    request: &LaunchRequest,
    staged: &Staged,
) -> Evidence {
    let spec = &staged.spec;
    let landlock = format!("Landlock ABI {}", native.landlock_abi);
    let mount_and_landlock = format!("bubblewrap mount view, {landlock}");
    let rule = |rule: String, enforced_by: &str| AppliedRule {
        rule,
        enforced_by: enforced_by.to_owned(),
    };
    let mut rules = vec![
        rule(format!("platform {}", profile.platform), BACKEND_NAME),
        rule(format!("provider {}", profile.provider), BACKEND_NAME),
        rule(
            format!("needs: {}", spec.needs_source.describe()),
            BACKEND_NAME,
        ),
    ];
    for folder in &spec.read_write {
        rules.push(rule(
            format!("read-write {}", folder.display()),
            &mount_and_landlock,
        ));
    }
    rules.push(rule(
        format!(
            "home {} is the session's own folder {}, kept across its relaunches",
            spec.home.display(),
            spec.home_source.display()
        ),
        "bubblewrap mount view",
    ));
    for folder in &spec.folders {
        rules.push(rule(
            format!(
                "folder {} at {} ({}'s own, not the host's)",
                folder.source.display(),
                folder.destination.display(),
                profile.provider
            ),
            &mount_and_landlock,
        ));
    }
    // The launcher binds only the system folders this host has.
    for path in SYSTEM_READ_ONLY
        .iter()
        .map(PathBuf::from)
        .filter(|folder| folder.exists())
        .chain(spec.read_only.iter().cloned())
        .chain(spec.sockets.iter().cloned())
    {
        rules.push(rule(
            format!("read-only {}", path.display()),
            &mount_and_landlock,
        ));
    }
    for path in &spec.read_only_declared {
        rules.push(rule(
            format!("read-only {} when present (declared)", path.display()),
            &mount_and_landlock,
        ));
    }
    rules.push(rule(
        "read everything else: not mounted".to_owned(),
        &mount_and_landlock,
    ));
    for Protected { path, when_absent } in &spec.protected {
        let text = match when_absent {
            WhenAbsent::EmptyFolder => format!(
                "protect {}: read-only; when absent, an empty read-only folder is left there",
                path.display()
            ),
            WhenAbsent::Unprotected => format!(
                "protect {}: read-only when present at launch",
                path.display()
            ),
        };
        rules.push(rule(text, "bubblewrap read-only mount"));
    }
    for path in &spec.hidden {
        rules.push(rule(
            format!("hide {} inside a mounted folder", path.display()),
            "bubblewrap mask",
        ));
    }
    rules.push(rule(
        "network: none but lo (no proxy in this release, #831)".to_owned(),
        "network namespace",
    ));
    rules.push(rule(
        "system calls: tracing, mounts, namespaces, kernel modules, keys, BPF, io_uring and \
         terminal injection refused"
            .to_owned(),
        "seccomp",
    ));
    rules.push(rule(
        "processes: own PID namespace; signals and abstract sockets stay inside".to_owned(),
        &format!("bubblewrap, {landlock}"),
    ));
    rules.extend(profile.secrets.iter().map(|secret| {
        rule(
            format!("secret {} passed through", secret.variable),
            "environment",
        )
    }));
    for variable in &request.needs.credential_variables {
        rules.push(rule(
            format!("credential variable {variable} not passed"),
            "environment",
        ));
    }
    rules.push(rule("host-service forwards: none".to_owned(), BACKEND_NAME));
    rules.push(rule(
        format!("desktop session {}", profile.desktop.as_str()),
        "environment",
    ));
    Evidence {
        enforcement: Enforcement::Sandboxed {
            backend: BACKEND_NAME.to_owned(),
            engine: format!("bubblewrap {}", native.bubblewrap_version),
            client_identity: launch_identity(&request.session),
        },
        platform: profile.platform.name().to_owned(),
        rules,
        observed: vec![
            format!("landlock abi: {}", native.landlock_abi),
            "seccomp: filter applied before the program runs".to_owned(),
            "namespaces: user, mount, pid, network, ipc, uts, cgroup".to_owned(),
            format!("subject: session {}", request.session),
        ],
    }
}

/// One launch's own name, which an owner's observations name to attach to it.
fn launch_identity(session: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{session}-{nanos:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_home_path_with_a_pattern_is_refused_not_narrowed() {
        let refused = in_home(Path::new("/home/owner"), "~/.claude.json.*").unwrap_err();
        assert_eq!(refused.rule, "declared path ~/.claude.json.*");
    }

    #[test]
    fn a_declared_home_path_lands_under_the_owners_home() {
        assert_eq!(
            in_home(Path::new("/home/owner"), "~/.local/share/claude").unwrap(),
            PathBuf::from("/home/owner/.local/share/claude")
        );
    }

    #[test]
    fn the_probe_names_a_missing_bubblewrap() {
        let empty = tempfile::tempdir().unwrap();
        let backend = Backend::probe(Path::new("/nonexistent"), Some(empty.path().as_os_str()));
        assert!(
            backend.missing_reason().unwrap().contains("bubblewrap"),
            "{backend:?}"
        );
    }
}
