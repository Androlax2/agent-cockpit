#![doc = include_str!("../README.md")]

pub mod events;
mod http;
mod process;
pub mod server;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::net::{Ipv4Addr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cockpit_provider_api::{
    Action, Activity, Capabilities, Capability, Conversation, Key, LaunchCommand, LaunchRequest,
    Provenance, Provider, ProviderError, ProviderName, TerminalInput,
};

pub use server::{Invocation, NativeCommand, Server, ServerError};

/// The OpenCode version this adapter's conformance tests pin; capabilities are `tested` only
/// on it.
pub const CONFORMANCE_VERSION: &str = "1.19.5";

/// Every flag a launch may pass; the conformance test finds each in `opencode --help`.
pub const LAUNCH_FLAGS: [&str; 4] = ["--port", "--hostname", "--session", "--prompt"];

/// The only address a session's server listens on.
pub const SERVER_HOSTNAME: &str = "127.0.0.1";

/// The user name of the server's basic authentication; OpenCode's default, set explicitly so
/// an inherited `OPENCODE_SERVER_USERNAME` cannot change it.
pub const SERVER_USERNAME: &str = "opencode";

/// The variables a launch sets for OpenCode itself; the conformance test finds each name in
/// the binary. The password protects the session's server from every other local user and
/// program; the inline configuration loads the plugin bridge into this server only.
pub const OPENCODE_VARIABLES: [&str; 3] = [
    "OPENCODE_SERVER_PASSWORD",
    "OPENCODE_SERVER_USERNAME",
    "OPENCODE_CONFIG_CONTENT",
];

/// The variables the plugin reads: the bridge program, the `cockpit` program it reports
/// through, and the port of the server the plugin must run in.
pub const PLUGIN_VARIABLES: [&str; 3] = [
    "COCKPIT_OPENCODE_BRIDGE",
    "COCKPIT_OPENCODE_COCKPIT",
    "COCKPIT_OPENCODE_PORT",
];

/// The name Cockpit's MCP server (`cockpit mcp`) has in every harness a launch registers it
/// with (#419).
pub const MCP_SERVER_NAME: &str = "cockpit";

/// The plugin OpenCode loads in every launch with a bridge. The daemon writes it to the
/// file [`Bridge::plugin`] names; [`Bridge::check`] refuses a file with other content.
pub const PLUGIN_SOURCE: &str = include_str!("../plugin/cockpit-opencode-plugin.js");

/// How long `opencode --version` may take at discovery.
const VERSION_DEADLINE: Duration = Duration::from_secs(10);

/// How long the bridge may take to answer [`BRIDGE_VERSION_FLAG`] at the daemon's start.
const BRIDGE_CHECK_DEADLINE: Duration = Duration::from_secs(10);

/// The argument that makes `cockpit-opencode-bridge` print [`bridge_version_line`] and exit.
pub const BRIDGE_VERSION_FLAG: &str = "--version";

/// What `cockpit-opencode-bridge --version` prints: its name and the version it was built as.
pub fn bridge_version_line() -> String {
    format!("cockpit-opencode-bridge {}", env!("CARGO_PKG_VERSION"))
}

/// What a launch ships into the sandbox: the plugin OpenCode loads, the bridge program it
/// runs for each event it forwards, and the `cockpit` program the bridge reports through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    pub plugin: PathBuf,
    pub bridge_program: PathBuf,
    pub cockpit_program: PathBuf,
}

impl Bridge {
    /// Whether this bridge can run here: the three files exist, the plugin is this release's
    /// [`PLUGIN_SOURCE`], `cockpit` has its execute bit, and the bridge runs and answers
    /// [`BRIDGE_VERSION_FLAG`] with this release's [`bridge_version_line`]. The reason when
    /// not. Blocks up to 10 seconds.
    pub fn check(&self) -> Result<(), String> {
        self.check_within(BRIDGE_CHECK_DEADLINE)
    }

    /// [`Bridge::check`], giving up on the bridge after `deadline`.
    pub fn check_within(&self, deadline: Duration) -> Result<(), String> {
        let missing: Vec<String> = [&self.plugin, &self.bridge_program, &self.cockpit_program]
            .into_iter()
            .filter(|file| !file.is_file())
            .map(|file| file.display().to_string())
            .collect();
        if !missing.is_empty() {
            return Err(format!("{} missing", missing.join(" and ")));
        }
        if !self.plugin.is_absolute() {
            return Err(format!(
                "{} is not an absolute path, which OpenCode would resolve from the project",
                self.plugin.display()
            ));
        }
        let plugin = std::fs::read_to_string(&self.plugin)
            .map_err(|error| format!("{}: {error}", self.plugin.display()))?;
        if plugin != PLUGIN_SOURCE {
            return Err(format!(
                "{} is not this release's plugin",
                self.plugin.display()
            ));
        }
        if !is_executable(&self.cockpit_program) {
            return Err(format!(
                "{} has no execute bit, so the bridge cannot run it",
                self.cockpit_program.display()
            ));
        }
        let answer =
            process::run_with_deadline(&self.bridge_program, BRIDGE_VERSION_FLAG, deadline)
                .map_err(|reason| format!("{}: {reason}", self.bridge_program.display()))?;
        let expected = bridge_version_line();
        if answer != expected {
            return Err(format!(
                "{}: `{BRIDGE_VERSION_FLAG}` printed {answer:?}, not this release's {expected:?}",
                self.bridge_program.display()
            ));
        }
        Ok(())
    }

