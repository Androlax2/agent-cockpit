//! The first backend: Cockpit's patched Greywall and greyproxy forks (decision register item
//! 15). Greywall builds the sandbox (bubblewrap, Landlock, seccomp and a network namespace on
//! Linux; Seatbelt on macOS) and sends its traffic through greyproxy.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{EndpointKind, FolderMount, Platform, Profile, Refusal, SecretHandling};

/// The greywall fork build the release pins (`packages/greywall/build.sh` builds the same
/// commit until the release bundle takes over, #61): an installed binary of another commit
/// is not the engine whose behavior the profile was checked against.
pub const PINNED_GREYWALL_COMMIT: &str = "7c06d51afd26cbc90a32c82375a393570c22ffc7";

/// The greyproxy fork build the release pins (`packages/greyproxy/build.sh`).
pub const PINNED_GREYPROXY_COMMIT: &str = "cdd5cde76dce1262c0fd8d6b1ab2bd9953df5e71";

pub const GREYWALL_PROGRAM: &str = "greywall";
pub const GREYPROXY_PROGRAM: &str = "greyproxy";

/// The engine's reviewed profiles for the providers' own folders and hosts. A provider
/// without one is refused rather than run under a profile written for another agent.
const ENGINE_PROFILES: &[&str] = &["claude", "codex", "opencode"];

/// The capability `greywall --version` reports when its filtered session-bus proxy can run.
const BUS_PROXY_CAPABILITY: &str = "D-Bus isolated";

/// The flag that shows a host folder at another path inside the sandbox,
/// `--mount <host-dir>:<sandbox-path>` (Theo's fork, Linux only; #446). A build has it when
/// `greywall --help` lists it.
pub const MOUNT_FLAG: &str = "--mount";

/// How long `greywall --version` and `greyproxy --version` may take.
const VERSION_DEADLINE: Duration = Duration::from_secs(10);

/// Where the two programs are.
#[derive(Debug, Clone)]
pub struct GreywallPrograms {
    pub greywall: PathBuf,
    pub greyproxy: PathBuf,
}

impl GreywallPrograms {
    /// Both looked up on `path` (a `PATH` value); `None` names the first one missing.
    pub fn on_path(path: Option<&OsString>) -> Result<GreywallPrograms, String> {
        let find = |program: &str| {
            path.and_then(|path| {
                std::env::split_paths(path)
                    .map(|dir| dir.join(program))
                    .find(|candidate| candidate.is_file())
            })
            .ok_or_else(|| format!("no `{program}` on PATH"))
        };
        Ok(GreywallPrograms {
            greywall: find(GREYWALL_PROGRAM)?,
            greyproxy: find(GREYPROXY_PROGRAM)?,
        })
    }
}

/// An installed build: its version and the commit it says it was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineBuild {
    pub version: String,
    pub commit: String,
}

impl EngineBuild {
    fn short(&self) -> String {
        format!(
            "{} ({})",
            self.version,
            &self.commit[..7.min(self.commit.len())]
        )
    }
}

/// One host capability `greywall --version` checks, as it reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCapability {
    pub name: String,
    pub available: bool,
    pub detail: Option<String>,
}

/// `greywall --version`: its build and the host capabilities it checked.
pub fn parse_greywall_version(output: &str) -> Result<(EngineBuild, Vec<HostCapability>), String> {
    let field = |label: &str| {
        output.lines().find_map(|line| {
            line.trim()
                .strip_prefix(label)
                .map(|value| value.trim().to_owned())
        })
    };
    let version = field("Version:").ok_or("it names no version")?;
    let commit = field("Commit:").ok_or("it names no commit")?;
    let capabilities = output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (available, rest) = if let Some(rest) = line.strip_prefix('✓') {
                (true, rest)
            } else {
                (false, line.strip_prefix('✗')?)
            };
            let rest = rest.trim();
            let (name, detail) = match rest.split_once(" (") {
                Some((name, detail)) => (
                    name.trim(),
                    Some(detail.trim_end_matches(')').trim().to_owned()),
                ),
                None => (rest, None),
            };
            Some(HostCapability {
                name: name.to_owned(),
                available,
                detail,
            })
        })
        .collect();
    Ok((EngineBuild { version, commit }, capabilities))
}

