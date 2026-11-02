//! The profile Cockpit requests for one launch: what the agent may read and write, which
//! hosts it may reach beyond its provider's own, its environment and secrets, and whether
//! it gets the owner's desktop session. Built once per launch; a rule the platform cannot
//! enforce refuses the launch with the rule and the reason.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use cockpit_config::DesktopSession;

use crate::Platform;

/// Read denials every profile carries, whatever the project: credential stores and the
/// proxy's own keys (from the launcher the current cockpit runs, `greywall.json`). `~` is
/// the agent's home; the engine expands it.
pub const BASELINE_DENIED_READ: &[&str] = &[
    "~/.ssh/id_*",
    "~/.gnupg/**",
    "~/.aws/**",
    "~/.kube/**",
    "~/.docker/config.json",
    "~/.netrc",
    "~/.git-credentials",
    "~/.password-store/**",
    "~/.local/share/keyrings/**",
    "~/.local/share/greyproxy/**",
    ".env",
    ".env.*",
];

/// Write denials every profile carries: what would run with the owner's rights outside the
/// sandbox (Git hooks and config, the project's Cockpit file whose checks the daemon runs on
/// the host, direnv and IDE run configurations, shell and service start-up, installed
/// programs).
pub const BASELINE_DENIED_WRITE: &[&str] = &[
    ".git/hooks",
    ".git/config",
    ".cockpit",
    ".envrc",
    ".vscode/tasks.json",
    ".vscode/launch.json",
    ".idea/runConfigurations",
    "~/.gitconfig",
    "~/.ssh",
    "~/.gnupg",
    "~/.local/bin",
    "~/.config/systemd",
    "~/.config/autostart",
    "~/.config/greywall",
    "~/.config/fish",
    "~/.bashrc",
    "~/.profile",
    "~/.zshrc",
];

/// The owner's desktop session variables (#303, #305): a profile that withholds the desktop
/// session refuses an environment holding any of them. The same four names as
/// `cockpit_protocol::DESKTOP_SESSION_VARIABLES`, which this crate does not depend on.
pub const DESKTOP_VARIABLES: [&str; 4] = [
    "DISPLAY",
    "XAUTHORITY",
    "WAYLAND_DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
];

/// WSL2's variable naming the interop socket that starts Windows programs.
const WSL_INTEROP_VARIABLE: &str = "WSL_INTEROP";

/// The folder Codex 0.161.0 keeps its app-server sockets in, whatever `TMPDIR` says:
/// `<this><uid>`. Inside the sandbox it is a folder of the session's own (#446).
const CODEX_SOCKET_FOLDER_PREFIX: &str = "/tmp/codex-daemon-";

/// The session's folder, under its directory, mounted at Codex's socket folder.
pub const CODEX_SOCKET_FOLDER_NAME: &str = "codex-daemon";

/// Where X11 clients find display `:N`'s socket.
const X11_SOCKET_DIR: &str = "/tmp/.X11-unix";

/// How one environment variable that holds a secret crosses into the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretHandling {
    /// The agent sees a placeholder; greyproxy swaps the real value into outgoing requests.
    Placeholder,
    /// The agent sees the real value: the engine's credential detection leaves it alone.
    /// For values used outside HTTP headers, such as the launch token on the agent socket.
    PassThrough,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Secret {
    pub variable: String,
    pub handling: SecretHandling,
}

/// What one launch asks for, before Cockpit's baseline and the platform's rules.
#[derive(Debug, Clone)]
pub struct ProfileRequest {
    /// The provider whose agent runs; its own data folders and hosts come from the engine's
    /// reviewed profile of that name.
    pub provider: String,
    /// The checkout folder the agent works in: read and write.
    pub working_dir: PathBuf,
    /// The session's private directory (`sessions/<id>` in the daemon's socket folder), where
    /// its provider keeps what reaches the running agent, such as Codex's app-server socket:
    /// read and write.
    pub session_dir: PathBuf,
    /// The session's folder in the daemon's mount sources: where the folders the sandbox
    /// shows at another path live. It and its parent must be outside every folder the agent
    /// writes (#446).
    pub mount_dir: PathBuf,
    /// The user the daemon and its agents run as; names the session bus a shared desktop
    /// session reaches.
    pub owner_uid: u32,
    /// Folders the agent must reach read-only: the agent socket's folder, the bridge
    /// programs.
    pub read_only: Vec<PathBuf>,
    /// Cockpit's own files no agent may read, even inside a folder it may.
    pub denied: Vec<PathBuf>,
    /// Host-owned paths whose write denial overrides any broader writable grant.
    pub denied_write: Vec<PathBuf>,
    /// Hosts (`host` or `host:port`) the agent may reach beyond its provider's own.
    pub allow_hosts: Vec<String>,
    pub environment: Vec<(OsString, OsString)>,
    pub secrets: Vec<Secret>,
    pub desktop: DesktopSession,
}

