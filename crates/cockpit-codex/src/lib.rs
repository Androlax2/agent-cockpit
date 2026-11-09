#![doc = include_str!("../README.md")]

mod sign_in;
pub use sign_in::SIGN_IN_CONFORMANCE_VERSION;

pub mod app_server;
pub mod delivery;
pub mod hooks;
pub mod needs;
mod probe;
mod quota;
pub mod reports;
pub mod rollout;
pub mod turns;

pub use probe::probe;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cockpit_provider_api::{
    Action, Capabilities, Capability, Conversation, ConversationId, Credentials, Delivered,
    DeliveryTarget, InvalidValue, LaunchCommand, LaunchRequest, Needs, Probe, Provenance, Provider,
    ProviderError, ProviderName, QuotaReading,
};

/// The Codex versions the adapter's conformance tests pass against (`tested-versions.txt`,
/// #732); capabilities are `tested` on each of them.
pub const TESTED_VERSIONS: &str = include_str!("../tested-versions.txt");

/// The Codex version the recorded fixtures and the native evidence come from, kept on
/// [`TESTED_VERSIONS`]. Not the version capabilities are tested on: any listed version is.
pub const CONFORMANCE_VERSION: &str = "0.166.0";

/// Every flag a launch without the bridge passes; the conformance test finds each in
/// `codex --help` and `codex resume --help`. `--no-daemon` keeps the conversation out of
/// Codex's shared background server, whose hooks run with the environment, launch token
/// included, of whichever launch started it (the failure #184 fixed for Claude Code).
pub const LAUNCH_FLAGS: [&str; 1] = ["--no-daemon"];

/// Every flag the session launcher passes the TUI ([`tui_arguments`]); found in
/// `codex --help` and `codex resume --help`. A launch without the bridge passes
/// [`CONFIG_FLAG`] too: every launch turns the update screen off ([`UPDATE_CHECK_CONFIG`]).
pub const TUI_FLAGS: [&str; 2] = ["--remote", CONFIG_FLAG];

/// Every flag the session launcher passes the app-server ([`server_arguments`]); found in
/// `codex app-server --help`.
pub const SERVER_FLAGS: [&str; 3] = ["--listen", "--enable", "--config"];

/// The name Cockpit's MCP server (`cockpit mcp`) has in every harness a launch registers it
/// with (#419).
pub const MCP_SERVER_NAME: &str = "torlek";

/// The feature that runs hooks; a launch enables it so an owner who turned it off still
/// gets the session's events.
pub const HOOKS_FEATURE: &str = "hooks";

/// The flag a launch sets a configuration value with, on the TUI (`codex`, `codex resume`)
/// and the app-server; the probe finds it where a compaction threshold is passed (#783).
pub const CONFIG_FLAG: &str = "--config";

/// Codex's own compaction threshold: a whole percent of the usable context window, after
/// which a turn's end compacts the history; `0` turns it off (#783). A launch with a
/// [`LaunchRequest::compaction_threshold`] passes it as `--config <key>=<percent>`.
pub const COMPACTION_THRESHOLD_KEY: &str = "model_post_turn_compact_threshold_percent";

/// The value the probe gives [`COMPACTION_THRESHOLD_KEY`]: a Codex that knows the key refuses
/// it as not a number, naming the key; one that does not know it says nothing.
pub const COMPACTION_THRESHOLD_PROBE_VALUE: &str = "torlek-probe";

/// The TUI's `--config` value that skips Codex's "Update available" screen, which would hold a
/// launch until the owner answers it in the terminal (#1281). Measured on 0.166.0, for the TUI
/// alone and against an app-server: with it, the TUI goes straight to its own screen.
pub const UPDATE_CHECK_CONFIG: &str = "check_for_update_on_startup=false";