/// `greyproxy --version`: `greyproxy <version> (...)`, then a `commit:` line.
pub fn parse_greyproxy_version(output: &str) -> Result<EngineBuild, String> {
    let version = output
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("greyproxy "))
        .and_then(|rest| rest.split_whitespace().next())
        .ok_or("it does not start with `greyproxy <version>`")?
        .to_owned();
    let commit = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("commit:"))
        .map(|commit| commit.trim().to_owned())
        .ok_or("it names no commit")?;
    Ok(EngineBuild { version, commit })
}

/// A ready engine: the pinned builds, on a platform it supports, with the host capabilities
/// it reported.
#[derive(Debug, Clone)]
pub struct Greywall {
    pub programs: GreywallPrograms,
    pub greywall: EngineBuild,
    pub greyproxy: EngineBuild,
    pub platform: Platform,
    pub capabilities: Vec<HostCapability>,
    /// Whether `greywall --help` lists [`MOUNT_FLAG`].
    pub can_mount_folders: bool,
    /// Where each launch's settings file goes: owner-only, never inside a sandbox.
    pub settings_dir: PathBuf,
}

impl Greywall {
    /// Runs both programs' `--version` and `greywall --help` (up to 10 s each) and checks
    /// they are the pinned builds. The reason when the engine cannot be used.
    pub fn detect(
        programs: GreywallPrograms,
        platform: Platform,
        settings_dir: PathBuf,
    ) -> Result<Greywall, String> {
        let greywall_output = program_output(&programs.greywall, "--version")?;
        let can_mount_folders =
            lists_flag(&program_output(&programs.greywall, "--help")?, MOUNT_FLAG);
        let (greywall, capabilities) = parse_greywall_version(&greywall_output)
            .map_err(|reason| format!("{} --version: {reason}", programs.greywall.display()))?;
        let greyproxy = parse_greyproxy_version(&program_output(&programs.greyproxy, "--version")?)
            .map_err(|reason| format!("{} --version: {reason}", programs.greyproxy.display()))?;
        let mut engine = Greywall::from_builds(
            programs,
            greywall,
            greyproxy,
            capabilities,
            platform,
            settings_dir,
        )?;
        engine.can_mount_folders = can_mount_folders;
        Ok(engine)
    }

    /// [`Greywall::detect`] once the versions are read, for a build whose `--help` lists no
    /// [`MOUNT_FLAG`].
    pub fn from_builds(
        programs: GreywallPrograms,
        greywall: EngineBuild,
        greyproxy: EngineBuild,
        capabilities: Vec<HostCapability>,
        platform: Platform,
        settings_dir: PathBuf,
    ) -> Result<Greywall, String> {
        if greywall.commit != PINNED_GREYWALL_COMMIT {
            return Err(format!(
                "greywall at {} is commit {}, the release pins {PINNED_GREYWALL_COMMIT}",
                programs.greywall.display(),
                greywall.commit
            ));
        }
        if greyproxy.commit != PINNED_GREYPROXY_COMMIT {
            return Err(format!(
                "greyproxy at {} is commit {}, the release pins {PINNED_GREYPROXY_COMMIT}",
                programs.greyproxy.display(),
                greyproxy.commit
            ));
        }
        if let Platform::Unsupported(name) = &platform {
            return Err(format!("greywall does not run on {name}"));
        }
        Ok(Greywall {
            programs,
            greywall,
            greyproxy,
            platform,
            capabilities,
            can_mount_folders: false,
            settings_dir,
        })
    }

    /// `greywall 0.3.7+dotfiles (7c06d51), greyproxy 0.4.5 (cdd5cde)`.
    pub fn engine(&self) -> String {
        format!(
            "greywall {}, greyproxy {}",
            self.greywall.short(),
            self.greyproxy.short()
        )
    }

    /// The host capabilities as observed, one line each.
    pub fn observations(&self) -> Vec<String> {
        self.capabilities
            .iter()
            .map(|capability| {
                let mark = if capability.available { "yes" } else { "no" };
                match &capability.detail {
                    Some(detail) => format!("{}: {mark} ({detail})", capability.name),
                    None => format!("{}: {mark}", capability.name),
                }
            })
            .collect()
    }