/// The effective profile: the request, Cockpit's baseline and the platform's own denials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub platform: Platform,
    pub provider: String,
    pub working_dir: PathBuf,
    pub session_dir: PathBuf,
    pub owner_uid: u32,
    pub read_only: Vec<PathBuf>,
    pub denied: Vec<PathBuf>,
    pub denied_read_patterns: Vec<String>,
    pub denied_write_patterns: Vec<String>,
    pub allow_hosts: Vec<String>,
    pub environment: Vec<(OsString, OsString)>,
    pub secrets: Vec<Secret>,
    pub desktop: DesktopSession,
    /// What the shared desktop session's variables name, each one reachable on its own;
    /// empty when the session is withheld.
    pub desktop_endpoints: Vec<DesktopEndpoint>,
    /// Folders of the session's own the provider needs at a fixed path inside the sandbox:
    /// Codex's socket folder. Never the host's folder at that path, which the owner's own
    /// Codex uses (#446).
    pub folder_mounts: Vec<FolderMount>,
}

/// A host folder the sandbox shows at another path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderMount {
    /// On the host: in the daemon's own folder for the session, outside everything the
    /// agent writes; created owner-only before the launch.
    pub host: PathBuf,
    /// Where the agent finds it.
    pub sandbox: PathBuf,
}

impl FolderMount {
    /// The host's name for `inside`, an entry directly in the mounted folder as the agent
    /// sees it; None for any other path.
    pub fn host_path(&self, inside: &Path) -> Option<PathBuf> {
        let mut rest = inside.strip_prefix(&self.sandbox).ok()?.components();
        let (Some(std::path::Component::Normal(name)), None) = (rest.next(), rest.next()) else {
            return None;
        };
        Some(self.host.join(name))
    }
}

/// One socket or file a desktop variable names, which a shared desktop session reaches
/// inside the sandbox (#435). Nothing else of the runtime directory or `/tmp` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEndpoint {
    pub variable: &'static str,
    pub path: PathBuf,
    pub kind: EndpointKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointKind {
    /// A display server's socket (`DISPLAY`, `WAYLAND_DISPLAY`): connecting writes to it.
    Socket,
    /// A file read only (`XAUTHORITY`).
    File,
    /// The session bus (`DBUS_SESSION_BUS_ADDRESS`): never the owner's own bus, which
    /// starts programs outside the sandbox, but the engine's filtered proxy at this path.
    Bus,
}

/// Why a profile cannot be enforced: the rule, and what the platform or request lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub rule: String,
    pub reason: String,
}

impl Refusal {
    pub(crate) fn new(rule: impl Into<String>, reason: impl Into<String>) -> Refusal {
        Refusal {
            rule: rule.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the sandbox refuses `{}`: {}",
            self.rule, self.reason
        )
    }
}

impl std::error::Error for Refusal {}

