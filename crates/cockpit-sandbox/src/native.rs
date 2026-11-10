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

use crate::credentials::{
    BUNDLE_VARIABLES, BoundCredential, CA_BUNDLE_FILE, CREDENTIALS_FILE, CredentialValue,
    EXTRA_CA_VARIABLE, Interception, PLACEHOLDER, SESSION_CA_FILE, SYSTEM_BUNDLES, SpecCredential,
};
use crate::egress::{AllowEntry, Allowlist, EgressDecisions, PROXY_ADDRESS, Target};
use crate::spec::{
    LANDLOCK_ABI_FLOOR, LaunchSpec, NeedsSource, Protected, SPEC_FILE, SPEC_VERSION,
    SYSTEM_READ_ONLY, Subject, WhenAbsent,
};
use crate::{
    AppliedRule, Enforcement, Evidence, HostPaths, Prepared, Profile, ProviderReach, Refusal,
    SecretHandling,
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
    /// Where the session proxy lets the harness go (#831): its declared hosts, and the
    /// registries of the ecosystems its checkout uses (#859), each with its source.
    pub egress: Vec<AllowEntry>,
}

impl ProviderNeeds {
    /// Nothing declared: the floor.
    pub fn floor() -> ProviderNeeds {
        ProviderNeeds {
            source: NeedsSource::Floor,
            read: Vec::new(),
            credential_files: Vec::new(),
            credential_variables: Vec::new(),
            egress: Vec::new(),
        }
    }
}

/// What the checkout's toolchains and Git ask of a sandboxed launch (#859), resolved by the
/// caller from the toolchain table. An unsandboxed launch gets none of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolNeeds {
    /// The ecosystems detected in the checkout, as the evidence names them.
    pub ecosystems: Vec<String>,
    /// The owner's install roots where the launch's `PATH` resolves those ecosystems' tools:
    /// bound read-only when present.
    pub install_roots: Vec<PathBuf>,
    /// The variables that keep caches and tool configuration in the session's own folders.
    pub variables: Vec<(String, ToolSetting)>,
    /// The owner's Git identity, copied as values into the session's Git configuration.
    pub git_identity: GitIdentity,
}

impl ToolNeeds {
    /// No toolchain, no variable, no identity.
    pub fn none() -> ToolNeeds {
        ToolNeeds {
            ecosystems: Vec::new(),
            install_roots: Vec::new(),
            variables: Vec::new(),
            git_identity: GitIdentity::default(),
        }
    }
}

/// The value a tool variable gets in a sandboxed launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSetting {
    /// A folder of this name in the session's cache folder, inside its temp folder.
    CacheFolder(String),
    /// An empty file of this name in the session's tool configuration folder, which stands
    /// in for the owner's configuration and its tokens.
    ConfigFile(String),
    /// This value as it is; an empty one turns a wrapper off.
    Value(String),
    /// This flag added, after a space, to the value the launch already gives the variable.
    Flag(String),
}

/// `user.name` and `user.email` of the owner's Git configuration, each absent when unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitIdentity {
    pub name: Option<String>,
    pub email: Option<String>,
}

/// The session's caches, inside its temp folder: removed at its end, counted by
/// `torlek storage` (#859).
const CACHE_FOLDER: &str = "caches";

/// The session's tool configuration folder, inside its folder of the daemon's mount
/// sources, bound read-only: no agent can replace a file the daemon writes there.
const CONFIG_FOLDER: &str = "config";

/// The Git configuration Torlek writes for a sandboxed session, which `GIT_CONFIG_GLOBAL`
/// names: the session's home holds none of the owner's.
const GIT_CONFIG_FILE: &str = "gitconfig";

/// What must exist on disk before the launcher runs: created by [`Staged::write`], the one
/// place the daemon creates a sandboxed session's mount sources; the session's end removes them
/// (#482).
#[derive(Debug, Clone)]
pub struct Staged {
    /// The session's folder in the daemon's mount sources: the spec and folder mounts.
    pub mount_dir: PathBuf,
    /// The program's own home, created with its parent: a session's, kept across its
    /// relaunches, or a check run's.
    pub home_source: PathBuf,
    pub folder_sources: Vec<PathBuf>,
    /// The session's tool configuration folder, and each file Torlek writes there with its
    /// contents.
    pub config_dir: PathBuf,
    pub config_files: Vec<(String, String)>,
    pub spec: LaunchSpec,
    /// The owner's decisions on the subject's network requests when it launches (#674).
    pub decisions: EgressDecisions,
    /// The bound credentials' values, in the order of the spec's interception (#832).
    pub credential_values: Vec<CredentialValue>,
    /// What the session's home holds for its login before the launcher binds it (#59).
    pub login: Option<LoginEntry>,
}