    fn capability(&self, name: &str) -> Option<&HostCapability> {
        self.capabilities
            .iter()
            .find(|capability| capability.name == name)
    }

    /// The command that runs the agent in `profile` under Greywall, as `client`, with the
    /// settings file it reads, or the rule this host cannot enforce.
    pub fn plan(&self, profile: &Profile, client: &ClientIdentity) -> Result<Plan, Refusal> {
        if profile.platform != self.platform {
            return Err(Refusal::new(
                format!("platform {}", profile.platform),
                format!("the engine was checked on {}", self.platform),
            ));
        }
        let mut enforced_by = Vec::new();
        if !ENGINE_PROFILES.contains(&profile.provider.as_str()) {
            return Err(Refusal::new(
                format!("provider {}", profile.provider),
                format!(
                    "greywall has no reviewed profile for it (it has {})",
                    ENGINE_PROFILES.join(", ")
                ),
            ));
        }
        if matches!(self.platform, Platform::Linux | Platform::Wsl2 { .. }) {
            for (capability, rule) in [
                ("bubblewrap", "read everything else: denied"),
                ("landlock", "read everything else: denied"),
                ("seccomp", "read everything else: denied"),
                ("network isolation", "network: through greyproxy only"),
            ] {
                match self.capability(capability) {
                    Some(found) if found.available => {}
                    Some(found) => {
                        return Err(Refusal::new(
                            rule,
                            format!(
                                "greywall reports {capability} unavailable on this host{}",
                                found
                                    .detail
                                    .as_ref()
                                    .map(|detail| format!(" ({detail})"))
                                    .unwrap_or_default()
                            ),
                        ));
                    }
                    None => {
                        return Err(Refusal::new(
                            rule,
                            format!("greywall did not report {capability} on this host"),
                        ));
                    }
                }
            }
        }
        self.check_bus(profile)?;
        let reach = self.provider_reach(profile);
        let settings_path = self.settings_dir.join(format!("{}.json", client.as_str()));
        let mut arguments: Vec<OsString> = vec![
            "--settings".into(),
            settings_path.clone().into(),
            "--auto-profile".into(),
            "--profile".into(),
            profile.provider.clone().into(),
            "--proxy-user".into(),
            client.as_str().into(),
        ];
        let (filesystem, network) = match self.platform {
            Platform::MacOs => ("greywall Seatbelt", "greywall proxy environment"),
            _ => ("greywall bubblewrap+Landlock", "greywall network namespace"),
        };
        for rule in profile.rules() {
            let by = if rule.starts_with("platform ") {
                format!("checked: {}", self.engine())
            } else if rule.starts_with("provider ") {
                format!("greywall --profile {}", profile.provider)
            } else if rule.starts_with("folder ") {
                match &reach {
                    ProviderReach::Mounted(_) => "greywall --mount".to_owned(),
                    ProviderReach::Direct | ProviderReach::Unreachable { .. } => format!(
                        "not mounted: this greywall cannot `{MOUNT_FLAG}`; the daemon refuses \
                         delivery to the agent"
                    ),
                }
            } else if rule.starts_with("read-write ") || rule.starts_with("desktop socket ") {
                format!("{filesystem} (--allow-path)")
            } else if rule.starts_with("desktop file ") {
                format!("{filesystem} (--allow-read-path)")
            } else if rule.starts_with("desktop bus ") {
                "greywall's xdg-dbus-proxy in its place, org.freedesktop.Notifications only"
                    .to_owned()
            } else if rule.starts_with("read-only ") {
                format!("{filesystem} (--allow-read-path)")
            } else if rule == "read everything else: denied" {
                format!("{filesystem} (defaultDenyRead)")
            } else if rule.starts_with("deny write ") {
                format!("{filesystem} (denyWrite)")
            } else if rule.starts_with("deny") {
                format!("{filesystem} (denyRead, applied last)")
            } else if rule.starts_with("network") {
                format!("{network} to greyproxy as {}", client.as_str())
            } else if rule.starts_with("allow host ") {
                "greyproxy (--allow)".to_owned()
            } else if rule.ends_with("as a placeholder") {
                "greyproxy credential substitution (--secret)".to_owned()
            } else if rule.ends_with("passed through") {
                "greywall --ignore-secret".to_owned()
            } else {
                match profile.desktop {
                    cockpit_config::DesktopSession::Withheld => {
                        "the environment holds none of the desktop variables".to_owned()
                    }
                    cockpit_config::DesktopSession::Shared => {
                        "the desktop variables passed; each endpoint they name is its own rule"
                            .to_owned()
                    }
                }
            };
            enforced_by.push((rule, by));
        }
        let read_write = [&profile.working_dir, &profile.session_dir]
            .into_iter()
            .chain(
                profile
                    .desktop_endpoints
                    .iter()
                    .filter(|endpoint| endpoint.kind == EndpointKind::Socket)
                    .map(|endpoint| &endpoint.path),
            );
        for path in read_write {
            arguments.push("--allow-path".into());
            arguments.push(path.clone().into());
        }
        let read_only = profile.read_only.iter().chain(
            profile
                .desktop_endpoints
                .iter()
                .filter(|endpoint| endpoint.kind == EndpointKind::File)
                .map(|endpoint| &endpoint.path),
        );
        for path in read_only {
            arguments.push("--allow-read-path".into());
            arguments.push(path.clone().into());
        }
        if let ProviderReach::Mounted(mounts) = &reach {
            for mount in mounts {
                let mut value = OsString::from(&mount.host);
                value.push(":");
                value.push(&mount.sandbox);
                arguments.push("--mount".into());
                arguments.push(value);
            }
        }
        for host in &profile.allow_hosts {
            arguments.push("--allow".into());
            arguments.push(host.into());
        }
        for secret in &profile.secrets {
            arguments.push(match secret.handling {
                SecretHandling::Placeholder => "--secret".into(),
                SecretHandling::PassThrough => "--ignore-secret".into(),
            });
            arguments.push(secret.variable.clone().into());
        }
        arguments.push("--".into());
        let denied_read: Vec<String> = profile
            .denied
            .iter()
            .map(|path| path.display().to_string())
            .chain(profile.denied_read_patterns.iter().cloned())
            .collect();
        let settings = serde_json::json!({
            "filesystem": {
                "defaultDenyRead": true,
                "denyRead": denied_read,
                "denyWrite": profile.denied_write_patterns,
            },
        });
        let mut environment = profile.environment.clone();
        // The engine's drift prompt would wait for a terminal answer the daemon never gives.
        environment.push(("GREYWALL_NO_PROMPT".into(), "1".into()));
        Ok(Plan {
            program: self.programs.greywall.clone(),
            arguments,
            environment,
            settings_path,
            settings: format!("{settings:#}\n"),
            enforced_by,
            reach,
        })
    }