/// The sandbox every session Torlek starts runs its commands in, over the owner's own
/// `sandbox_mode` (owner's decision on #1281, 2026-11-09): the workspace writable, and the
/// network open, without which Codex's sandbox refuses every `connect`, Torlek's agent socket
/// included (`EPERM`, measured on 0.166.0; `writable_roots` does not change it, and no narrower
/// setting lets one Unix socket through). Torlek's own sandbox, its proxy and its grants stay the
/// boundary. The owner's approval policy and `writable_roots` are left alone: a `--config` array
/// would replace the owner's list, not add to it. Codex sessions started by hand are untouched.
pub const SANDBOX_CONFIGS: [&str; 2] = [
    "sandbox_mode=\"workspace-write\"",
    "sandbox_workspace_write.network_access=true",
];

/// Codex's model setting; a launch with a [`LaunchRequest::model`] sets it for the session.
pub const MODEL_KEY: &str = "model";

/// Codex's reasoning effort setting; a launch with a [`LaunchRequest::effort`] sets it for the
/// session.
pub const EFFORT_KEY: &str = "model_reasoning_effort";

/// Longest model Torlek hands over: an id such as `gpt-6.1-terra`.
const MAX_MODEL_BYTES: usize = 200;

/// Longest effort Torlek hands over; Codex's own levels are single words (`low` to `ultra`).
const MAX_EFFORT_BYTES: usize = 64;

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
    version_line("torlek-codex-hook")
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
        check_version(&self.hook_program, "torlek-codex-hook", deadline)?;
        check_version(&self.session_program, "torlek-codex-session", deadline)
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
    /// What the runtime probe found in this version, when it ran (#732).
    probe: Option<Probe>,
}

impl Codex {
    /// The adapter for `program`, running `version` when known.
    pub fn new(program: PathBuf, version: Option<String>, bridge: Option<Bridge>) -> Codex {
        Codex {
            name: ProviderName::new("codex").expect("`codex` is a valid provider name"),
            program,
            version,
            bridge,
            probe: None,
        }
    }

    /// What [`probe()`](fn@probe) found in the running version. On a tested version, or for another
    /// version than the one running, the probe changes nothing.
    pub fn with_probe(mut self, probe: Probe) -> Codex {
        self.probe = Some(probe);
        self
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

    /// `tested` on a version in [`TESTED_VERSIONS`]; on any other, `probed` when the probe
    /// found every requirement, `discovered` without a clean probe.
    pub fn provenance(&self) -> Provenance {
        Provenance::for_adapter(
            self.version.as_deref(),
            TESTED_VERSIONS,
            self.probe.as_ref(),
        )
    }
}

impl Provider for Codex {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    /// From Codex's own quota sign-in under `~/.local/share/codex-quota`, never the sessions'.
    fn quota(&self, home: &Path) -> Result<QuotaReading, ProviderError> {
        self.quota_within(home, quota::QUOTA_DEADLINE)
    }

    fn login(&self) -> Result<Option<cockpit_provider_api::Login>, InvalidValue> {
        needs::login(&self.program).map(Some)
    }

    fn sign_in(&self) -> cockpit_provider_api::SignIn {
        self.sign_in_within(Duration::from_secs(10))
    }

    fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Resume, model, effort, compaction threshold and quota; with the bridge, also events,
    /// submit, cancel and outcome, which its session launcher and app-server provide (#407).
    /// Not compact, billing, commands, evidence, remote control or screen reading: untested.
    fn capabilities(&self) -> Capabilities {
        let bridged = if self.bridge.is_some() {
            BRIDGED_CAPABILITIES.as_slice()
        } else {
            &[]
        };
        Capabilities::from_probe(
            self.version.as_deref(),
            TESTED_VERSIONS,
            self.probe.as_ref(),
            [
                Capability::Resume,
                Capability::Model,
                Capability::Effort,
                Capability::CompactionThreshold,
                Capability::Quota,
            ]
            .into_iter()
            .chain(bridged.iter().copied()),
        )
    }

    fn probe(&self) -> Option<&Probe> {
        self.probe.as_ref()
    }

    fn needs(&self) -> Result<Needs, InvalidValue> {
        needs::needs()
    }

    fn credentials(&self) -> Result<Credentials, InvalidValue> {
        needs::credentials()
    }