    /// `OPENCODE_CONFIG_CONTENT`: OpenCode merges it over the owner's configuration files
    /// and appends its plugin to theirs; it also registers `cockpit mcp` as the
    /// [`MCP_SERVER_NAME`] MCP server (#419). `owner` is the value the owner already set,
    /// which this one replaces: its settings are kept, the plugin appended to its own and the
    /// server added to its own. Refused when it is not a JSON object, rather than dropping the
    /// owner's settings unseen.
    pub fn inline_config(&self, owner: Option<&OsStr>) -> Result<String, ProviderError> {
        let refused = |why: &str| ProviderError {
            reason: format!("OPENCODE_CONFIG_CONTENT {why}, so the plugin cannot join it"),
        };
        let mut config = match owner {
            None => serde_json::json!({}),
            Some(owner) => {
                let text = owner.to_str().ok_or_else(|| refused("is not UTF-8"))?;
                serde_json::from_str(text).map_err(|_| refused("is not JSON"))?
            }
        };
        let config_object = config
            .as_object_mut()
            .ok_or_else(|| refused("is not a JSON object"))?;
        let plugins = config_object
            .entry("plugin")
            .or_insert_with(|| serde_json::json!([]))
            .as_array_mut()
            .ok_or_else(|| refused("has a `plugin` that is not a list"))?;
        plugins.push(self.plugin.to_string_lossy().into());
        let servers = config_object
            .entry("mcp")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or_else(|| refused("has an `mcp` that is not an object"))?;
        servers.insert(
            MCP_SERVER_NAME.to_owned(),
            serde_json::json!({
                "type": "local",
                "command": [self.cockpit_program.to_string_lossy(), "mcp"],
            }),
        );
        Ok(config.to_string())
    }
}

