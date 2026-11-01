#![doc = include_str!("../README.md")]

mod sign_in;
pub use sign_in::SIGN_IN_CONFORMANCE_VERSION;

pub mod app_server;
pub mod delivery;
pub mod hooks;
pub mod turns;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cockpit_provider_api::{
    Action, Capabilities, Capability, Conversation, ConversationId, Delivered, DeliveryTarget,
    LaunchCommand, LaunchRequest, Provenance, Provider, ProviderError, ProviderName,
};

/// The Codex version this adapter's conformance tests pin; capabilities are `tested` only on
/// it.
pub const CONFORMANCE_VERSION: &str = "0.161.0";

/// Every flag a launch without the bridge passes; the conformance test finds each in
/// `codex --help` and `codex resume --help`. `--no-daemon` keeps the conversation out of
/// Codex's shared background server, whose hooks run with the environment, launch token
/// included, of whichever launch started it (the failure #184 fixed for Claude Code).
pub const LAUNCH_FLAGS: [&str; 1] = ["--no-daemon"];

/// Every flag the session launcher passes the TUI ([`tui_arguments`]); found in
/// `codex --help` and `codex resume --help`.
pub const TUI_FLAGS: [&str; 1] = ["--remote"];

/// Every flag the session launcher passes the app-server ([`server_arguments`]); found in
/// `codex app-server --help`.
pub const SERVER_FLAGS: [&str; 3] = ["--listen", "--enable", "--config"];

/// The name Cockpit's MCP server (`cockpit mcp`) has in every harness a launch registers it
/// with (#419).
pub const MCP_SERVER_NAME: &str = "cockpit";

/// The feature that runs hooks; a launch enables it so an owner who turned it off still
/// gets the session's events.
pub const HOOKS_FEATURE: &str = "hooks";

/// How long `codex --version` may take at discovery.
const VERSION_DEADLINE: Duration = Duration::from_secs(10);

/// What a launch ships into the sandbox: the session launcher that runs the conversation
/// (`cockpit-codex-session`), the hook program Codex runs on each hook event, and the
/// `cockpit` program both report through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    pub session_program: PathBuf,
    pub hook_program: PathBuf,
    pub cockpit_program: PathBuf,
}

/// The argument that makes `cockpit-codex-hook` and `cockpit-codex-session` print their
/// [`version_line`] and exit.
pub const HOOK_VERSION_FLAG: &str = "--version";

/// What `cockpit-codex-hook --version` prints: its name and the version it was built as.
pub fn hook_version_line() -> String {
    version_line("cockpit-codex-hook")
}

/// What `program --version` prints for one of this crate's programs.
pub fn version_line(program: &str) -> String {
    format!("{program} {}", env!("CARGO_PKG_VERSION"))
}

/// How long each bridge program may take to answer [`HOOK_VERSION_FLAG`] at the daemon's
/// start.
const HOOK_CHECK_DEADLINE: Duration = Duration::from_secs(10);

impl Bridge {
    /// Whether this bridge can run here (#203): its programs are files, `cockpit` has its
    /// execute bit, and the hook and the session launcher run and answer
    /// [`HOOK_VERSION_FLAG`] with this release's [`version_line`]. The reason when not.
    /// Blocks up to 10 seconds per program.
    pub fn check(&self) -> Result<(), String> {
        self.check_within(HOOK_CHECK_DEADLINE)
    }

    /// [`Bridge::check`], giving up on each program after `deadline`.
    pub fn check_within(&self, deadline: Duration) -> Result<(), String> {
        let missing: Vec<String> = [
            &self.session_program,
            &self.hook_program,
            &self.cockpit_program,
        ]
        .into_iter()
        .filter(|program| !program.is_file())
        .map(|program| program.display().to_string())
        .collect();
        if !missing.is_empty() {
            return Err(format!("{} missing", missing.join(" and ")));
        }
        // The hook command is text Codex hands to a shell; a path it cannot spell would name
        // another, missing program.
        if let Some(program) = [&self.hook_program, &self.cockpit_program]
            .into_iter()
            .find(|program| program.to_str().is_none())
        {
            return Err(format!(
                "{} is not valid UTF-8, so a hook command cannot name it",
                program.display()
            ));
        }
        if !is_executable(&self.cockpit_program) {
            return Err(format!(
                "{} has no execute bit, so the hook cannot run it",
                self.cockpit_program.display()
            ));
        }
        check_version(&self.hook_program, "cockpit-codex-hook", deadline)?;
        check_version(&self.session_program, "cockpit-codex-session", deadline)
    }