/// The folder inside a session's home that a [`crate::LoginMount::FolderAndLink`] login is
/// bound at.
const LOGIN_FOLDER: &str = ".torlek-login";

/// What [`Staged::write`] puts in the session's own home folder, at the paths the launcher
/// binds the login on (#59). The agent wrote that folder in its last launch, so a link or
/// any other entry it left in the way is replaced, and a link among the folders above
/// refuses the launch: the bind never lands where the agent pointed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEntry {
    /// An empty file the login file is bound on.
    File(PathBuf),
    /// An empty folder the account's folder is bound on, and a link naming the login in it.
    FolderAndLink {
        folder: PathBuf,
        link: PathBuf,
        target: PathBuf,
    },
}

impl Staged {
    pub fn spec_path(&self) -> PathBuf {
        self.mount_dir.join(SPEC_FILE)
    }

    /// Where [`Staged::write`] puts the bound credentials' values; None when the launch binds
    /// none. The launcher removes the file as it starts.
    pub fn credentials_path(&self) -> Option<PathBuf> {
        (!self.credential_values.is_empty()).then(|| self.mount_dir.join(CREDENTIALS_FILE))
    }

    /// Creates the owner-only folders and writes the spec, the decisions and the bound
    /// credentials' values, owner-only, replacing what a failed launch left; removes the last
    /// launch's egress counters and refused destinations, so the evidence never shows them
    /// as this launch's and the daemon files only this launch's requests. Each folder the
    /// sandbox mounts starts empty, so an entry the last launch's provider left, such as
    /// Codex's socket, never answers for this one (#482).
    pub fn write(&self) -> std::io::Result<()> {
        let mut folders = std::fs::DirBuilder::new();
        folders.recursive(true).mode(0o700);
        folders.create(&self.mount_dir)?;
        folders.create(&self.home_source)?;
        for source in &self.folder_sources {
            remove_entry(source)?;
            folders.create(source)?;
        }
        match &self.login {
            Some(LoginEntry::File(file)) => {
                real_folders_above(&self.home_source, file)?;
                if !std::fs::symlink_metadata(file).is_ok_and(|entry| entry.is_file()) {
                    remove_entry(file)?;
                    replace_owner_only(file, b"")?;
                }
            }
            Some(LoginEntry::FolderAndLink {
                folder,
                link,
                target,
            }) => {
                real_folders_above(&self.home_source, folder)?;
                if !std::fs::symlink_metadata(folder).is_ok_and(|entry| entry.is_dir()) {
                    remove_entry(folder)?;
                    folders.create(folder)?;
                }
                real_folders_above(&self.home_source, link)?;
                remove_entry(link)?;
                std::os::unix::fs::symlink(target, link)?;
            }
            None => {}
        }
        folders.create(&self.config_dir)?;
        for (name, contents) in &self.config_files {
            replace_owner_only(&self.config_dir.join(name), contents.as_bytes())?;
        }
        remove_if_present(&self.mount_dir.join(crate::EGRESS_FILE))?;
        remove_if_present(&self.mount_dir.join(crate::REQUESTS_FILE))?;
        let decisions = serde_json::to_vec(&self.decisions).map_err(std::io::Error::other)?;
        replace_atomically(&self.mount_dir.join(crate::DECISIONS_FILE), &decisions)?;
        let credentials = self.mount_dir.join(CREDENTIALS_FILE);
        if self.credential_values.is_empty() {
            remove_if_present(&credentials)?;
        } else {
            let values: Vec<&str> = self
                .credential_values
                .iter()
                .map(CredentialValue::expose)
                .collect();
            let text = serde_json::to_vec(&values).map_err(std::io::Error::other)?;
            replace_owner_only(&credentials, &text)?;
        }
        let text = serde_json::to_vec(&self.spec).map_err(std::io::Error::other)?;
        replace_owner_only(&self.spec_path(), &text)
    }
}