impl Profile {
    /// The effective profile for `request` on `platform`, or the first rule that cannot be
    /// enforced there.
    pub fn build(request: ProfileRequest, platform: &Platform) -> Result<Profile, Refusal> {
        if let Platform::Unsupported(name) = platform {
            return Err(Refusal::new(
                "sandbox",
                format!("no sandbox backend enforces a profile on {name}"),
            ));
        }
        check_provider(&request.provider)?;
        let mut denied = request.denied;
        let mut environment = request.environment;
        if let Platform::Wsl2 {
            windows_drives,
            interop_sockets,
        } = platform
        {
            for drive in windows_drives {
                refuse_inside(&request.working_dir, drive, "read-write")?;
                for path in &request.read_only {
                    refuse_inside(path, drive, "read-only")?;
                }
            }
            denied.extend(windows_drives.iter().cloned());
            denied.push(interop_sockets.clone());
            environment.retain(|(name, _)| name != WSL_INTEROP_VARIABLE);
        }
        check_absolute(&request.working_dir, "read-write")?;
        check_absolute(&request.session_dir, "read-write")?;
        for path in request.read_only.iter() {
            check_absolute(path, "read-only")?;
        }
        for path in &request.denied_write {
            check_absolute(path, "denied-write")?;
        }
        check_absolute(&request.mount_dir, "folder")?;
        let folder_mounts = folder_mounts(&request.provider, &request.mount_dir, request.owner_uid);
        let desktop_endpoints = match request.desktop {
            DesktopSession::Shared => desktop_endpoints(&environment)?,
            DesktopSession::Withheld => Vec::new(),
        };
        for path in &denied {
            check_absolute(path, "denied")?;
            refuse_masked(&request.working_dir, path, "read-write")?;
            refuse_masked(&request.session_dir, path, "read-write")?;
            for allowed in &request.read_only {
                refuse_masked(allowed, path, "read-only")?;
            }
            for endpoint in &desktop_endpoints {
                refuse_masked(&endpoint.path, path, "desktop")?;
            }
        }
        for host in &request.allow_hosts {
            check_host(host)?;
        }
        let writable: Vec<&Path> = [request.working_dir.as_path(), &request.session_dir]
            .into_iter()
            .chain(
                desktop_endpoints
                    .iter()
                    .filter(|endpoint| endpoint.kind == EndpointKind::Socket)
                    .map(|endpoint| endpoint.path.as_path()),
            )
            .collect();
        for mount in &folder_mounts {
            refuse_writable_source(mount, &writable)?;
        }
        for secret in &request.secrets {
            check_variable(&secret.variable)?;
        }
        if request.desktop == DesktopSession::Withheld
            && let Some((name, _)) = environment
                .iter()
                .find(|(name, _)| DESKTOP_VARIABLES.iter().any(|variable| name == variable))
        {
            return Err(Refusal::new(
                "desktop withheld",
                format!(
                    "the environment holds {} although the owner withholds the desktop session",
                    name.to_string_lossy()
                ),
            ));
        }
        Ok(Profile {
            platform: platform.clone(),
            provider: request.provider,
            working_dir: request.working_dir,
            session_dir: request.session_dir,
            owner_uid: request.owner_uid,
            read_only: request.read_only,
            denied,
            denied_read_patterns: BASELINE_DENIED_READ
                .iter()
                .map(|rule| rule.to_string())
                .collect(),
            denied_write_patterns: BASELINE_DENIED_WRITE
                .iter()
                .map(|rule| rule.to_string())
                .chain(
                    request
                        .denied_write
                        .iter()
                        .map(|path| path.display().to_string()),
                )
                .collect(),
            allow_hosts: request.allow_hosts,
            environment,
            secrets: request.secrets,
            desktop: request.desktop,
            desktop_endpoints,
            folder_mounts,
        })
    }

    /// The profile's rules, one line each in a fixed order: what a dry run prints and the
    /// evidence lists.
    pub fn rules(&self) -> Vec<String> {
        let mut rules = vec![
            format!("platform {}", self.platform),
            format!("provider {}", self.provider),
            format!("read-write {}", self.working_dir.display()),
            format!(
                "read-write {} (the session's directory)",
                self.session_dir.display()
            ),
        ];
        rules.extend(self.folder_mounts.iter().map(|mount| {
            format!(
                "folder {} at {} ({}'s own, not the host's)",
                mount.host.display(),
                mount.sandbox.display(),
                self.provider
            )
        }));
        rules.extend(
            self.read_only
                .iter()
                .map(|path| format!("read-only {}", path.display())),
        );
        rules.push("read everything else: denied".to_owned());
        rules.extend(
            self.denied
                .iter()
                .map(|path| format!("deny {}", path.display())),
        );
        rules.extend(
            self.denied_read_patterns
                .iter()
                .map(|pattern| format!("deny read {pattern}")),
        );
        rules.extend(
            self.denied_write_patterns
                .iter()
                .map(|pattern| format!("deny write {pattern}")),
        );
        rules.push("network: through greyproxy only".to_owned());
        rules.extend(
            self.allow_hosts
                .iter()
                .map(|host| format!("allow host {host}")),
        );
        rules.extend(self.secrets.iter().map(|secret| match secret.handling {
            SecretHandling::Placeholder => format!("secret {} as a placeholder", secret.variable),
            SecretHandling::PassThrough => format!("secret {} passed through", secret.variable),
        }));
        rules.push(format!("desktop session {}", self.desktop.as_str()));
        rules.extend(self.desktop_endpoints.iter().map(|endpoint| {
            let kind = match endpoint.kind {
                EndpointKind::Socket => "socket",
                EndpointKind::File => "file",
                EndpointKind::Bus => "bus",
            };
            format!(
                "desktop {kind} {} for {}",
                endpoint.path.display(),
                endpoint.variable
            )
        }));
        rules
    }
}

