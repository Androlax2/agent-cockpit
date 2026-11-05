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
/// place the daemon creates a sandboxed session's mount sources (#482 removes them).
#[derive(Debug, Clone)]
pub struct Staged {
    /// The session's folder in the daemon's mount sources: the spec and folder mounts.
    pub mount_dir: PathBuf,
    /// The session's own home folders' parent, then its own home.
    pub homes: PathBuf,
    pub home_source: PathBuf,
    pub folder_sources: Vec<PathBuf>,
    /// The session's tool configuration folder, and each file Torlek writes there with its
    /// contents.
    pub config_dir: PathBuf,
    pub config_files: Vec<(String, String)>,
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
        folders.create(&self.config_dir)?;
        for (name, contents) in &self.config_files {
            replace_owner_only(&self.config_dir.join(name), contents.as_bytes())?;
        }
        let text = serde_json::to_vec(&self.spec).map_err(std::io::Error::other)?;
        replace_owner_only(&self.spec_path(), &text)
    }
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
        .chain(request.tools.install_roots.iter().cloned().map(Ok))
        .collect::<Result<Vec<_>, _>>()?;
    let config_dir = mount_dir.join(CONFIG_FOLDER);
    let tool_environment = tool_environment(
        &request.tools,
        &profile.temp_dir.join(CACHE_FOLDER),
        &config_dir,
        &profile.working_dir,
    )?;
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
        read_only: host
            .read_only
            .iter()
            .cloned()
            .chain([config_dir.clone()])
            .collect(),
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
    let mut environment: Vec<(OsString, OsString)> = request
        .environment
        .iter()
        .filter(|(name, _)| {
            let is_credential = request
                .needs
                .credential_variables
                .iter()
                .any(|variable| name == variable.as_str());
            let is_replaced = tool_environment
                .variables
                .iter()
                .any(|(variable, _)| variable == name);
            !is_credential && !is_replaced
        })
        .cloned()
        .collect();
    environment.extend(tool_environment.variables);
    let staged = Staged {
        mount_dir,
        homes: host.homes.clone(),
        home_source,
        folder_sources: folder_mounts
            .iter()
            .map(|mount| mount.host.clone())
            .collect(),
        config_dir,
        config_files: tool_environment.files,
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

/// The variables a sandboxed launch gets from its tools, and the configuration files Torlek
/// writes for them.
struct ToolEnvironment {
    variables: Vec<(OsString, OsString)>,
    files: Vec<(String, String)>,
}

fn tool_environment(
    tools: &ToolNeeds,
    cache_dir: &Path,
    config_dir: &Path,
    checkout: &Path,
) -> Result<ToolEnvironment, Refusal> {
    let mut variables: Vec<(OsString, OsString)> = vec![(
        "GIT_CONFIG_GLOBAL".into(),
        config_dir.join(GIT_CONFIG_FILE).into_os_string(),
    )];
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
/// directory, and no credential helper, not even one `/etc/gitconfig` names.
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
    // An empty helper clears the list the system configuration started.
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
        let source = if request.tools.install_roots.contains(path) {
            "toolchain install root"
        } else {
            "declared"
        };
        rules.push(rule(
            format!("read-only {} when present ({source})", path.display()),
            &mount_and_landlock,
        ));
    }
    let ecosystems = if request.tools.ecosystems.is_empty() {
        "none".to_owned()
    } else {
        request.tools.ecosystems.join(", ")
    };
    rules.push(rule(
        format!("toolchains detected in the checkout: {ecosystems}"),
        BACKEND_NAME,
    ));
    let cache_variables: Vec<&str> = request
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
    let wrappers_off: Vec<&str> = request
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
                Path::new("/tmp/s1/caches"),
                Path::new("/run/mounts/s1/config"),
                Path::new("/work/app"),
            );

            assert!(refused.is_err(), "{name:?}");
        }
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
