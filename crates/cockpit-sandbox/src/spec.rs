//! The launch spec: everything `torlek-sandbox` needs to start one program inside the native
//! sandbox (#829), written by the daemon into the session's mount folder, which no agent
//! reaches, and removed by the launcher as soon as it read it (#434). It holds paths and
//! rules, never a secret: the program's environment reaches the launcher the way every
//! hosted launch's does, and bound credentials' values travel in their own file (#832).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::credentials::{CredentialScheme, Interception};
use crate::egress::{Allowlist, EgressDecisions, TLS_PORT, Target, normalized_name};
use crate::forwards::{
    CONTAINERS_SERVICE, Endpoint, FIRST_UNPRIVILEGED_PORT, Forward, MAX_FORWARDS, is_engine_user,
    is_service_name, proxy_port,
};

/// Bumped when a field changes meaning; the launcher refuses any other version, so a launcher
/// left from an older install never half-reads a newer daemon's spec. 3: bound credentials
/// (#832), which an older launcher would leave unadded. 4: a login account's sign-in bound
/// in the session's home (#59). 5: host-service forwards (#880), which an older launcher
/// refuses. 6: the project engine's socket (#390), which an older launcher would leave out.
pub const SPEC_VERSION: u32 = 6;

/// Where the project engine's socket appears inside the sandbox, which `DOCKER_HOST` names
/// (#390): in a folder of its own, so the program reaches that socket and nothing beside it.
pub const ENGINE_SOCKET_INSIDE: &str = "/run/torlek-engine/docker.sock";

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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Subject {
    /// An agent session's provider.
    Session { session: String },
    /// A run of a project's check (#862): no provider, agent socket, launch token or session
    /// identity.
    Check { run: String, check: String },
    /// A project's containers (#390): the proxy of the project's engine asks for them. They
    /// never launch a sandbox of their own, so no launch spec names them.
    Containers { project: String },
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
    /// A login account's sign-in, bound read-write inside [`LaunchSpec::home`] (#59, #858):
    /// the account's login file at the harness's path, or the account's folder holding only
    /// it, which a link at that path names. None for a launch on no login account.
    pub login: Option<Folder>,
    /// Paths inside a [`LaunchSpec::read_write`] folder the program reads but never
    /// changes: Git hooks and configuration, the project's Torlek folder.
    pub protected: Vec<Protected>,
    /// Paths inside a bound folder the program never reads: Torlek's own files and
    /// credential files. A last name ending in `*` stands for every entry of its folder
    /// that starts with the rest. Absent ones stay absent.
    pub hidden: Vec<PathBuf>,
    pub needs_source: NeedsSource,
    /// The declared services the launcher serves inside the sandbox (#880); each relays only
    /// while the owner's decisions allow its record.
    pub forwards: Vec<Forward>,
    /// What the session proxy lets out of the sandbox (#831).
    pub egress: Allowlist,
    /// The hosts the session proxy intercepts to add a bound credential (#832); None for a
    /// launch with no bound credential, whose proxy intercepts nothing and has no CA.
    pub interception: Option<Interception>,
    /// The project engine whose socket the launcher binds at [`ENGINE_SOCKET_INSIDE`]
    /// (#390): None for a launch that reaches no engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<EngineSocket>,
}

/// The folder of [`ENGINE_SOCKET_INSIDE`], which holds that socket alone.
pub fn engine_folder_inside() -> PathBuf {
    Path::new(ENGINE_SOCKET_INSIDE)
        .parent()
        .expect("the engine's socket is in a folder")
        .to_path_buf()
}

