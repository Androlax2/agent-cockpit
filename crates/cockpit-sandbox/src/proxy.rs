//! Profile-owned Greyproxy configuration, login service and current health (#495).
mod command;
mod ownership;
mod service;

use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, TcpListener, UdpSocket};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use cockpit_config::{Environment, Layout, Profile};
use service::ServiceManager;

use crate::Platform;

const FILE_LIMIT: u64 = 32 * 1024;
const COMMAND_DEADLINE: Duration = Duration::from_secs(5);
const STARTUP_DEADLINE: Duration = Duration::from_secs(12);

/// Typed failures retained in readiness and CLI diagnostics; no failure disables isolation.
#[derive(Debug)]
pub enum ProxyError {
    NotConfigured,
    ServiceStopped,
    EngineRestartRequired,
    EngineCapability(String),
    ServiceManager(String),
    EndpointOccupied(String),
    Unhealthy(String),
    ConfigurationChanged(PathBuf),
    UnsafePath(PathBuf),
    Unsupported(String),
    Io(std::io::Error),
}

impl ProxyError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotConfigured => "proxy-not-configured",
            Self::ServiceStopped => "proxy-service-stopped",
            Self::EngineRestartRequired => "proxy-engine-restart-required",
            Self::EngineCapability(_) => "proxy-engine-capability",
            Self::ServiceManager(_) => "proxy-service-manager",
            Self::EndpointOccupied(_) => "proxy-endpoint-occupied",
            Self::Unhealthy(_) => "proxy-unhealthy",
            Self::ConfigurationChanged(_) => "proxy-configuration-changed",
            Self::UnsafePath(_) => "proxy-unsafe-path",
            Self::Unsupported(_) => "proxy-unsupported",
            Self::Io(_) => "proxy-io",
        }
    }

    pub fn next(&self, profile: &Profile) -> String {
        let command = format!("cockpit --profile {profile} sandbox");
        match self {
            Self::NotConfigured => format!("run `{command} install`"),
            Self::EndpointOccupied(endpoint) => format!(
                "have the owner resolve the listener at {endpoint}, then run `{command} install`; Cockpit will not stop or adopt it"
            ),
            Self::ConfigurationChanged(path) | Self::UnsafePath(path) => format!(
                "inspect {} without changing another installation, then retry `{command} install`",
                path.display()
            ),
            Self::ServiceManager(_) => format!(
                "make this user's systemd service manager (also on WSL2) or macOS login launchd domain available, then run `{command} install`"
            ),
            Self::Unsupported(_) => "use Linux, macOS or a WSL2 distribution".into(),
            Self::EngineCapability(_) => "a verified Greywall release with macOS public-CA access and private-state isolation is required; keep sandboxing enabled and update Cockpit when that pin is published".into(),
            _ => format!("run `{command} restart`, then `cockpit --profile {profile} doctor`"),
        }
    }
}