    /// Whether the daemon can reach what the provider publishes in its folders: Codex's
    /// sockets, through the session's own folder mounted at Codex's path, only when this
    /// greywall can mount one (#446). Never by sharing the host's folder at that path.
    fn provider_reach(&self, profile: &Profile) -> ProviderReach {
        if profile.folder_mounts.is_empty() {
            return ProviderReach::Direct;
        }
        // Greywall mounts on Linux only, and refuses a macOS configuration with mounts.
        let can_mount = self.can_mount_folders && self.platform != Platform::MacOs;
        if can_mount {
            return ProviderReach::Mounted(profile.folder_mounts.clone());
        }
        let missing = if self.can_mount_folders {
            format!(
                "greywall mounts folders only on Linux, not on {}",
                self.platform
            )
        } else {
            format!("greywall {}'s --help lists no {MOUNT_FLAG}", self.engine())
        };
        ProviderReach::Unreachable {
            feature: MOUNT_FLAG,
            reason: format!(
                "{missing}: {}'s sockets stay in the sandbox's private /tmp, out of the \
                 daemon's reach",
                profile.provider
            ),
        }
    }

    /// A shared session bus reaches the agent only through greywall's filtered proxy, which
    /// it mounts where the owner's bus is under systemd, `/run/user/<uid>/bus`: the owner's
    /// own bus starts programs outside the sandbox and is never mounted.
    fn check_bus(&self, profile: &Profile) -> Result<(), Refusal> {
        let Some(bus) = profile
            .desktop_endpoints
            .iter()
            .find(|endpoint| endpoint.kind == EndpointKind::Bus)
        else {
            return Ok(());
        };
        let rule = format!("desktop bus {} for {}", bus.path.display(), bus.variable);
        if self.platform == Platform::MacOs {
            return Err(Refusal::new(
                rule,
                "greywall proxies the session bus only on Linux; withhold the desktop session \
                 or unset DBUS_SESSION_BUS_ADDRESS",
            ));
        }
        if !self
            .capability(BUS_PROXY_CAPABILITY)
            .is_some_and(|found| found.available)
        {
            return Err(Refusal::new(
                rule,
                "greywall reports no `D-Bus isolated` (xdg-dbus-proxy) on this host, so the \
                 session bus could only be the owner's own; install xdg-dbus-proxy or withhold \
                 the desktop session",
            ));
        }
        let proxied = PathBuf::from(format!("/run/user/{}/bus", profile.owner_uid));
        if bus.path != proxied {
            return Err(Refusal::new(
                rule,
                format!(
                    "greywall's filtered proxy takes the place of {} only",
                    proxied.display()
                ),
            ));
        }
        Ok(())
    }
}