/// Creates the folders between `home` and `path`, owner-only; refuses one that is a link
/// or no folder.
fn real_folders_above(home: &Path, path: &Path) -> std::io::Result<()> {
    let relative = path
        .parent()
        .and_then(|parent| parent.strip_prefix(home).ok())
        .ok_or_else(|| {
            std::io::Error::other(format!("{} is outside {}", path.display(), home.display()))
        })?;
    let mut current = home.to_path_buf();
    for name in relative.components() {
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(entry) if entry.is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::other(format!(
                    "{} is a link or a file, not a folder: the login is not bound through it",
                    current.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new().mode(0o700).create(&current)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Removes the folder, file or link at `path` and what is inside it, never following a link.
fn remove_entry(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        removed => removed,
    }
}

/// Writes `contents` to `path`, owner-only, through a file beside it: a reader polling `path`
/// sees the old contents or the new, never none (#674). The file beside it is this write's
/// own: a launch staging the decisions and the daemon's resync rewriting them at once each
/// rename their own file, never take the other's (#984).
pub(crate) fn replace_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let write = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut staged = path.as_os_str().to_owned();
    staged.push(format!(".staged-{}-{write}", std::process::id()));
    let staged = PathBuf::from(staged);
    replace_owner_only(&staged, contents)
        .and_then(|()| std::fs::rename(&staged, path))
        .inspect_err(|_| {
            // No later write reuses this name: the write's own error is the one returned.
            let _ = std::fs::remove_file(&staged);
        })
}

/// Writes `contents` to a new owner-only file at `path`, replacing one an earlier launch left.
fn replace_owner_only(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// One sandboxed program's own facts, beside its profile: a session's provider or a check
/// (#862).
pub(crate) struct Launch {
    pub subject: Subject,
    /// Where the program starts, inside the checkout.
    pub start_dir: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
    pub needs: ProviderNeeds,
    pub tools: ToolNeeds,
    /// Whether the program reaches Torlek's agent socket, bridge programs and instruction
    /// bundle: a session's provider does, a check never.
    pub reaches_agent: bool,
    /// The credentials the session proxy adds (#832); a check binds none.
    pub credentials: Vec<BoundCredential>,
    /// The owner's decisions when it launches (#674); a check's are none.
    pub decisions: EgressDecisions,
    /// The login account's sign-in a session uses (#59); a check binds none.
    pub login: Option<crate::SessionLogin>,
    /// The declared services the launcher serves (#880); a check serves none.
    pub forwards: Vec<crate::Forward>,
}

/// The launch of `program` with `arguments` for `launch` through `native`: the launcher in
/// the program's place, the spec it reads, and the evidence.
#[allow(
    clippy::too_many_arguments,
    reason = "one launch's facts and the host's, each placed into the spec"
)]
pub(crate) fn prepare(
    native: &NativeBackend,
    host: &HostPaths,
    profile: &Profile,
    launch: &Launch,
    mount_dir: PathBuf,
    home_source: PathBuf,
    program: PathBuf,
    arguments: Vec<OsString>,
) -> Result<Prepared, Refusal> {
    if let Some(endpoint) = profile.desktop_endpoints.first() {
        return Err(Refusal::shared_desktop(endpoint));
    }
    if let Some(secret) = profile
        .secrets
        .iter()
        .find(|secret| secret.handling == SecretHandling::Placeholder)
    {
        return Err(Refusal::new(
            format!("secret {} as a placeholder", secret.variable),
            "a secret in the environment never crosses as a placeholder; bind it to its \
             hosts as a credential instead (#832)",
        ));
    }
    let home = home_folder(&launch.environment)?;
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
    let read_only_declared = launch
        .needs
        .read
        .iter()
        .map(|path| in_home(&home, path))
        .chain(launch.tools.install_roots.iter().cloned().map(Ok))
        .collect::<Result<Vec<_>, _>>()?;
    let config_dir = mount_dir.join(CONFIG_FOLDER);
    let tool_environment = tool_environment(
        &launch.tools,
        &launch.environment,
        &profile.temp_dir.join(CACHE_FOLDER),
        &config_dir,
        &profile.working_dir,
    )?;
    let credential_files = launch
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
    let (agent_read_only, agent_sockets) = if launch.reaches_agent {
        (host.read_only.clone(), host.sockets.clone())
    } else {
        (Vec::new(), Vec::new())
    };
    let interception = interception(launch, &config_dir)?;
    let login = launch
        .login
        .as_ref()
        .map(|login| login_bind(&home, &home_source, login))
        .transpose()?;
    let spec = LaunchSpec {
        version: SPEC_VERSION,
        subject: launch.subject.clone(),
        bubblewrap: native.bubblewrap.clone(),
        landlock_abi: native.landlock_abi,
        program,
        arguments,
        working_dir: launch.start_dir.clone(),
        home,
        home_source: home_source.clone(),
        read_only: agent_read_only
            .into_iter()
            .chain([config_dir.clone()])
            .collect(),
        read_only_declared,
        sockets: agent_sockets,
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
        login: login.as_ref().map(|(bind, _)| bind.clone()),
        protected,
        hidden,
        needs_source: launch.needs.source.clone(),
        forwards: launch.forwards.clone(),
        egress: Allowlist {
            program: program_name(&launch.subject, profile),
            entries: launch.needs.egress.clone(),
        },
        interception,
    };
    spec.validate(&launch.decisions)
        .map_err(|error| Refusal::new("launch spec", error.to_string()))?;
    let mut environment: Vec<(OsString, OsString)> = launch
        .environment
        .iter()
        .filter(|(name, _)| {
            let is_credential = launch
                .needs
                .credential_variables
                .iter()
                .any(|variable| name == variable.as_str());
            let is_replaced = tool_environment
                .variables
                .iter()
                .any(|(variable, _)| variable == name);
            // `GIT_CONFIG_COUNT` and its pairs, or `GIT_CONFIG_PARAMETERS`, could name a
            // credential helper again.
            let is_git_configuration = name.to_string_lossy().starts_with("GIT_CONFIG");
            !is_credential
                && !is_replaced
                && !is_git_configuration
                && !is_proxy_variable(name)
                && !is_interception_variable(launch, name)
        })
        .cloned()
        .chain(proxy_variables())
        .collect();
    environment.extend(tool_environment.variables);
    if let Some(interception) = &spec.interception {
        environment.extend(interception_variables(launch, interception));
    }
    let staged = Staged {
        mount_dir,
        home_source,
        folder_sources: folder_mounts
            .iter()
            .map(|mount| mount.host.clone())
            .collect(),
        config_dir,
        config_files: tool_environment.files,
        spec,
        decisions: launch.decisions.clone(),
        credential_values: launch
            .credentials
            .iter()
            .map(|credential| credential.value.clone())
            .collect(),
        login: login.map(|(_, entry)| entry),
    };
    let evidence = evidence(native, profile, launch, &staged);
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

/// The variables that send clients through the session proxy (#546 M4 A); the owner's own
/// proxy settings would point at nothing the sandbox reaches.
const PROXY_VARIABLES: [&str; 6] = [
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
];

/// The variables that keep the agent's own servers on its loopback direct.
const NO_PROXY_VARIABLES: [&str; 2] = ["NO_PROXY", "no_proxy"];

/// Loopback inside the sandbox: the agent's own servers, never the host's.
const NO_PROXY: &str = "localhost,127.0.0.1,::1";

fn is_proxy_variable(name: &OsStr) -> bool {
    PROXY_VARIABLES
        .iter()
        .chain(&NO_PROXY_VARIABLES)
        .any(|variable| name == *variable)
        || name == "NODE_USE_ENV_PROXY"
}

fn proxy_variables() -> Vec<(OsString, OsString)> {
    let proxy = format!("http://{PROXY_ADDRESS}");
    let mut variables: Vec<(OsString, OsString)> = PROXY_VARIABLES
        .iter()
        .map(|name| (name.into(), proxy.clone().into()))
        .collect();
    variables.extend(
        NO_PROXY_VARIABLES
            .iter()
            .map(|name| (name.into(), NO_PROXY.into())),
    );
    // Node's built-in `fetch` and `http` honor the variables only with it (Node 24.0, 22.21).
    variables.push(("NODE_USE_ENV_PROXY".into(), "1".into()));
    variables
}

/// What the launcher intercepts for `launch`'s bound credentials (#832), its trust files in
/// the session's read-only tool configuration folder `config_dir`; None without one.
fn interception(launch: &Launch, config_dir: &Path) -> Result<Option<Interception>, Refusal> {
    if launch.credentials.is_empty() {
        return Ok(None);
    }
    Ok(Some(Interception {
        credentials: launch
            .credentials
            .iter()
            .map(|credential| SpecCredential {
                name: credential.name.clone(),
                hosts: credential.hosts.clone(),
            })
            .collect(),
        system_bundle: system_bundle(&launch.environment)?,
        ca_file: config_dir.join(SESSION_CA_FILE),
        bundle_file: config_dir.join(CA_BUNDLE_FILE),
    }))
}

/// The host's CA bundle: the owner's `SSL_CERT_FILE` when the launch environment sets one,
/// else the first of [`SYSTEM_BUNDLES`] present. Refused when there is none, rather than
/// verifying upstream servers against nothing.
fn system_bundle(environment: &[(OsString, OsString)]) -> Result<PathBuf, Refusal> {
    let refused = |reason: String| Refusal::new("host CA bundle", reason);
    let owners = environment
        .iter()
        .rev()
        .find(|(name, _)| name == "SSL_CERT_FILE")
        .map(|(_, value)| PathBuf::from(value));
    if let Some(path) = owners {
        if !path.is_absolute() || !path.is_file() {
            return Err(refused(format!(
                "SSL_CERT_FILE names {}, which is no file at an absolute path",
                path.display()
            )));
        }
        return Ok(path);
    }
    SYSTEM_BUNDLES
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or_else(|| {
            refused(format!(
                "none of {} exists; set SSL_CERT_FILE in the daemon's environment to the \
                 host's CA bundle",
                SYSTEM_BUNDLES.join(", ")
            ))
        })
}

/// A variable the interception sets: the trust variables and the credentials' placeholders,
/// which nothing of the owner's may replace.
fn is_interception_variable(launch: &Launch, name: &OsStr) -> bool {
    if launch.credentials.is_empty() {
        return false;
    }
    BUNDLE_VARIABLES
        .iter()
        .chain([&EXTRA_CA_VARIABLE])
        .any(|variable| name == *variable)
        || launch
            .credentials
            .iter()
            .filter_map(|credential| credential.placeholder_variable.as_deref())
            .any(|variable| name == variable)
}

/// The trust variables per runtime, then each credential's placeholder.
fn interception_variables(
    launch: &Launch,
    interception: &Interception,
) -> Vec<(OsString, OsString)> {
    let mut variables: Vec<(OsString, OsString)> = BUNDLE_VARIABLES
        .iter()
        .map(|name| (name.into(), interception.bundle_file.clone().into()))
        .collect();
    variables.push((
        EXTRA_CA_VARIABLE.into(),
        interception.ca_file.clone().into(),
    ));
    variables.extend(
        launch
            .credentials
            .iter()
            .filter_map(|credential| credential.placeholder_variable.as_deref())
            .map(|variable| (variable.into(), PLACEHOLDER.into())),
    );
    variables
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

/// Where `login` is bound in the program's `home`, and what the session's own `home_source`
/// holds for it (#59, #858).
fn login_bind(
    home: &Path,
    home_source: &Path,
    login: &crate::SessionLogin,
) -> Result<(crate::spec::Folder, LoginEntry), Refusal> {
    let file = in_home(home, &login.file)?;
    let in_source = |path: &Path| {
        home_source.join(
            path.strip_prefix(home)
                .expect("in_home joins the declared path to the home"),
        )
    };
    let name = file.file_name().expect("in_home refuses an empty path");
    Ok(match login.mount {
        crate::LoginMount::File => (
            crate::spec::Folder {
                source: login.folder.join(name),
                destination: file.clone(),
            },
            LoginEntry::File(in_source(&file)),
        ),
        crate::LoginMount::FolderAndLink => {
            let folder = home.join(LOGIN_FOLDER);
            (
                crate::spec::Folder {
                    source: login.folder.clone(),
                    destination: folder.clone(),
                },
                LoginEntry::FolderAndLink {
                    folder: in_source(&folder),
                    link: in_source(&file),
                    target: folder.join(name),
                },
            )
        }
    })
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

/// The variables a sandboxed launch gets from its tools, and the configuration files Torlek
/// writes for them.
struct ToolEnvironment {
    variables: Vec<(OsString, OsString)>,
    files: Vec<(String, String)>,
}

fn tool_environment(
    tools: &ToolNeeds,
    launch_environment: &[(OsString, OsString)],
    cache_dir: &Path,
    config_dir: &Path,
    checkout: &Path,
) -> Result<ToolEnvironment, Refusal> {
    let mut variables: Vec<(OsString, OsString)> = vec![
        (
            "GIT_CONFIG_GLOBAL".into(),
            config_dir.join(GIT_CONFIG_FILE).into_os_string(),
        ),
        // `/etc/gitconfig` may name credential helpers, scoped to a host where an empty
        // generic helper does not reach.
        ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
    ];
    let mut files = vec![(
        GIT_CONFIG_FILE.to_owned(),
        git_config(&tools.git_identity, checkout)?,
    )];
    for (name, setting) in &tools.variables {
        let value = match setting {
            ToolSetting::CacheFolder(folder) => cache_dir.join(plain_name(folder)?).into(),
            ToolSetting::ConfigFile(file) => {
                let file = plain_name(file)?;
                if file == GIT_CONFIG_FILE {
                    return Err(Refusal::new(
                        format!("tool variable {name}"),
                        format!("{GIT_CONFIG_FILE} is the session's Git configuration"),
                    ));
                }
                files.push((file.to_owned(), String::new()));
                config_dir.join(file).into()
            }
            ToolSetting::Value(value) => value.into(),
            ToolSetting::Flag(flag) => {
                let launch_value = launch_environment
                    .iter()
                    .rev()
                    .find(|(variable, _)| variable == name.as_str())
                    .map(|(_, value)| value.clone())
                    .filter(|value| !value.is_empty());
                match launch_value {
                    Some(mut value) => {
                        value.push(" ");
                        value.push(flag);
                        value
                    }
                    None => flag.into(),
                }
            }
        };
        variables.push((name.into(), value));
    }
    Ok(ToolEnvironment { variables, files })
}

/// `name` when it is one plain file name, which cannot leave the folder it is joined to.
fn plain_name(name: &str) -> Result<&str, Refusal> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(_)), None) => Ok(name),
        _ => Err(Refusal::new(
            format!("tool setting {name}"),
            "a cache folder or configuration file is one plain name",
        )),
    }
}