impl fmt::Display for ProxyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::NotConfigured => f.write_str("this profile has no managed Greyproxy setup"),
            Self::ServiceStopped => f.write_str("the owned Greyproxy service is not running"),
            Self::EngineRestartRequired => f.write_str("this profile's service belongs to an earlier engine release; an explicit restart is required"),
            Self::ServiceManager(reason) | Self::Unhealthy(reason) | Self::Unsupported(reason) | Self::EngineCapability(reason) => {
                f.write_str(reason)
            }
            Self::EndpointOccupied(endpoint) => {
                write!(f, "{endpoint} is occupied; its process was left untouched")
            }
            Self::ConfigurationChanged(path) => write!(
                f,
                "{} does not match the owned configuration",
                path.display()
            ),
            Self::UnsafePath(path) => write!(
                f,
                "{} is not an owned, regular private path",
                path.display()
            ),
            Self::Io(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ProxyError {}
impl From<std::io::Error> for ProxyError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// The fixed endpoints are an upstream constraint: pinned Greywall's management API is
/// localhost:43080. Different profiles cannot own the ports concurrently.
#[derive(Debug, Clone)]
pub struct ManagedProxy {
    home: PathBuf,
    profile: Profile,
    data_dir: PathBuf,
    program: PathBuf,
    unit_path: PathBuf,
    manager: ServiceManager,
    uid: u32,
    // Private injection points let unit fixtures avoid all live endpoints and managers.
    ports: [u16; 4],
    service_program: PathBuf,
    curl: PathBuf,
    command_deadline: Duration,
    proc_root: PathBuf,
    lsof: PathBuf,
    metadata_commands: PathBuf,
}

impl ManagedProxy {
    pub fn new(environment: &Environment, platform: &Platform) -> Result<Self, ProxyError> {
        let home = environment
            .home_dir()
            .map_err(|error| ProxyError::Unsupported(error.to_string()))?;
        let uid = fs::metadata(&home)?.uid();
        let layout = Layout::resolve(environment, &home);
        let manager = match platform {
            Platform::Linux | Platform::Wsl2 { .. } => ServiceManager::Systemd,
            Platform::MacOs => ServiceManager::Launchd,
            Platform::Unsupported(name) => return Err(ProxyError::Unsupported(name.clone())),
        };
        let unit_dir = match manager {
            ServiceManager::Systemd => environment
                .xdg_config_home
                .as_deref()
                .map(Path::new)
                .filter(|path| path.is_absolute())
                .map(Path::to_owned)
                .unwrap_or_else(|| home.join(".config"))
                .join("systemd/user"),
            ServiceManager::Launchd => home.join("Library/LaunchAgents"),
        };
        let profile = environment.profile.value.clone();
        let unit_path = unit_dir.join(manager.unit_name(&profile));
        Ok(Self {
            program: crate::install::programs(&layout.state_dir.value)
                .map_err(|error| ProxyError::Unsupported(error.to_string()))?
                .greyproxy,
            data_dir: layout.state_dir.value.join("greyproxy"),
            home,
            profile,
            unit_path,
            manager,
            uid,
            ports: [43080, 43051, 43052, 43053],
            service_program: manager.program().into(),
            curl: "curl".into(),
            command_deadline: COMMAND_DEADLINE,
            proc_root: "/proc".into(),
            lsof: "lsof".into(),
            metadata_commands: PathBuf::new(),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
    pub fn unit_path(&self) -> &Path {
        &self.unit_path
    }

    /// Select an explicit directory of host commands, e.g. isolated service/HTTP fixtures
    /// for an in-process daemon. This changes command resolution, never ownership checks.
    pub fn with_command_directory(mut self, directory: &Path) -> Self {
        self.service_program = directory.join(self.manager.program());
        self.curl = directory.join("curl");
        self.lsof = directory.join("lsof");
        self.metadata_commands = directory.to_owned();
        self
    }

    /// Inspect the exact managed files without reading state or starting a service.
    /// Callers writing these files must preserve the private-path checks in `install`.
    pub fn configuration_files(&self) -> Result<Vec<(PathBuf, String)>, ProxyError> {
        let config = self.configuration();
        let unit = self.unit_text()?;
        let receipt = serde_json::json!({
            "profile": self.profile.name(), "configuration": config, "unit": unit, "program": self.program,
        })
        .to_string();
        Ok(vec![
            (self.configuration_path(), config),
            (self.unit_path.clone(), unit),
            (self.receipt_path(), receipt),
        ])
    }

    pub(crate) fn refusal(&self, error: ProxyError) -> crate::Refusal {
        crate::Refusal::new(
            error.code(),
            format!("{error}; next: {}", error.next(&self.profile)),
        )
    }

    fn configuration_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }
    fn receipt_path(&self) -> PathBuf {
        self.data_dir.join("owned.json")
    }

    /// Install/start only this profile's service, preserving an already healthy process.
    /// Engine downloads and verification are performed by the caller through `install`.
    pub fn install(&self) -> Result<(), ProxyError> {
        let _lock = self.setup_lock()?;
        if self.receipt_path().exists() {
            let owned = self.owned()?;
            if owned.program != self.program {
                return Err(ProxyError::EngineRestartRequired);
            }
            if self.service_pid_for_action(true)?.is_some() {
                return self.health().map(|_| ());
            }
        } else if self.unit_path.symlink_metadata().is_ok() {
            return Err(ProxyError::ConfigurationChanged(self.unit_path.clone()));
        } else if fs::read_dir(&self.data_dir)?
            .any(|entry| entry.map_or(true, |entry| entry.file_name() != "setup.lock"))
        {
            return Err(ProxyError::ConfigurationChanged(self.data_dir.clone()));
        }
        self.verify_program()?;
        let ports = self.check_ports()?;
        self.write_configuration()?;
        drop(ports);
        self.start_service()?;
        self.wait_healthy()
    }

    /// An explicit owner restart may interrupt this profile's agents' network connections.
    pub fn restart(&self) -> Result<(), ProxyError> {
        let _lock = self.setup_lock()?;
        let owned = self.owned()?;
        self.verify_program()?;
        owned.stop_service()?;
        let ports = self.check_ports()?;
        self.write_configuration()?;
        drop(ports);
        self.start_service()?;
        self.wait_healthy()
    }

    pub fn stop(&self) -> Result<(), ProxyError> {
        let owned = self.owned()?;
        let _lock = self.setup_lock()?;
        owned.stop_service()
    }

    /// Fresh observation, never cached as proof that a future launch can use the proxy.
    pub fn health(&self) -> Result<String, ProxyError> {
        if self.owned()?.program != self.program {
            return Err(ProxyError::EngineRestartRequired);
        }
        let pid = self.service_pid()?.ok_or(ProxyError::ServiceStopped)?;
        self.verify_endpoint_owner(pid)?;
        self.ca_pair()?;
        let token = read_file(&self.data_dir.join("api-token"), self.uid, true)?;
        let token = token.trim();
        if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ProxyError::Unhealthy(
                "the owned API token is invalid".into(),
            ));
        }
        let (code, body) = self.request("/api/health", None)?;
        let health: serde_json::Value = serde_json::from_str(&body)
            .map_err(|_| ProxyError::Unhealthy("invalid health response".into()))?;
        if code != 200
            || health["service"] != "greyproxy"
            || health["status"] != "ok"
            || health["ports"] != self.port_map()
        {
            return Err(ProxyError::Unhealthy(
                "health does not describe the configured proxy and all its listeners".into(),
            ));
        }
        // Authentication is checked only after the owned service and its health. An open
        // API or a foreign endpoint is never treated as the configured management service.
        if self.request("/api/settings", None)?.0 != 401 {
            return Err(ProxyError::Unhealthy(
                "the management API does not require authentication".into(),
            ));
        }
        self.refuse_ipv6_management_listener()?;
        let (code, body) = self.request("/api/settings", Some(token))?;
        if code != 200
            || !serde_json::from_str::<serde_json::Value>(&body)
                .is_ok_and(|value| value.is_object())
        {
            return Err(ProxyError::Unhealthy(
                "the management API rejected this profile's private token".into(),
            ));
        }
        Ok(format!(
            "owned Greyproxy service healthy at 127.0.0.1:{}",
            self.ports[0]
        ))
    }

    /// Service health and launch capability are separate. The pinned macOS backend
    /// cannot re-expose its public CA through the required private-state denial.
    pub fn launch_health(&self, engine: Option<&crate::Greywall>) -> Result<String, ProxyError> {
        let health = self.health()?;
        if matches!(self.manager, ServiceManager::Launchd)
            && !engine.is_some_and(|engine| {
                engine.capabilities.iter().any(|capability| {
                    capability.name == "public CA isolation" && capability.available
                })
            })
        {
            return Err(ProxyError::EngineCapability(format!(
                "Greywall {} cannot expose only the public CA inside the protected managed data directory on macOS",
                crate::PINNED_GREYWALL_COMMIT
            )));
        }
        Ok(health)
    }

    fn verify_program(&self) -> Result<(), ProxyError> {
        let result = command::run(
            Command::new(&self.program).arg("--version"),
            None,
            self.command_deadline,
        )
        .map_err(ProxyError::Unhealthy)?;
        if !result.status.success() {
            return Err(ProxyError::Unhealthy(
                "the pinned Greyproxy version command failed".into(),
            ));
        }
        let output = String::from_utf8(result.stdout)
            .map_err(|_| ProxyError::Unhealthy("invalid version output".into()))?;
        let build = crate::parse_greyproxy_version(&output).map_err(ProxyError::Unhealthy)?;
        if build.commit != crate::PINNED_GREYPROXY_COMMIT {
            return Err(ProxyError::Unhealthy(
                "the service binary does not match the release's full Greyproxy commit".into(),
            ));
        }
        Ok(())
    }

    fn setup_lock(&self) -> Result<File, ProxyError> {
        private_dir(&self.data_dir, self.uid)?;
        let path = self.data_dir.join("setup.lock");
        if path.symlink_metadata().is_ok() {
            let _ = read_file(&path, self.uid, true)?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        file.try_lock()
            .map_err(|_| ProxyError::Unhealthy("another setup command is in progress".into()))?;
        Ok(file)
    }

    fn owned(&self) -> Result<Self, ProxyError> {
        match fs::symlink_metadata(&self.data_dir) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ProxyError::NotConfigured);
            }
            Err(error) => return Err(error.into()),
            Ok(metadata)
                if !metadata.is_dir()
                    || metadata.uid() != self.uid
                    || metadata.mode() & 0o077 != 0 =>
            {
                return Err(ProxyError::UnsafePath(self.data_dir.clone()));
            }
            Ok(_) => {}
        }
        if !self.receipt_path().exists() {
            return Err(ProxyError::NotConfigured);
        }
        let receipt: serde_json::Value =
            serde_json::from_str(&read_file(&self.receipt_path(), self.uid, true)?)
                .map_err(|_| ProxyError::ConfigurationChanged(self.receipt_path()))?;
        let mut owned = self.clone();
        owned.program = PathBuf::from(
            receipt["program"]
                .as_str()
                .ok_or_else(|| ProxyError::ConfigurationChanged(self.receipt_path()))?,
        );
        self.validate_owned_program(&owned.program)?;
        let configuration = read_file(&self.configuration_path(), self.uid, true)?;
        let unit = read_file(&self.unit_path, self.uid, false)?;
        if receipt["profile"] != self.profile.name()
            || receipt["configuration"] != configuration
            || receipt["unit"] != unit
            || configuration != self.configuration()
            || unit != owned.unit_text()?
        {
            return Err(ProxyError::ConfigurationChanged(self.unit_path.clone()));
        }
        Ok(owned)
    }

    /// A receipt can name an earlier immutable pair only below this profile's engine root.
    /// This permits stopping that owned service without executing an unpinned old binary.
    fn validate_owned_program(&self, program: &Path) -> Result<(), ProxyError> {
        let root = self
            .data_dir
            .parent()
            .expect("profile state")
            .join("engines");
        let relative = program
            .strip_prefix(&root)
            .map_err(|_| ProxyError::UnsafePath(program.into()))?;
        let parts: Vec<_> = relative.iter().collect();
        let valid_hashes = parts
            .first()
            .and_then(|part| part.to_str())
            .is_some_and(|name| {
                name.split_once('-').is_some_and(|(wall, proxy)| {
                    [wall, proxy].into_iter().all(|hash| {
                        hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                })
            });
        if parts.len() != 3 || !valid_hashes || parts[1] != "greyproxy" || parts[2] != "greyproxy" {
            return Err(ProxyError::UnsafePath(program.into()));
        }
        let mut path = root;
        for part in std::iter::once(None).chain(parts.into_iter().map(Some)) {
            if let Some(part) = part {
                path.push(part);
            }
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.uid() != self.uid
                || metadata.mode() & 0o022 != 0
                || if path == program {
                    !metadata.is_file()
                } else {
                    !metadata.is_dir()
                }
            {
                return Err(ProxyError::UnsafePath(path));
            }
        }
        Ok(())
    }

    fn write_configuration(&self) -> Result<(), ProxyError> {
        private_dir(&self.data_dir, self.uid)?;
        let parent = self.unit_path.parent().expect("unit has a parent");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        if fs::symlink_metadata(parent)?.file_type().is_symlink() {
            return Err(ProxyError::UnsafePath(parent.into()));
        }
        let settings = self.data_dir.join("settings.json");
        let mut files = self.configuration_files()?;
        if settings.symlink_metadata().is_err() {
            files.push((
                settings,
                "{\"conversationsEnabled\":false,\"notificationsEnabled\":false}\n".into(),
            ));
        } else {
            serde_json::from_str::<serde_json::Value>(&read_file(&settings, self.uid, true)?)
                .map_err(|_| ProxyError::ConfigurationChanged(settings))?;
        }
        let certificate = self.data_dir.join("ca-cert.pem");
        let key = self.data_dir.join("ca-key.pem");
        if certificate.symlink_metadata().is_err() && key.symlink_metadata().is_err() {
            // The pinned generator only writes this pair. Never call `cert install`,
            // which would change the OS trust store. Stage into a private scratch
            // directory so a failed or interrupted generator cannot leave half a CA.
            let staging = tempfile::tempdir_in(&self.data_dir)?;
            let output = command::run(
                Command::new(&self.program)
                    .env("HOME", &self.home)
                    .env("GREYPROXY_DATA_HOME", staging.path())
                    .args(["cert", "generate"]),
                None,
                self.command_deadline,
            )
            .map_err(ProxyError::Unhealthy)?;
            if !output.status.success() {
                return Err(ProxyError::Unhealthy(
                    "the pinned Greyproxy CA generator failed".into(),
                ));
            }
            for path in [certificate, key] {
                let source = staging.path().join(path.file_name().expect("CA filename"));
                let text = read_file(&source, self.uid, source.ends_with("ca-key.pem"))?;
                files.push((path, text));
            }
        } else {
            self.ca_pair()?;
        }
        // Prepare every write first; permission/allocation errors leave the old setup intact.
        let staged = files
            .into_iter()
            .map(|(path, text)| {
                let previous = match fs::symlink_metadata(&path) {
                    Ok(_) => Some(read_file(&path, self.uid, path != self.unit_path)?),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.into()),
                };
                let mut file =
                    tempfile::NamedTempFile::new_in(path.parent().expect("owned parent"))?;
                file.write_all(text.as_bytes())?;
                file.as_file().sync_all()?;
                Ok((path, previous, file))
            })
            .collect::<Result<Vec<_>, ProxyError>>()?;
        let mut written: Vec<(PathBuf, Option<String>)> = Vec::new();
        for (path, previous, file) in staged {
            if let Err(error) = file.persist(&path) {
                for (path, previous) in written.into_iter().rev() {
                    match previous {
                        Some(text) => write_private(&path, &text)?,
                        None => fs::remove_file(path)?,
                    }
                }
                return Err(error.error.into());
            }
            written.push((path, previous));
        }
        Ok(())
    }

    fn ca_pair(&self) -> Result<(), ProxyError> {
        for (name, private, kind) in [
            ("ca-cert.pem", false, "CERTIFICATE"),
            ("ca-key.pem", true, "EC PRIVATE KEY"),
        ] {
            let path = self.data_dir.join(name);
            let text = read_file(&path, self.uid, private).map_err(|error| match error {
                ProxyError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    ProxyError::ConfigurationChanged(path.clone())
                }
                other => other,
            })?;
            // The pinned engine validates DER and the matching pair when loading the
            // explicit configuration. Here reject missing/empty/replaced private files.
            if !text.starts_with(&format!("-----BEGIN {kind}-----\n"))
                || !text.trim_end().ends_with(&format!("-----END {kind}-----"))
            {
                return Err(ProxyError::ConfigurationChanged(path));
            }
        }
        Ok(())
    }

    fn check_ports(&self) -> Result<PortReservation, ProxyError> {
        let mut reservation = PortReservation {
            _tcp: Vec::new(),
            _udp: Vec::new(),
        };
        for port in self.ports {
            reservation._tcp.push(
                TcpListener::bind((Ipv4Addr::LOCALHOST, port))
                    .map_err(|_| ProxyError::EndpointOccupied(format!("127.0.0.1:{port}/tcp")))?,
            );
            match TcpListener::bind((Ipv6Addr::LOCALHOST, port)) {
                Ok(listener) => reservation._tcp.push(listener),
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    return Err(ProxyError::EndpointOccupied(format!("[::1]:{port}/tcp")));
                }
                // A host with IPv6 disabled still supports the pinned IPv4 proxy.
                Err(error)
                    if error.kind() == std::io::ErrorKind::AddrNotAvailable
                        || error.kind() == std::io::ErrorKind::Unsupported => {}
                Err(error) => return Err(error.into()),
            }
        }
        let port = self.ports[3];
        reservation._udp.push(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, port))
                .map_err(|_| ProxyError::EndpointOccupied(format!("127.0.0.1:{port}/udp")))?,
        );
        Ok(reservation)
    }

    fn request(&self, path: &str, token: Option<&str>) -> Result<(u16, String), ProxyError> {
        let url = format!("http://127.0.0.1:{}{path}", self.ports[0]);
        let mut command = Command::new(&self.curl);
        command
            .env("HOME", &self.home)
            .env("COCKPIT_PROFILE", self.profile.name())
            .args([
                "-q",
                "--silent",
                "--show-error",
                "--noproxy",
                "*",
                "--proxy",
                "",
                "--proto",
                "=http",
                "--connect-timeout",
                "1",
                "--max-time",
                "2",
                "--max-filesize",
                "16384",
                "--write-out",
                "\n%{http_code}",
            ]);
        let header = token.map(|token| format!("Authorization: Bearer {token}\n"));
        if header.is_some() {
            command.args(["--header", "@-"]);
        }
        command.arg(url);
        let output = command::run(
            &mut command,
            header.as_deref().map(str::as_bytes),
            self.command_deadline.min(Duration::from_secs(3)),
        )
        .map_err(ProxyError::Unhealthy)?;
        if !output.status.success() {
            return Err(ProxyError::Unhealthy(format!(
                "bounded local HTTP check failed ({})",
                output.status
            )));
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|_| ProxyError::Unhealthy("non-UTF-8 HTTP response".into()))?;
        let (body, code) = text
            .rsplit_once('\n')
            .ok_or_else(|| ProxyError::Unhealthy("missing HTTP status".into()))?;
        let code = code
            .trim()
            .parse()
            .map_err(|_| ProxyError::Unhealthy("invalid HTTP status".into()))?;
        Ok((code, body.into()))
    }

    fn wait_healthy(&self) -> Result<(), ProxyError> {
        let deadline = Instant::now() + STARTUP_DEADLINE;
        loop {
            match self.health() {
                Ok(_) => return Ok(()),
                Err(error) if Instant::now() >= deadline => return Err(error),
                Err(ProxyError::ConfigurationChanged(path) | ProxyError::UnsafePath(path)) => {
                    return Err(ProxyError::ConfigurationChanged(path));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }
}

#[derive(Debug)]
struct PortReservation {
    _tcp: Vec<TcpListener>,
    _udp: Vec<UdpSocket>,
}

fn private_dir(path: &Path, uid: u32) -> Result<(), ProxyError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(ProxyError::UnsafePath(path.into()));
    }
    Ok(())
}

fn read_file(path: &Path, uid: u32, private: bool) -> Result<String, ProxyError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & if private { 0o077 } else { 0o022 } != 0
        || metadata.len() > FILE_LIMIT
    {
        return Err(ProxyError::UnsafePath(path.into()));
    }
    let mut text = String::new();
    File::open(path)?
        .take(FILE_LIMIT + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > FILE_LIMIT {
        return Err(ProxyError::UnsafePath(path.into()));
    }
    Ok(text)
}

fn write_private(path: &Path, text: &str) -> Result<(), ProxyError> {
    if fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_file()) {
        return Err(ProxyError::UnsafePath(path.into()));
    }
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().expect("owned file has a parent"))?;
    file.write_all(text.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|error| ProxyError::Io(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests;