/// A project engine's Docker socket, as the daemon verified the engine behind it (#390): the
/// launcher binds it only once the kernel says the engine user's uid listens there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineSocket {
    pub user: String,
    pub uid: u32,
    pub socket: PathBuf,
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
    /// Refuses a spec the launcher cannot enforce as written: a project's containers' (#390),
    /// another version, a relative
    /// path, a Landlock ABI below the floor, a protected or hidden path outside every bound
    /// folder, a working directory outside the writable ones, a forward the launcher cannot
    /// serve as written, an allowlist entry that is no host name or address and a port, or a
    /// credential bound to a host neither the allowlist nor the owner's `decisions` at launch
    /// (#674) let through.
    pub fn validate(&self, decisions: &EgressDecisions) -> Result<(), InvalidSpec> {
        if let Subject::Containers { .. } = self.subject {
            return Err(InvalidSpec(
                "a project's containers run in its engine, never in a sandbox launch".to_owned(),
            ));
        }
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
            .chain(&self.login)
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
        if let Some(login) = &self.login
            && (!login.destination.starts_with(&self.home) || login.destination == self.home)
        {
            return Err(InvalidSpec(format!(
                "the login is bound at {}, outside the home {}",
                login.destination.display(),
                self.home.display()
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
        if let Some(interception) = &self.interception {
            check_interception(interception, &self.egress.with_decisions(decisions))?;
        }
        if let Some(engine) = &self.engine {
            check_engine(engine, &self.subject)?;
        }
        self.check_forwards()
    }

    /// Every path the view binds, at its destination: what a forward's socket folder may
    /// neither lie inside nor contain, since the launcher binds that folder of its own there.
    pub fn bound_destinations(&self) -> Vec<PathBuf> {
        let fixed = ["/proc", "/dev", "/tmp"].iter().chain(SYSTEM_READ_ONLY);
        fixed
            .map(PathBuf::from)
            .chain([self.home.clone(), self.program.clone()])
            .chain(
                [
                    &self.read_only,
                    &self.read_only_declared,
                    &self.sockets,
                    &self.read_write,
                ]
                .into_iter()
                .flatten()
                .cloned(),
            )
            .chain(self.folders.iter().map(|folder| folder.destination.clone()))
            .chain(self.engine.iter().map(|_| engine_folder_inside()))
            .collect()
    }

    /// Only a session's launch forwards; each service once, by a lower-case name; each
    /// listen once, a port the sandbox can bind other than the proxy's or a socket whose
    /// folder the launcher can bind; each target a port or a plain path.
    fn check_forwards(&self) -> Result<(), InvalidSpec> {
        if self.forwards.is_empty() {
            return Ok(());
        }
        if !matches!(self.subject, Subject::Session { .. }) {
            return Err(InvalidSpec(
                "only a session's launch serves forwards".to_owned(),
            ));
        }
        if self.forwards.len() > MAX_FORWARDS {
            return Err(InvalidSpec(format!(
                "{} forwards; a launch serves at most {MAX_FORWARDS}",
                self.forwards.len()
            )));
        }
        let bound = self.bound_destinations();
        for (index, forward) in self.forwards.iter().enumerate() {
            let service = &forward.service;
            if !is_service_name(service) {
                return Err(InvalidSpec(format!(
                    "service name {service:?} is not a lower-case letter, then letters, digits \
                     and `-`"
                )));
            }
            if service == CONTAINERS_SERVICE {
                return Err(InvalidSpec(format!(
                    "service {service} is the project engine's ports, which only the owner's \
                     decisions open"
                )));
            }
            let earlier = &self.forwards[..index];
            if earlier.iter().any(|other| &other.service == service) {
                return Err(InvalidSpec(format!("service {service} is forwarded twice")));
            }
            if let Some(other) = earlier.iter().find(|other| other.listen == forward.listen) {
                return Err(InvalidSpec(format!(
                    "service {service} listens on {}, which service {} listens on already",
                    forward.listen, other.service
                )));
            }
            match &forward.listen {
                Endpoint::Port { port } if *port < FIRST_UNPRIVILEGED_PORT => {
                    return Err(InvalidSpec(format!(
                        "service {service} listens on port {port}; the sandbox binds ports from \
                         {FIRST_UNPRIVILEGED_PORT} up"
                    )));
                }
                Endpoint::Port { port } if *port == proxy_port() => {
                    return Err(InvalidSpec(format!(
                        "service {service} listens on {port}, the session proxy's port"
                    )));
                }
                Endpoint::Port { .. } => {}
                Endpoint::Socket { path } => {
                    check_socket_listen(service, path, &bound, earlier)?;
                }
                Endpoint::Engine { .. } => {
                    return Err(InvalidSpec(format!(
                        "service {service} listens on an engine's port, which is a target"
                    )));
                }
            }
            match &forward.target {
                Endpoint::Port { port: 0 } => {
                    return Err(InvalidSpec(format!("service {service} targets port 0")));
                }
                Endpoint::Port { .. } => {}
                Endpoint::Socket { path } => {
                    check_plain(path)?;
                    check_record_path(service, path)?;
                }
                // Opened live from the owner's decisions, never declared (#390).
                Endpoint::Engine { .. } => {
                    return Err(InvalidSpec(format!(
                        "service {service} targets an engine's port, which only the owner's \
                         decisions open"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// A socket listen is a plain path with a file name, in a folder other than `/` that holds
/// no folder the view binds, or another listen's, and lies in none: the launcher binds a
/// read-only folder of its own there, which would hide or be hidden by them.
fn check_socket_listen(
    service: &str,
    path: &Path,
    bound: &[PathBuf],
    earlier: &[Forward],
) -> Result<(), InvalidSpec> {
    check_plain(path)?;
    check_record_path(service, path)?;
    let folder = match path.parent() {
        Some(folder) if folder != Path::new("/") && path.file_name().is_some() => folder,
        _ => {
            return Err(InvalidSpec(format!(
                "service {service} listens on {}, which is in no folder of its own",
                path.display()
            )));
        }
    };
    if let Some(clash) = bound
        .iter()
        .find(|bound| folder.starts_with(bound) || bound.starts_with(folder))
    {
        return Err(InvalidSpec(format!(
            "service {service} listens in {}, which overlaps {}, a folder the sandbox binds",
            folder.display(),
            clash.display()
        )));
    }
    let nested = earlier.iter().find_map(|other| match &other.listen {
        Endpoint::Socket { path: other_path } => other_path
            .parent()
            .filter(|other_folder| {
                *other_folder != folder
                    && (folder.starts_with(other_folder) || other_folder.starts_with(folder))
            })
            .map(|other_folder| (other, other_folder)),
        Endpoint::Port { .. } | Endpoint::Engine { .. } => None,
    });
    if let Some((other, other_folder)) = nested {
        return Err(InvalidSpec(format!(
            "service {service} listens in {}, which overlaps {}, where service {} listens",
            folder.display(),
            other_folder.display(),
            other.service
        )));
    }
    Ok(())
}

/// A socket path in a forward's record is UTF-8 without spaces or control characters, so
/// two forwards never print one record, which one grant would cover.
fn check_record_path(service: &str, path: &Path) -> Result<(), InvalidSpec> {
    let is_printable = path.to_str().is_some_and(|text| {
        !text
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    });
    if is_printable {
        return Ok(());
    }
    Err(InvalidSpec(format!(
        "service {service} names {path:?}, which is not UTF-8 without spaces or control \
         characters"
    )))
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

/// At least one credential; each host a lower-case DNS name bound once, which `allowlist` (the
/// spec's with the launch's decisions) lets the session reach on 443 (#857 decision 3: a
/// credential is never bound to a host without reach); a Git username Basic credentials can
/// carry; plain paths.
fn check_interception(
    interception: &Interception,
    allowlist: &Allowlist,
) -> Result<(), InvalidSpec> {
    if interception.credentials.is_empty() {
        return Err(InvalidSpec(
            "the interception binds no credential".to_owned(),
        ));
    }
    for path in [
        &interception.system_bundle,
        &interception.ca_file,
        &interception.bundle_file,
    ] {
        check_plain(path)?;
    }
    let mut bound: Vec<&str> = Vec::new();
    for credential in &interception.credentials {
        let is_printable = !credential.name.is_empty()
            && credential
                .name
                .chars()
                .all(|character| matches!(character, ' '..='~'));
        if !is_printable {
            return Err(InvalidSpec(format!(
                "credential name {:?} is not printable ASCII",
                credential.name
            )));
        }
        if credential.hosts.is_empty() {
            return Err(InvalidSpec(format!(
                "{} is bound to no host",
                credential.name
            )));
        }
        for bound_host in &credential.hosts {
            let host = bound_host.host.as_str();
            if normalized_name(host).as_deref() != Some(host) || !host.contains('.') {
                return Err(InvalidSpec(format!(
                    "{} is bound to {host:?}, which is not a lower-case host name",
                    credential.name
                )));
            }
            if bound.contains(&host) {
                return Err(InvalidSpec(format!(
                    "{host} carries more than one credential"
                )));
            }
            bound.push(host);
            if let Err(refusal) = allowlist.decide(host, TLS_PORT) {
                return Err(InvalidSpec(format!(
                    "{} cannot be bound to {host}: the session may not reach it ({refusal}); \
                     a credential is bound only to a host the session reaches (#857)",
                    credential.name
                )));
            }
            if let CredentialScheme::Header { name } = &bound_host.scheme
                && !is_credential_header(name)
            {
                return Err(InvalidSpec(format!(
                    "the credential header {name:?} for {host} is not a lower-case `x-` header"
                )));
            }
            if let CredentialScheme::GitReads { username } = &bound_host.scheme {
                let is_valid = !username.is_empty()
                    && username
                        .chars()
                        .all(|character| character.is_ascii_graphic() && character != ':');
                if !is_valid {
                    return Err(InvalidSpec(format!(
                        "the Git username {username:?} for {host} is empty or holds a `:`"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// `x-` then lower-case ASCII letters, digits and `-`: a header the proxy may set to a
/// credential, never one that frames or routes the request. The adapters' declarations are
/// held to the same rule (`cockpit_provider_api::is_credential_header`); the spec checks it
/// again since the launcher trusts nothing the daemon did not write.
fn is_credential_header(name: &str) -> bool {
    name.strip_prefix("x-").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    })
}

/// Refuses an engine a session cannot be given as written: one for a check run, run by root
/// or by no engine user, or whose socket is no plain absolute path.
fn check_engine(engine: &EngineSocket, subject: &Subject) -> Result<(), InvalidSpec> {
    if !matches!(subject, Subject::Session { .. }) {
        return Err(InvalidSpec(
            "a check run reaches no project engine".to_owned(),
        ));
    }
    if engine.uid == 0 {
        return Err(InvalidSpec(format!(
            "the project engine of {} runs as root",
            engine.user
        )));
    }
    if !is_engine_user(&engine.user) {
        return Err(InvalidSpec(format!(
            "{:?} is not an engine user",
            engine.user
        )));
    }
    check_plain(&engine.socket)
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