/// The session's Git configuration: the owner's identity as values, the checkout as a safe
/// directory, and no generic credential helper. `GIT_CONFIG_NOSYSTEM` keeps
/// `/etc/gitconfig` out, so only the checkout's own configuration, which the agent cannot
/// change, may name a helper for a host.
fn git_config(identity: &GitIdentity, checkout: &Path) -> Result<String, Refusal> {
    let checkout = checkout.to_str().ok_or_else(|| {
        Refusal::new(
            "Git configuration",
            format!("the checkout {} is not UTF-8", checkout.display()),
        )
    })?;
    let mut text = String::from(
        "# Written by Torlek for one sandboxed session (#859); rewritten at each launch.\n",
    );
    if identity.name.is_some() || identity.email.is_some() {
        text.push_str("[user]\n");
        for (key, value) in [("name", &identity.name), ("email", &identity.email)] {
            if let Some(value) = value {
                text.push_str(&format!("\t{key} = {}\n", git_value(value)));
            }
        }
    }
    text.push_str(&format!("[safe]\n\tdirectory = {}\n", git_value(checkout)));
    text.push_str("[credential]\n\thelper = \"\"\n");
    Ok(text)
}

/// `value` quoted for a Git configuration file, so no value can start another line.
fn git_value(value: &str) -> String {
    let mut quoted = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// The program's name in what the proxy answers and the evidence says: the provider of a
/// session, the check of a check run.
fn program_name(subject: &Subject, profile: &Profile) -> String {
    match subject {
        Subject::Session { .. } => profile.provider.clone().unwrap_or_default(),
        Subject::Check { check, .. } => format!("check {check}"),
    }
}

/// Who the program runs for, as the evidence names it.
fn subject(subject: &Subject, profile: &Profile) -> String {
    match subject {
        Subject::Session { .. } => match &profile.provider {
            Some(provider) => format!("provider {provider}"),
            None => "provider: none".to_owned(),
        },
        Subject::Check { run, check } => format!(
            "subject: check run {run} of check {check}; no provider, agent socket, launch token \
             or session identity"
        ),
    }
}

fn evidence(
    native: &NativeBackend,
    profile: &Profile,
    launch: &Launch,
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
        rule(subject(&spec.subject, profile), BACKEND_NAME),
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
            "home {} is {} {}",
            spec.home.display(),
            match spec.subject {
                Subject::Session { .. } => "the session's own folder, kept across its relaunches,",
                Subject::Check { .. } => "the run's own folder",
            },
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
                profile.provider.as_deref().unwrap_or("the program")
            ),
            &mount_and_landlock,
        ));
    }
    if let Some(login) = &spec.login {
        rules.push(rule(
            format!(
                "login account's sign-in {} at {}, read-write: a refreshed sign-in reaches the account",
                login.source.display(),
                login.destination.display()
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
        let source = if launch.tools.install_roots.contains(path) {
            "toolchain install root"
        } else {
            "declared"
        };
        rules.push(rule(
            format!("read-only {} when present ({source})", path.display()),
            &mount_and_landlock,
        ));
    }
    let ecosystems = if launch.tools.ecosystems.is_empty() {
        "none".to_owned()
    } else {
        launch.tools.ecosystems.join(", ")
    };
    rules.push(rule(
        format!("toolchains detected in the checkout: {ecosystems}"),
        BACKEND_NAME,
    ));
    let cache_variables: Vec<&str> = launch
        .tools
        .variables
        .iter()
        .filter(|(_, setting)| matches!(setting, ToolSetting::CacheFolder(_)))
        .map(|(name, _)| name.as_str())
        .collect();
    rules.push(rule(
        format!(
            "cache mode: session-own, in {}, removed at its end; none of the owner's caches \
             is mounted ({})",
            profile.temp_dir.join(CACHE_FOLDER).display(),
            cache_variables.join(", ")
        ),
        "environment",
    ));
    let wrappers_off: Vec<&str> = launch
        .tools
        .variables
        .iter()
        .filter(|(_, setting)| *setting == ToolSetting::Value(String::new()))
        .map(|(name, _)| name.as_str())
        .collect();
    if !wrappers_off.is_empty() {
        rules.push(rule(
            format!("compiler wrappers off: {}", wrappers_off.join(", ")),
            "environment",
        ));
    }
    rules.push(rule(
        format!(
            "tool configuration: Torlek's files in {}, read-only; Git gets the owner's \
             identity as values and no credential helper",
            staged.config_dir.display()
        ),
        "bubblewrap read-only mount",
    ));
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
    rules.extend(egress_rules(&spec.egress));
    if let Some(interception) = &spec.interception {
        rules.extend(interception_rules(launch, interception));
    }
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
    for variable in &launch.needs.credential_variables {
        rules.push(rule(
            format!("credential variable {variable} not passed"),
            "environment",
        ));
    }
    rules.extend(forward_rules(&spec.forwards));
    rules.push(rule(
        format!("desktop session {}", profile.desktop.as_str()),
        "environment",
    ));
    let (identity, observed_subject) = match &spec.subject {
        Subject::Session { session } => (session, format!("subject: session {session}")),
        Subject::Check { run, .. } => {
            (&format!("check-{run}"), format!("subject: check run {run}"))
        }
    };
    Evidence {
        enforcement: Enforcement::Sandboxed {
            backend: BACKEND_NAME.to_owned(),
            engine: format!("bubblewrap {}", native.bubblewrap_version),
            client_identity: launch_identity(identity),
        },
        platform: profile.platform.name().to_owned(),
        rules,
        observed: vec![
            format!("landlock abi: {}", native.landlock_abi),
            "seccomp: filter applied before the program runs".to_owned(),
            "namespaces: user, mount, pid, network, ipc, uts, cgroup".to_owned(),
            observed_subject,
        ],
    }
}

/// Each forward the launcher serves, by the record a grant is bound to (#880).
fn forward_rules(forwards: &[crate::Forward]) -> Vec<AppliedRule> {
    let launcher = format!("{BACKEND_NAME} launcher");
    if forwards.is_empty() {
        return vec![AppliedRule {
            rule: "host-service forwards: none".to_owned(),
            enforced_by: BACKEND_NAME.to_owned(),
        }];
    }
    forwards
        .iter()
        .map(|forward| AppliedRule {
            rule: format!(
                "host-service forward: {}, relayed only while the owner allows that record",
                forward.record()
            ),
            enforced_by: launcher.clone(),
        })
        .collect()
}

/// The network a launch gets: the session proxy alone, and each destination it allows.
fn egress_rules(allowlist: &Allowlist) -> Vec<AppliedRule> {
    let proxy = format!("{BACKEND_NAME} session proxy");
    let rule = |text: String, enforced_by: &str| AppliedRule {
        rule: text,
        enforced_by: enforced_by.to_owned(),
    };
    let mut rules = vec![rule(
        format!("network: only through the session proxy at {PROXY_ADDRESS}; no DNS or UDP inside"),
        "network namespace",
    )];
    if allowlist.entries.is_empty() {
        rules.push(rule(
            format!("egress: no host, `{}` declares no hosts", allowlist.program),
            &proxy,
        ));
    }
    for entry in &allowlist.entries {
        let check = match &entry.target {
            Target::Name(_) if entry.port == crate::egress::TLS_PORT => ", TLS server name checked",
            Target::Name(_) | Target::Address(_) => "",
        };
        rules.push(rule(
            format!("egress: {entry} ({}){check}", entry.source),
            &proxy,
        ));
    }
    rules.push(rule(
        "egress: other hosts, IP literals, and loopback, private, link-local, shared, \
         multicast and reserved addresses refused unless granted by address"
            .to_owned(),
        &proxy,
    ));
    rules
}

/// The credentials a launch binds, where each is added and what the sandbox holds instead.
fn interception_rules(launch: &Launch, interception: &Interception) -> Vec<AppliedRule> {
    let proxy = format!("{BACKEND_NAME} session proxy");
    let rule = |text: String, enforced_by: &str| AppliedRule {
        rule: text,
        enforced_by: enforced_by.to_owned(),
    };
    let mut rules = Vec::new();
    for credential in &launch.credentials {
        for host in &credential.hosts {
            rules.push(rule(
                format!(
                    "credential: {} added on {}:443 {}; intercepted with a per-session CA \
                     whose key stays in the launcher's memory",
                    credential.name,
                    host.host,
                    host.scheme.describe()
                ),
                &proxy,
            ));
        }
        let held = match &credential.placeholder_variable {
            Some(variable) => format!("a placeholder in {variable}"),
            None => "nothing".to_owned(),
        };
        rules.push(rule(
            format!(
                "credential: {} never enters the sandbox, which holds {held}",
                credential.name
            ),
            "environment",
        ));
    }
    rules.push(rule(
        format!(
            "trust: the per-session CA in {EXTRA_CA_VARIABLE} ({}); the host's bundle {} with \
             it in {} ({}); every other host is a plain tunnel",
            interception.ca_file.display(),
            interception.system_bundle.display(),
            BUNDLE_VARIABLES.join(", "),
            interception.bundle_file.display()
        ),
        "environment",
    ));
    rules
}

/// One launch's own name, which an owner's observations name to attach to it.
fn launch_identity(subject: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{subject}-{nanos:x}")
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
    fn an_identity_value_cannot_start_another_configuration_line() {
        let identity = GitIdentity {
            name: Some("Owner\n[core]\n\tsshCommand = evil \"x\" \\".to_owned()),
            email: None,
        };

        let text = git_config(&identity, Path::new("/work/app")).unwrap();

        assert!(
            text.contains("\tname = \"Owner\\n[core]\\n\\tsshCommand = evil \\\"x\\\" \\\\\"\n"),
            "{text}"
        );
        assert!(!text.contains("\n[core]"), "{text}");
    }

    #[test]
    fn the_sessions_git_configuration_clears_every_credential_helper() {
        let text = git_config(&GitIdentity::default(), Path::new("/work/app")).unwrap();

        assert!(text.ends_with("[credential]\n\thelper = \"\"\n"), "{text}");
        assert!(!text.contains("[user]"), "{text}");
        assert!(text.contains("\tdirectory = \"/work/app\"\n"), "{text}");
    }

    #[test]
    fn a_tool_setting_that_is_not_one_plain_name_is_refused() {
        for name in ["../escape", "nested/name", "/absolute", ""] {
            let tools = ToolNeeds {
                variables: vec![(
                    "CARGO_HOME".to_owned(),
                    ToolSetting::CacheFolder(name.to_owned()),
                )],
                ..ToolNeeds::none()
            };

            let refused = tool_environment(
                &tools,
                &[],
                Path::new("/tmp/s1/caches"),
                Path::new("/run/mounts/s1/config"),
                Path::new("/work/app"),
            );

            assert!(refused.is_err(), "{name:?}");
        }
    }

    #[test]
    fn a_flag_joins_the_launch_value_of_its_variable() {
        let tools = ToolNeeds {
            variables: vec![(
                "GOFLAGS".to_owned(),
                ToolSetting::Flag("-modcacherw".to_owned()),
            )],
            ..ToolNeeds::none()
        };
        let launch = [("GOFLAGS".into(), "-mod=vendor".into())];

        let environment = tool_environment(
            &tools,
            &launch,
            Path::new("/tmp/s1/caches"),
            Path::new("/run/mounts/s1/config"),
            Path::new("/work/app"),
        )
        .unwrap();

        assert!(
            environment
                .variables
                .contains(&("GOFLAGS".into(), "-mod=vendor -modcacherw".into())),
            "{:?}",
            environment.variables
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