fn is_executable(program: &Path) -> bool {
    std::fs::metadata(program).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

/// The OpenCode adapter.
#[derive(Debug)]
pub struct OpenCode {
    name: ProviderName,
    program: PathBuf,
    version: Option<String>,
    /// Without it, launches load no plugin and `events` is not reported.
    bridge: Option<Bridge>,
}

impl OpenCode {
    /// The adapter for `program`, running `version` when known.
    pub fn new(program: PathBuf, version: Option<String>, bridge: Option<Bridge>) -> OpenCode {
        OpenCode {
            name: ProviderName::new("opencode").expect("`opencode` is a valid provider name"),
            program,
            version,
            bridge,
        }
    }

    /// The adapter for `program`, with the version it prints. Blocks up to 10 seconds.
    pub fn discover(program: &Path, bridge: Option<Bridge>) -> Result<OpenCode, DiscoveryError> {
        OpenCode::discover_within(program, bridge, VERSION_DEADLINE)
    }

    /// [`OpenCode::discover`], giving up on `--version` after `deadline`.
    pub fn discover_within(
        program: &Path,
        bridge: Option<Bridge>,
        deadline: Duration,
    ) -> Result<OpenCode, DiscoveryError> {
        let failed = |reason: String| DiscoveryError {
            program: program.to_path_buf(),
            reason,
        };
        let output = process::run_with_deadline(program, "--version", deadline).map_err(failed)?;
        let version = output
            .split_whitespace()
            .next()
            .map(|word| word.strip_prefix('v').unwrap_or(word))
            .filter(|word| is_dotted_version(word))
            .ok_or_else(|| failed(format!("`--version` printed {output:?}, not a version")))?;
        Ok(OpenCode::new(
            program.to_path_buf(),
            Some(version.to_owned()),
            bridge,
        ))
    }

    /// The first executable `opencode` in the directories of `path` (a `PATH` value).
    pub fn find_on_path(path: &OsStr) -> Option<PathBuf> {
        std::env::split_paths(path)
            .map(|dir| dir.join("opencode"))
            .find(|candidate| {
                candidate
                    .metadata()
                    .is_ok_and(|metadata| metadata.is_file() && is_executable(candidate))
            })
    }

    /// `tested` on [`CONFORMANCE_VERSION`], `discovered` on any other version; every
    /// capability it reports has this provenance.
    pub fn provenance(&self) -> Provenance {
        match &self.version {
            Some(version) if version == CONFORMANCE_VERSION => Provenance::Tested {
                version: version.clone(),
            },
            _ => Provenance::Discovered,
        }
    }
}

impl Provider for OpenCode {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    fn capabilities(&self) -> Capabilities {
        // Events and outcomes reach Cockpit only through the plugin bridge, and without
        // them the activity is never known, so `input` would refuse every submit and cancel.
        let bridged = self.bridge.is_some().then_some([
            Capability::Submit,
            Capability::Cancel,
            Capability::Events,
            Capability::Outcome,
        ]);
        let provenance = self.provenance();
        [Capability::Resume]
            .into_iter()
            .chain(bridged.into_iter().flatten())
            .fold(Capabilities::none(), |capabilities, capability| {
                capabilities.with(capability, provenance.clone())
            })
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        request.require(&self.capabilities())?;
        let port = free_loopback_port()?;
        let mut arguments: Vec<OsString> = vec![
            "--port".into(),
            port.to_string().into(),
            "--hostname".into(),
            SERVER_HOSTNAME.into(),
        ];
        // OpenCode names a new session only when its first prompt is submitted, so the
        // bridge's first report names it (cockpit-sessions follows it).
        let conversation = match request.conversation {
            Conversation::New => None,
            Conversation::Resume(id) => {
                let id = server::session_id(id)?;
                arguments.extend(["--session".into(), id.as_str().into()]);
                Some(id)
            }
        };
        // One word, so a message starting with `-` is never read as a flag.
        if let Some(message) = request.first_message {
            let mut prompt = OsString::from("--prompt=");
            prompt.push(message);
            arguments.push(prompt);
        }
        let mut environment: Vec<(OsString, OsString)> = vec![
            ("OPENCODE_SERVER_PASSWORD".into(), new_password()?.into()),
            ("OPENCODE_SERVER_USERNAME".into(), SERVER_USERNAME.into()),
        ];
        if let Some(bridge) = &self.bridge {
            let [bridge_variable, cockpit_variable, port_variable] = PLUGIN_VARIABLES;
            environment.extend([
                (
                    "OPENCODE_CONFIG_CONTENT".into(),
                    bridge
                        .inline_config(std::env::var_os("OPENCODE_CONFIG_CONTENT").as_deref())?
                        .into(),
                ),
                (bridge_variable.into(), bridge.bridge_program.clone().into()),
                (
                    cockpit_variable.into(),
                    bridge.cockpit_program.clone().into(),
                ),
                (port_variable.into(), port.to_string().into()),
            ]);
        }
        Ok(LaunchCommand {
            program: self.program.clone(),
            arguments,
            environment,
            conversation,
        })
    }

    fn input(
        &self,
        action: &Action<'_>,
        activity: Option<Activity>,
    ) -> Result<Vec<TerminalInput>, ProviderError> {
        let refused = |reason: &str| ProviderError {
            reason: reason.to_owned(),
        };
        match action {
            // The TUI aborts the turn on the second Escape within 5 seconds; while it waits,
            // Escape would close whatever is open instead.
            Action::Cancel => match activity {
                Some(Activity::Working) => Ok(vec![
                    TerminalInput::Key(Key::Escape),
                    TerminalInput::Key(Key::Escape),
                ]),
                _ => Err(refused("cancel runs only during a turn")),
            },
            Action::Compact => Err(ProviderError::unsupported(Capability::Compact)),
            Action::Submit(message) => {
                typed_message_refusal(message).map_or(Ok(()), |reason| Err(refused(reason)))?;
                match activity {
                    // The TUI submits during a turn too; OpenCode queues the message.
                    Some(Activity::Waiting | Activity::Working) => Ok(vec![
                        TerminalInput::Text((*message).to_owned()),
                        TerminalInput::Key(Key::Enter),
                    ]),
                    Some(Activity::NeedsYou) => {
                        Err(refused("a dialog is open in its terminal; answer it first"))
                    }
                    None => Err(refused(
                        "its activity is unknown, so a message could answer a dialog",
                    )),
                }
            }
        }
    }
}

/// Why typing `message` into the TUI's prompt would not submit it as written, if it would
/// not. A leading `/` or `!` and any `@` open a menu that Enter would pick from; `exit`,
/// `quit` and `:q` quit OpenCode. [`Server::deliver`] sends any text as written.
fn typed_message_refusal(message: &str) -> Option<&'static str> {
    let trimmed = message.trim();
    if trimmed.is_empty() || message.chars().any(char::is_control) {
        return Some("a message is one line of text");
    }
    if trimmed.starts_with(['/', '!']) || message.contains('@') {
        return Some("a typed message starting with `/` or `!`, or holding `@`, opens a menu");
    }
    if ["exit", "quit", ":q"].contains(&trimmed) {
        return Some("typing that message quits OpenCode");
    }
    None
}

/// OpenCode's program could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryError {
    pub program: PathBuf,
    pub reason: String,
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.program.display(), self.reason)
    }
}

impl std::error::Error for DiscoveryError {}

fn is_dotted_version(word: &str) -> bool {
    let parts: Vec<&str> = word.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// A loopback port free right now. Another program may take it before OpenCode listens;
/// OpenCode then fails to start, which the session shows, rather than share a server.
fn free_loopback_port() -> Result<u16, ProviderError> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| ProviderError {
            reason: format!("cannot find a free loopback port for its server: {error}"),
        })
}

/// 128 random bits as hex: the session server's password.
fn new_password() -> Result<String, ProviderError> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| ProviderError {
            reason: format!("cannot read /dev/urandom for its server's password: {error}"),
        })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