/// What `provider` needs at a fixed path of the sandbox, from the session's mount folder.
fn folder_mounts(provider: &str, mount_dir: &Path, owner_uid: u32) -> Vec<FolderMount> {
    match provider {
        "codex" => vec![FolderMount {
            host: mount_dir.join(CODEX_SOCKET_FOLDER_NAME),
            sandbox: PathBuf::from(format!("{CODEX_SOCKET_FOLDER_PREFIX}{owner_uid}")),
        }],
        _ => Vec::new(),
    }
}

/// Greywall refuses unsafe mount sources but checks before it mounts: a source, or the
/// folder holding it, that the agent writes could be swapped in between (#446).
fn refuse_writable_source(mount: &FolderMount, writable: &[&Path]) -> Result<(), Refusal> {
    let holder = mount.host.parent().unwrap_or(&mount.host);
    let Some(granted) = writable
        .iter()
        .find(|granted| holder.starts_with(granted) || granted.starts_with(holder))
    else {
        return Ok(());
    };
    Err(Refusal::new(
        format!(
            "folder {} at {}",
            mount.host.display(),
            mount.sandbox.display()
        ),
        format!(
            "the agent writes {}, so it could swap the mounted folder before the mount",
            granted.display()
        ),
    ))
}

/// The last value `environment` gives `name`, as a later entry overrides an earlier one.
fn variable<'environment>(
    environment: &'environment [(OsString, OsString)],
    name: &str,
) -> Option<&'environment OsString> {
    environment
        .iter()
        .rev()
        .find(|(variable, _)| variable == name)
        .map(|(_, value)| value)
}

/// The sockets and files the shared desktop variables in `environment` name, or the first
/// one the sandbox cannot reach as named.
fn desktop_endpoints(
    environment: &[(OsString, OsString)],
) -> Result<Vec<DesktopEndpoint>, Refusal> {
    let mut endpoints = Vec::new();
    let text = |name: &str| {
        variable(environment, name)
            .map(|value| {
                value.to_str().ok_or_else(|| {
                    Refusal::new(format!("desktop {name}"), "the value is not UTF-8")
                })
            })
            .transpose()
    };
    let refused = |name: &str, value: &str, reason: &str| {
        Refusal::new(format!("desktop {name}={value}"), reason)
    };
    if let Some(display) = text("DISPLAY")? {
        let path = x11_socket(display).map_err(|reason| refused("DISPLAY", display, reason))?;
        endpoints.push(endpoint("DISPLAY", path, EndpointKind::Socket)?);
    }
    if let Some(wayland) = text("WAYLAND_DISPLAY")? {
        let path = if Path::new(wayland).is_absolute() {
            PathBuf::from(wayland)
        } else {
            let runtime_dir = text("XDG_RUNTIME_DIR")?
                .filter(|dir| Path::new(dir).is_absolute())
                .ok_or_else(|| {
                    refused(
                        "WAYLAND_DISPLAY",
                        wayland,
                        "it names a socket in XDG_RUNTIME_DIR, which the environment does not \
                         give as an absolute path",
                    )
                })?;
            Path::new(runtime_dir).join(wayland)
        };
        endpoints.push(endpoint("WAYLAND_DISPLAY", path, EndpointKind::Socket)?);
    }
    if let Some(xauthority) = text("XAUTHORITY")? {
        endpoints.push(endpoint(
            "XAUTHORITY",
            PathBuf::from(xauthority),
            EndpointKind::File,
        )?);
    }
    if let Some(bus) = text("DBUS_SESSION_BUS_ADDRESS")? {
        let path =
            bus_socket(bus).map_err(|reason| refused("DBUS_SESSION_BUS_ADDRESS", bus, reason))?;
        endpoints.push(endpoint(
            "DBUS_SESSION_BUS_ADDRESS",
            path,
            EndpointKind::Bus,
        )?);
    }
    Ok(endpoints)
}