    /// The command Codex runs for each hook; Codex runs it through a shell.
    pub fn hook_command(&self) -> String {
        format!(
            "{} {}",
            shell_quoted(&self.hook_program),
            shell_quoted(&self.cockpit_program)
        )
    }
}

/// The Codex adapter.
#[derive(Debug)]
pub struct Codex {
    name: ProviderName,
    program: PathBuf,
    version: Option<String>,
    /// Without it, launches run the TUI alone, subscribe no hooks, and only `resume` is
    /// reported.
    bridge: Option<Bridge>,
}

impl Codex {
    /// The adapter for `program`, running `version` when known.
    pub fn new(program: PathBuf, version: Option<String>, bridge: Option<Bridge>) -> Codex {
        Codex {
            name: ProviderName::new("codex").expect("`codex` is a valid provider name"),
            program,
            version,
            bridge,
        }
    }

    /// The adapter for `program`, with the version it prints. Blocks up to 10 seconds.
    pub fn discover(program: &Path, bridge: Option<Bridge>) -> Result<Codex, DiscoveryError> {
        Codex::discover_within(program, bridge, VERSION_DEADLINE)
    }

    /// [`Codex::discover`], giving up on `--version` after `deadline`.
    pub fn discover_within(
        program: &Path,
        bridge: Option<Bridge>,
        deadline: Duration,
    ) -> Result<Codex, DiscoveryError> {
        let failed = |reason: String| DiscoveryError {
            program: program.to_path_buf(),
            reason,
        };
        let output = run_with_deadline(program, "--version", deadline).map_err(failed)?;
        // `codex-cli 0.161.0`, or a pre-release such as `0.162.0-alpha.3`, which is only
        // `discovered`; warnings go to standard error.
        let version = match output.split_whitespace().collect::<Vec<_>>()[..] {
            ["codex-cli", version, ..]
                if is_dotted_version(version.split('-').next().unwrap_or_default()) =>
            {
                version
            }
            _ => {
                return Err(failed(format!(
                    "`--version` printed {output:?}, not a version"
                )));
            }
        };
        Ok(Codex::new(
            program.to_path_buf(),
            Some(version.to_owned()),
            bridge,
        ))
    }