    /// With the bridge, the session launcher: `cockpit-codex-session --cockpit <cockpit>
    /// --codex <codex> --socket <session dir>/codex.sock --config <hook>… --config <mcp>…
    /// [--config <threshold>] --config <session>… --tui-config <session>… --tui-config
    /// <update check> [--resume <thread id>] [--message <first message>]`. It runs Codex's
    /// app-server with the hooks and the TUI against it ([`server_arguments`],
    /// [`tui_arguments`]). Without the bridge, the TUI alone: `codex [resume] --no-daemon
    /// [--config <threshold>] --config <session>… --config <update check> [-- [<thread id>]
    /// [<first message>]]`. A chosen compaction threshold is one more `--config` value
    /// ([`COMPACTION_THRESHOLD_KEY`]), for the app-server with the bridge and the TUI without.
    /// The session's own settings ([`session_configs`]: its sandbox, a chosen model and
    /// effort) go to both the app-server and the TUI, so neither falls back to the owner's
    /// `~/.codex/config.toml` for them; the TUI's update screen is always off
    /// ([`UPDATE_CHECK_CONFIG`]).
    /// A new conversation's thread id is Codex's own and unknown at launch: the first report
    /// names it.
    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        request.require(&self.capabilities())?;
        let resumed = match request.conversation {
            Conversation::New => None,
            Conversation::Resume(id) => Some(id),
        };
        let threshold_config = request
            .compaction_threshold
            .map(compaction_threshold_config)
            .transpose()?;
        let session = session_configs(request)?;
        let (program, arguments) = match &self.bridge {
            Some(bridge) => (
                bridge.session_program.clone(),
                session_arguments(
                    &self.program,
                    bridge,
                    request,
                    resumed,
                    threshold_config,
                    &session,
                ),
            ),
            None => (
                self.program.clone(),
                direct_arguments(resumed, request.first_message, threshold_config, &session),
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

/// The `--config` value that sets Codex's compaction threshold to `threshold`, a whole
/// percent from 0 to 100 in Codex's own words; Codex itself would refuse anything else, so
/// the refusal comes before the launch.
fn compaction_threshold_config(threshold: &str) -> Result<String, ProviderError> {
    // The value lands in a TOML document, which refuses a leading zero on an integer.
    let is_plain_integer = threshold.len() <= 3
        && threshold.bytes().all(|byte| byte.is_ascii_digit())
        && (threshold == "0" || !threshold.starts_with('0'));
    let percent = is_plain_integer
        .then(|| threshold.parse::<u8>().ok())
        .flatten()
        .filter(|percent| *percent <= 100);
    match percent {
        Some(_) => Ok(format!("{COMPACTION_THRESHOLD_KEY}={threshold}")),
        None => Err(ProviderError {
            reason: format!(
                "compaction threshold {threshold:?} is not a Codex \
                 `{COMPACTION_THRESHOLD_KEY}`: a whole percent from 0 to 100, written plainly"
            ),
        }),
    }
}

/// The `--config` values of the session's own settings, for both the app-server and the TUI:
/// its sandbox ([`SANDBOX_CONFIGS`]), then the chosen model and effort, as TOML strings. Codex
/// takes any model id and refuses one the account cannot use itself, so only the shapes are
/// checked here.
pub fn session_configs(request: &LaunchRequest<'_>) -> Result<Vec<String>, ProviderError> {
    let mut configs: Vec<String> = SANDBOX_CONFIGS.map(str::to_owned).to_vec();
    if let Some(model) = request.model {
        check_model(model)?;
        configs.push(format!("{MODEL_KEY}={}", hooks::toml_string(model)));
    }
    if let Some(effort) = request.effort {
        check_effort(effort)?;
        configs.push(format!("{EFFORT_KEY}={}", hooks::toml_string(effort)));
    }
    Ok(configs)
}

/// One word that cannot read as a flag.
fn check_model(model: &str) -> Result<(), ProviderError> {
    let is_plain = model.len() <= MAX_MODEL_BYTES
        && model.starts_with(|first: char| first.is_ascii_alphanumeric())
        && model
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._:/@".contains(character));
    if is_plain {
        return Ok(());
    }
    Err(ProviderError {
        reason: format!(
            "model {model:?} is not a Codex model name: 1 to {MAX_MODEL_BYTES} ASCII letters, \
             digits and `-._:/@`, starting with a letter or digit"
        ),
    })
}

/// One plain word, such as `high`.
fn check_effort(effort: &str) -> Result<(), ProviderError> {
    let is_plain = effort.len() <= MAX_EFFORT_BYTES
        && effort.starts_with(|first: char| first.is_ascii_alphanumeric())
        && effort
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_".contains(character));
    if is_plain {
        return Ok(());
    }
    Err(ProviderError {
        reason: format!(
            "effort {effort:?} is not a Codex reasoning effort: 1 to {MAX_EFFORT_BYTES} ASCII \
             letters, digits and `-_`, starting with a letter or digit"
        ),
    })
}

/// The session launcher's arguments for `request`; `threshold_config` is the compaction
/// threshold's `--config` value, after the hooks and the MCP server, and `session` the
/// session's own settings, last for the app-server and first for the TUI.
fn session_arguments(
    codex: &Path,
    bridge: &Bridge,
    request: &LaunchRequest<'_>,
    resumed: Option<&ConversationId>,
    threshold_config: Option<String>,
    session: &[String],
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = vec![
        "--torlek".into(),
        bridge.cockpit_program.clone().into(),
        "--codex".into(),
        codex.into(),
        "--socket".into(),
        delivery::socket_path(request.session_dir).into(),
    ];
    for value in hooks::subscription(&bridge.hook_command())
        .into_iter()
        .chain(mcp_configs(&bridge.cockpit_program))
        .chain(threshold_config)
        .chain(session.iter().cloned())
    {
        arguments.extend([CONFIG_FLAG.into(), value.into()]);
    }
    for value in tui_configs(session) {
        arguments.extend([TUI_CONFIG_OPTION.into(), value.into()]);
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
const MCP_ENVIRONMENT: [&str; 8] = [
    "TORLEK_LAUNCH_TOKEN",
    "TORLEK_AGENT_SOCKET",
    "TORLEK_PROFILE",
    "TORLEK_INSTRUCTIONS",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
];

/// The session launcher's option whose values it passes the TUI as `--config` values.
pub const TUI_CONFIG_OPTION: &str = "--tui-config";

/// The TUI's `--config` values: the session's own settings, then the update check.
fn tui_configs(session: &[String]) -> impl Iterator<Item = String> + '_ {
    session
        .iter()
        .cloned()
        .chain([UPDATE_CHECK_CONFIG.to_owned()])
}

/// `[resume] --no-daemon [--config <threshold>] --config <session>… --config <update check>
/// [-- [<thread id>] [<first message>]]`: every word after `--` is positional, so neither can
/// be read as a flag.
fn direct_arguments(
    resumed: Option<&ConversationId>,
    first_message: Option<&str>,
    threshold_config: Option<String>,
    session: &[String],
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = Vec::new();
    if resumed.is_some() {
        arguments.push("resume".into());
    }
    arguments.push("--no-daemon".into());
    for value in threshold_config.into_iter().chain(tui_configs(session)) {
        arguments.extend([CONFIG_FLAG.into(), value.into()]);
    }
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
        arguments.extend([CONFIG_FLAG.into(), value.into()]);
    }
    arguments
}

/// The TUI's arguments against the app-server on `socket`: `[resume] --remote unix://<socket>
/// [--config <value>]… [-- [<thread id>] [<first message>]]`. A remote TUI's conversation
/// runs in the app-server's working directory.
pub fn tui_arguments(
    socket: &Path,
    configs: &[String],
    resumed: Option<&ConversationId>,
    first_message: Option<&str>,
) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = Vec::new();
    if resumed.is_some() {
        arguments.push("resume".into());
    }
    arguments.extend(["--remote".into(), unix_address(socket)]);
    for value in configs {
        arguments.extend([CONFIG_FLAG.into(), value.into()]);
    }
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