/// An endpoint at an absolute path with no `..`, which could leave the folder it names.
fn endpoint(
    variable: &'static str,
    path: PathBuf,
    kind: EndpointKind,
) -> Result<DesktopEndpoint, Refusal> {
    let is_plain = path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, std::path::Component::ParentDir));
    if !is_plain {
        return Err(Refusal::new(
            format!("desktop {variable}={}", path.display()),
            "the sandbox reaches a desktop endpoint only at an absolute path without `..`",
        ));
    }
    Ok(DesktopEndpoint {
        variable,
        path,
        kind,
    })
}

/// `DISPLAY`'s socket: `:N`, `:N.S` and `unix:N` are `/tmp/.X11-unix/XN`; an absolute path
/// (XQuartz's launchd socket) is the socket itself.
fn x11_socket(display: &str) -> Result<PathBuf, &'static str> {
    if display.starts_with('/') {
        return Ok(PathBuf::from(display));
    }
    let (host, screen) = display
        .rsplit_once(':')
        .ok_or("a display is `[host]:number[.screen]`")?;
    if !(host.is_empty() || host == "unix") {
        return Err("a display on another host is reached over the network, not a local socket");
    }
    let number = screen.split_once('.').map_or(screen, |(number, _)| number);
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("a display is `[host]:number[.screen]`");
    }
    Ok(Path::new(X11_SOCKET_DIR).join(format!("X{number}")))
}

/// The session bus's socket: `unix:path=<socket>[,<key>=<value>...]`. An abstract socket,
/// TCP or a launched program are not a path the sandbox can reach.
fn bus_socket(address: &str) -> Result<PathBuf, &'static str> {
    let first = address.split(';').next().unwrap_or_default();
    first
        .strip_prefix("unix:")
        .and_then(|keys| keys.split(',').find_map(|pair| pair.strip_prefix("path=")))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or("only a `unix:path=` bus is reached inside the sandbox")
}

fn check_provider(provider: &str) -> Result<(), Refusal> {
    let is_name = !provider.is_empty()
        && provider
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if is_name {
        Ok(())
    } else {
        Err(Refusal::new(
            format!("provider {provider}"),
            "a provider name is lowercase letters, digits and dashes",
        ))
    }
}

fn check_absolute(path: &Path, rule: &str) -> Result<(), Refusal> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(Refusal::new(
            format!("{rule} {}", path.display()),
            "the path is not absolute",
        ))
    }
}

/// The engine applies denials last, so an allowed folder inside a denied one would be
/// unreadable after all: refused rather than handed over broken.
fn refuse_masked(allowed: &Path, denied: &Path, rule: &str) -> Result<(), Refusal> {
    if allowed.starts_with(denied) {
        return Err(Refusal::new(
            format!("{rule} {}", allowed.display()),
            format!(
                "it is inside {}, which the profile denies",
                denied.display()
            ),
        ));
    }
    Ok(())
}

/// A Windows drive under WSL2: a Windows program found there runs outside any Linux sandbox
/// through interop (report 11).
fn refuse_inside(path: &Path, drive: &Path, rule: &str) -> Result<(), Refusal> {
    if path.starts_with(drive) {
        return Err(Refusal::new(
            format!("{rule} {}", path.display()),
            format!(
                "{} is a Windows drive, where WSL interop runs Windows programs outside the \
                 sandbox; keep the project on the Linux filesystem",
                drive.display()
            ),
        ));
    }
    Ok(())
}

fn check_host(host: &str) -> Result<(), Refusal> {
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (host, None),
    };
    // A wildcard only as a whole first label (`*.example.com`): a bare `*` would open the
    // network to every host.
    let labels = name.strip_prefix("*.").unwrap_or(name);
    let is_name = !labels.is_empty()
        && name.len() <= 253
        && labels
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    let is_port = port.is_none_or(|port| port.parse::<u16>().is_ok_and(|port| port != 0));
    if is_name && is_port {
        Ok(())
    } else {
        Err(Refusal::new(
            format!("allow host {host}"),
            "a host is a DNS name, a wildcard only as its whole first label, with an optional port",
        ))
    }
}

fn check_variable(variable: &str) -> Result<(), Refusal> {
    let is_name = !variable.is_empty()
        && !variable.starts_with(|first: char| first.is_ascii_digit())
        && variable
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if is_name {
        Ok(())
    } else {
        Err(Refusal::new(
            format!("secret {variable}"),
            "a secret is an environment variable name",
        ))
    }
}