    /// The first executable `codex` in the directories of `path` (a `PATH` value).
    pub fn find_on_path(path: &OsStr) -> Option<PathBuf> {
        std::env::split_paths(path)
            .map(|dir| dir.join("codex"))
            .find(|candidate| {
                candidate.metadata().is_ok_and(|metadata| {
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                })
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

impl Provider for Codex {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn sign_in(&self) -> cockpit_provider_api::SignIn {
        self.sign_in_within(Duration::from_secs(10))
    }

    fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Resume; with the bridge, also events, submit, cancel and outcome, which its session
    /// launcher and app-server provide (#407). Not effort, compact, billing, commands,
    /// evidence, remote control or screen reading: untested.
    fn capabilities(&self) -> Capabilities {
        let provenance = self.provenance();
        let bridged = if self.bridge.is_some() {
            BRIDGED_CAPABILITIES.as_slice()
        } else {
            &[]
        };
        [Capability::Resume]
            .into_iter()
            .chain(bridged.iter().copied())
            .fold(Capabilities::none(), |capabilities, capability| {
                capabilities.with(capability, provenance.clone())
            })
    }

    /// With the bridge, the session launcher: `cockpit-codex-session --cockpit <cockpit>
    /// --codex <codex> --socket <session dir>/codex.sock --config <hook>… --config <mcp>…
    /// [--resume <thread id>] [--message <first message>]`. It runs Codex's app-server with the hooks and the
    /// TUI against it ([`server_arguments`], [`tui_arguments`]). Without the bridge, the TUI
    /// alone: `codex [resume] --no-daemon [-- [<thread id>] [<first message>]]`. A new
    /// conversation's thread id is Codex's own and unknown at launch: the first report names
    /// it.
    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        request.require(&self.capabilities())?;
        let resumed = match request.conversation {
            Conversation::New => None,
            Conversation::Resume(id) => Some(id),
        };
        let (program, arguments) = match &self.bridge {
            Some(bridge) => (
                bridge.session_program.clone(),
                session_arguments(&self.program, bridge, request, resumed),
            ),
            None => (
                self.program.clone(),
                direct_arguments(resumed, request.first_message),
            ),
        };
        Ok(LaunchCommand {
            program,
            arguments,
            environment: Vec::new(),
            conversation: resumed.cloned(),
        })
    }

    /// Submit and cancel through the session's app-server ([`delivery`]); a message joins the
    /// turn under way. Blocks up to a few seconds per call.
    fn deliver(
        &self,
        action: &Action<'_>,
        target: &DeliveryTarget<'_>,
        conversation: &ConversationId,
    ) -> Result<Delivered, ProviderError> {
        self.capabilities().require(action.capability())?;
        delivery::deliver(
            &delivery::socket_path(target.session_dir),
            target.sockets,
            action,
            conversation,
        )
    }
}

/// What the bridge adds to `resume`.
const BRIDGED_CAPABILITIES: [Capability; 4] = [
    Capability::Events,
    Capability::Submit,
    Capability::Cancel,
    Capability::Outcome,
];

/// The session launcher's arguments for `request`.
fn session_arguments(
    codex: &Path,
    bridge: &Bridge,
    request: &LaunchRequest<'_>,
    resumed: Option<&ConversationId>,
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = vec![
        "--cockpit".into(),
        bridge.cockpit_program.clone().into(),
        "--codex".into(),
        codex.into(),
        "--socket".into(),
        delivery::socket_path(request.session_dir).into(),
    ];
    for value in hooks::subscription(&bridge.hook_command())
        .into_iter()
        .chain(mcp_configs(&bridge.cockpit_program))
    {
        arguments.extend(["--config".into(), value.into()]);
    }
    if let Some(id) = resumed {
        arguments.extend(["--resume".into(), id.as_str().into()]);
    }
    if let Some(message) = request.first_message {
        arguments.extend(["--message".into(), message.into()]);
    }
    arguments
}

/// The `--config` values that register `cockpit mcp` as the [`MCP_SERVER_NAME`] MCP server
/// (#419). [`Bridge::check`] refuses a `cockpit` path that is not UTF-8.
pub fn mcp_configs(cockpit: &Path) -> Vec<String> {
    vec![
        format!(
            "mcp_servers.{MCP_SERVER_NAME}.command={}",
            hooks::toml_string(&cockpit.to_string_lossy())
        ),
        format!("mcp_servers.{MCP_SERVER_NAME}.args=[\"mcp\"]"),
        format!(
            "mcp_servers.{MCP_SERVER_NAME}.env_vars=[{}]",
            MCP_ENVIRONMENT
                .iter()
                .map(|variable| format!("\"{variable}\""))
                .collect::<Vec<_>>()
                .join(",")
        ),
    ]
}

/// What `cockpit mcp` needs from the launch's environment to reach its session: Codex hands
/// a stdio MCP server only `HOME`, `PATH` and a few locale variables unless a server names
/// more (`DEFAULT_ENV_VARS`, codex-rs/rmcp-client at rust-v0.161.0).
const MCP_ENVIRONMENT: [&str; 7] = [
    "COCKPIT_LAUNCH_TOKEN",
    "COCKPIT_AGENT_SOCKET",
    "COCKPIT_INSTRUCTIONS",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
];

/// `[resume] --no-daemon [-- [<thread id>] [<first message>]]`: every word after `--` is
/// positional, so neither can be read as a flag.
fn direct_arguments(
    resumed: Option<&ConversationId>,
    first_message: Option<&str>,
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = Vec::new();
    if resumed.is_some() {
        arguments.push("resume".into());
    }
    arguments.push("--no-daemon".into());
    arguments.extend(positional(resumed, first_message));
    arguments
}

fn positional(resumed: Option<&ConversationId>, first_message: Option<&str>) -> Vec<OsString> {
    let words: Vec<OsString> = resumed
        .map(|id| OsString::from(id.as_str()))
        .into_iter()
        .chain(first_message.map(OsString::from))
        .collect();
    if words.is_empty() {
        return words;
    }
    std::iter::once(OsString::from("--")).chain(words).collect()
}

/// The app-server's arguments: `app-server --listen unix://<socket> --enable hooks --config
/// <value>…`. The hooks are the server's: it runs them, with the launch's environment.
pub fn server_arguments(socket: &Path, configs: &[String]) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = vec![
        "app-server".into(),
        "--listen".into(),
        unix_address(socket),
        "--enable".into(),
        HOOKS_FEATURE.into(),
    ];
    for value in configs {
        arguments.extend(["--config".into(), value.into()]);
    }
    arguments
}

/// The TUI's arguments against the app-server on `socket`: `[resume] --remote unix://<socket>
/// [-- [<thread id>] [<first message>]]`. A remote TUI's conversation runs in the
/// app-server's working directory.
pub fn tui_arguments(
    socket: &Path,
    resumed: Option<&ConversationId>,
    first_message: Option<&str>,
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = Vec::new();
    if resumed.is_some() {
        arguments.push("resume".into());
    }
    arguments.extend(["--remote".into(), unix_address(socket)]);
    arguments.extend(positional(resumed, first_message));
    arguments
}

fn unix_address(socket: &Path) -> OsString {
    let mut address = OsString::from("unix://");
    address.push(socket);
    address
}

/// Codex's program could not be used.
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

/// Whether `program` answers [`HOOK_VERSION_FLAG`] as this release's `name`.
fn check_version(program: &Path, name: &str, deadline: Duration) -> Result<(), String> {
    let answer = run_with_deadline(program, HOOK_VERSION_FLAG, deadline)
        .map_err(|reason| format!("{}: {reason}", program.display()))?;
    if !answer.starts_with(&format!("{name} ")) {
        return Err(format!(
            "{}: `{HOOK_VERSION_FLAG}` printed {answer:?}, not {name}",
            program.display()
        ));
    }
    let expected = version_line(name);
    if answer != expected {
        return Err(format!(
            "{}: it is {answer:?}, not this release's {expected:?}",
            program.display()
        ));
    }
    Ok(())
}

fn is_executable(program: &Path) -> bool {
    std::fs::metadata(program).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

fn is_dotted_version(word: &str) -> bool {
    let parts: Vec<&str> = word.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// `path` as one POSIX shell word.
fn shell_quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// The trimmed standard output of `program argument`, or why it failed. Kills it at the
/// deadline, and stops waiting for its output there too: a wrapper may leave a background
/// process holding its pipes. A copy of `cockpit-claude`'s: adapters depend on
/// `cockpit-provider-api` only.
fn run_with_deadline(program: &Path, argument: &str, deadline: Duration) -> Result<String, String> {
    let mut child = Command::new(program)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run it: {error}"))?;
    let gave_up = format!(
        "`{argument}` did not answer within {} ms",
        deadline.as_millis()
    );
    let ends_at = Instant::now() + deadline;
    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot wait for it: {error}"))?
        {
            break status;
        }
        if Instant::now() > ends_at {
            let _ = child.kill();
            let _ = child.wait();
            return Err(gave_up);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let remaining = || ends_at.saturating_duration_since(Instant::now());
    let stdout = stdout
        .recv_timeout(remaining())
        .map_err(|_| gave_up.clone())?;
    let stderr = stderr
        .recv_timeout(remaining())
        .map_err(|_| gave_up.clone())?;
    if !status.success() {
        return Err(format!("`{argument}` failed ({status}): {}", stderr.trim()));
    }
    Ok(stdout.trim().to_owned())
}

/// The whole of `pipe` as text, read on a thread of its own so a full pipe never stalls
/// the child.
fn read_in_background(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}