/// How one launch runs under Greywall: the program and arguments before the agent's own,
/// the environment, and the settings file to write first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub program: PathBuf,
    /// Ends with `--`; the agent's program and arguments follow.
    pub arguments: Vec<OsString>,
    pub environment: Vec<(OsString, OsString)>,
    pub settings_path: PathBuf,
    pub settings: String,
    /// Each profile rule with what enforces it.
    pub enforced_by: Vec<(String, String)>,
    /// How the daemon reaches what the provider publishes in its folders.
    pub reach: ProviderReach,
}

/// How the daemon reaches the sockets a sandboxed provider publishes, such as Codex's
/// app-server (#446).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderReach {
    /// At the paths the provider names: they are the host's.
    Direct,
    /// Through these folders: a link the provider leaves to a path under a mount's
    /// `sandbox` names the same entry under its `host`.
    Mounted(Vec<FolderMount>),
    /// Not at all: the engine lacks `feature`. Delivery to the agent is refused with
    /// `reason`; the launch itself goes ahead.
    Unreachable {
        feature: &'static str,
        reason: String,
    },
}

impl Plan {
    /// Creates each mounted folder, and the session's folder holding it, owner-only in the
    /// daemon's mount sources, or checks the ones there are real folders owned like the
    /// mount sources. Greywall mounts only folders that exist. The folder that is not ready,
    /// with why.
    pub fn create_mount_sources(&self) -> Result<(), (PathBuf, std::io::Error)> {
        let ProviderReach::Mounted(mounts) = &self.reach else {
            return Ok(());
        };
        for mount in mounts {
            create_owned_folder(&mount.host).map_err(|error| (mount.host.clone(), error))?;
        }
        Ok(())
    }
}

/// `folder` and the session's folder holding it, real folders owned by whoever owns the
/// mount sources above them: the daemon.
fn create_owned_folder(folder: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let session_folder = folder
        .parent()
        .ok_or_else(|| std::io::Error::other("it has no session folder"))?;
    let mount_sources = session_folder
        .parent()
        .ok_or_else(|| std::io::Error::other("it is not in the daemon's mount sources"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)?;
    let owner = std::fs::symlink_metadata(mount_sources)?.uid();
    for created in [session_folder, folder] {
        let found = std::fs::symlink_metadata(created)?;
        if !(found.is_dir() && found.uid() == owner) {
            return Err(std::io::Error::other(format!(
                "{} is not a folder of user {owner}, so nothing is mounted",
                created.display()
            )));
        }
    }
    Ok(())
}

impl Plan {
    /// Writes the settings file whole, owner-only, under a name of its own before moving it
    /// into place, so concurrent launches never read each other's half-written file.
    pub fn write_settings(&self) -> std::io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let dir = self
            .settings_path
            .parent()
            .ok_or_else(|| std::io::Error::other("the settings path has no folder"))?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        let staged = dir.join(format!(
            ".{}.{}",
            self.settings_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            std::process::id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&staged)?;
        file.write_all(self.settings.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&staged, &self.settings_path)?;
        self.remove_earlier_settings(dir);
        Ok(())
    }

    /// The same session's earlier launches' files: their engines read them at start. Best
    /// effort; a file left behind holds no secret.
    fn remove_earlier_settings(&self, dir: &Path) {
        let Some(current) = self
            .settings_path
            .file_name()
            .and_then(|name| name.to_str())
        else {
            return;
        };
        let Some((session_prefix, _nonce)) = current
            .strip_suffix(".json")
            .and_then(|identity| identity.rsplit_once('-'))
        else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            // Exactly this session's prefix and a nonce: a session whose id merely starts
            // with this one's (`s1` and `s1-2`) keeps its files.
            let is_earlier = name != current
                && name
                    .strip_prefix(session_prefix)
                    .and_then(|rest| rest.strip_prefix('-'))
                    .and_then(|rest| rest.strip_suffix(".json"))
                    .is_some_and(is_nonce);
            if is_earlier && let Err(error) = std::fs::remove_file(entry.path()) {
                tracing::debug!(%error, file = name, "an earlier sandbox settings file stays");
            }
        }
    }
}

/// The name a launch's sandbox logs in to greyproxy with: `cockpit-<session>-<random>`. A
/// fresh one per launch, because greyproxy keeps one client-secret registration per name and
/// greywall registers a per-run secret: two launches under one name replace each other's
/// secret, and the first sandbox loses its network (greywall stack proposal 00f3a41c).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIdentity(String);

const IDENTITY_NONCE_BYTES: usize = 6;

fn is_nonce(text: &str) -> bool {
    text.len() == IDENTITY_NONCE_BYTES * 2
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl ClientIdentity {
    pub fn for_launch(session: &str) -> std::io::Result<ClientIdentity> {
        let mut nonce = [0u8; IDENTITY_NONCE_BYTES];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut nonce)?;
        let session: String = session
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '-'
                }
            })
            .collect();
        Ok(ClientIdentity(format!(
            "cockpit-{session}-{}",
            nonce
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A program's `--version` output (stdout and stderr), whatever its exit status: greywall
/// exits non-zero when a host capability is missing and still prints its build.
/// Whether `help`, a program's `--help`, lists `flag` among its options: on an option line
/// (`  -s, --settings string  …`), never in an example.
pub fn lists_flag(help: &str, flag: &str) -> bool {
    help.lines().any(|line| {
        line.split_whitespace()
            .take_while(|word| word.starts_with('-'))
            .any(|word| word.trim_end_matches(',') == flag)
    })
}

/// What `program <flag>` prints, within [`VERSION_DEADLINE`].
fn program_output(program: &Path, flag: &str) -> Result<String, String> {
    let mut child = Command::new(program)
        .arg(flag)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{} does not run: {error}", program.display()))?;
    // Read while it runs, on threads of their own: a full pipe never stalls the program, and
    // a child it leaves holding the pipes open cannot stall the daemon's start past the
    // deadline.
    let (sender, outputs) = std::sync::mpsc::channel();
    for mut stream in [
        child
            .stdout
            .take()
            .map(|out| Box::new(out) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|err| Box::new(err) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stream.read_to_string(&mut text);
            let _ = sender.send(text);
        });
    }
    drop(sender);
    let deadline = Instant::now() + VERSION_DEADLINE;
    let late = || {
        format!(
            "{} {flag} did not answer within {}s",
            program.display(),
            VERSION_DEADLINE.as_secs()
        )
    };
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(late());
            }
            Err(error) => return Err(format!("{} {flag}: {error}", program.display())),
        }
    }
    let mut output = String::new();
    for _ in 0..2 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match outputs.recv_timeout(remaining) {
            Ok(text) => output.push_str(&text),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Err(late()),
        }
    }
    Ok(output)
}
