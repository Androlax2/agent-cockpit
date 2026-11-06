#![doc = include_str!("../README.md")]

mod sign_in;

pub mod hooks;
pub mod needs;
mod screen;
pub mod trust;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use cockpit_provider_api::{
    Action, Activity, Capabilities, Capability, Conversation, ConversationId, Credentials,
    InvalidValue, Key, LaunchCommand, LaunchRequest, Needs, Observation, Provenance, Provider,
    ProviderError, ProviderName, TerminalInput, TrustRequest, WorkspaceTrust,
};

/// The Claude Code version this adapter's conformance tests pin; capabilities are `tested`
/// only on it.
pub const CONFORMANCE_VERSION: &str = "2.1.316";

/// Longest `--model` value: an alias (`opus`), a model id (`claude-opus-5-5[1m]`) or a cloud
/// provider's id or ARN.
const MAX_MODEL_BYTES: usize = 200;

/// Longest `--effort` value Torlek hands over; Claude Code's own levels are single words.
const MAX_EFFORT_BYTES: usize = 64;

/// How long `claude --help` may take when a start chooses an effort.
const HELP_DEADLINE: Duration = Duration::from_secs(10);

/// Every flag a launch may pass; the conformance test finds each in `claude --help`.
pub const LAUNCH_FLAGS: [&str; 8] = [
    "--remote-control",
    "--session-id",
    "--resume",
    "--settings",
    "--mcp-config",
    "--effort",
    "--model",
    "--add-dir",
];

/// The name Cockpit's MCP server (`cockpit mcp`) has in every harness a launch registers it
/// with (#419).
pub const MCP_SERVER_NAME: &str = "torlek";

/// The environment every launch sets; the conformance test finds each name in the binary.
/// `CLAUDE_CODE_DISABLE_AGENT_VIEW` turns off Claude Code's background daemon:
/// `/background` and the exit menu's "Move to background" would continue the conversation
/// in the daemon's process, whose environment, launch token included, is whichever session
/// started the daemon, so its hooks would report as another Cockpit session (#184).
/// `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS` gives a `SessionEnd` hook the agent channel's
/// 10 s report deadline instead of the 1.5 s after which Claude Code kills it and runs the
/// next `SessionStart` anyway (measured on 2.1.316): the end a `/clear` announces must reach
/// the daemon before the new conversation reports (#219).
pub const LAUNCH_ENVIRONMENT: [(&str, &str); 2] = [
    ("CLAUDE_CODE_DISABLE_AGENT_VIEW", "1"),
    ("CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS", "10000"),
];

/// How long `claude --version` may take at discovery.
const VERSION_DEADLINE: Duration = Duration::from_secs(10);

/// What a launch ships into the sandbox: the hook program Claude Code runs on each hook
/// event, and the `cockpit` program it reports through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    pub hook_program: PathBuf,
    pub cockpit_program: PathBuf,
}

/// The argument that makes `cockpit-claude-hook` print [`hook_version_line`] and exit.
pub const HOOK_VERSION_FLAG: &str = "--version";

/// What `cockpit-claude-hook --version` prints: its name and the version it was built as.
pub fn hook_version_line() -> String {
    format!("torlek-claude-hook {}", env!("CARGO_PKG_VERSION"))
}

/// How long the hook bridge may take to answer [`HOOK_VERSION_FLAG`] at the daemon's start.
const HOOK_CHECK_DEADLINE: Duration = Duration::from_secs(10);

impl Bridge {
    /// Whether this bridge can run here (#203): both programs are files, `cockpit` has its
    /// execute bit, and the hook runs and answers [`HOOK_VERSION_FLAG`] with this release's
    /// [`hook_version_line`], so a wrong architecture, a lost execute bit or a stale hook shows
    /// at the daemon's start, not as missing events. The reason when not. Blocks up to 10
    /// seconds.
    pub fn check(&self) -> Result<(), String> {
        self.check_within(HOOK_CHECK_DEADLINE)
    }

    /// [`Bridge::check`], giving up on the hook after `deadline`.
    pub fn check_within(&self, deadline: Duration) -> Result<(), String> {
        let missing: Vec<String> = [&self.hook_program, &self.cockpit_program]
            .into_iter()
            .filter(|program| !program.is_file())
            .map(|program| program.display().to_string())
            .collect();
        if !missing.is_empty() {
            return Err(format!("{} missing", missing.join(" and ")));
        }
        if !is_executable(&self.cockpit_program) {
            return Err(format!(
                "{} has no execute bit, so the hook cannot run it",
                self.cockpit_program.display()
            ));
        }
        let answer = run_with_deadline(&self.hook_program, HOOK_VERSION_FLAG, deadline)
            .map_err(|reason| format!("{}: {reason}", self.hook_program.display()))?;
        if !answer.starts_with("torlek-claude-hook ") {
            return Err(format!(
                "{}: `{HOOK_VERSION_FLAG}` printed {answer:?}, not a hook bridge",
                self.hook_program.display()
            ));
        }
        let expected = hook_version_line();
        if answer != expected {
            return Err(format!(
                "{}: the hook bridge is {answer:?}, not this release's {expected:?}",
                self.hook_program.display()
            ));
        }
        Ok(())
    }
}

fn is_executable(program: &Path) -> bool {
    std::fs::metadata(program).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

/// The Claude Code adapter.
#[derive(Debug)]
pub struct ClaudeCode {
    name: ProviderName,
    program: PathBuf,
    version: Option<String>,
    /// Without it, launches subscribe no hooks and `events` is not reported.
    bridge: Option<Bridge>,
    /// The directory each launch adds with `--add-dir`, whose [`instruction_skills_dir`]
    /// holds Cockpit's instruction bundle as a skill (#419).
    instruction_skills: Option<PathBuf>,
    /// What `claude --help` said, read when a start first chooses an effort and read again
    /// when the program file changes, since a Claude Code update replaces it (#743).
    help: Mutex<Option<HelpRead>>,
}

/// The effort levels one build of `claude` lists in its `--help`.
#[derive(Debug, Clone)]
struct HelpRead {
    /// The program file's modification time and size when read: another build has another.
    stamp: Option<(std::time::SystemTime, u64)>,
    /// The version that build printed, for the refusal to name.
    version: Option<String>,
    /// Empty when the help could not be read or lists none (or words its option differently):
    /// the choice then passes through and Claude Code answers for itself. Kept as read, so a
    /// slow or broken `claude` is asked once per build, not at every start.
    levels: Vec<String>,
}

/// Where Claude Code looks for skills in a directory added with `--add-dir`: the bundle is
/// written there so the `torlek` skill sits at `<root>/.claude/skills/torlek/SKILL.md`.
pub fn instruction_skills_dir(root: &Path) -> PathBuf {
    root.join(".claude").join("skills")
}

impl ClaudeCode {
    /// The adapter for `program`, running `version` when known.
    pub fn new(program: PathBuf, version: Option<String>, bridge: Option<Bridge>) -> ClaudeCode {
        ClaudeCode {
            name: ProviderName::new("claude").expect("`claude` is a valid provider name"),
            program,
            version,
            bridge,
            instruction_skills: None,
            help: Mutex::new(None),
        }
    }

    /// Every launch adds `root` with `--add-dir`, so Claude Code lists the skills written
    /// under [`instruction_skills_dir`]`(root)`. The caller writes them before any launch.
    pub fn with_instruction_skills(mut self, root: PathBuf) -> ClaudeCode {
        self.instruction_skills = Some(root);
        self
    }

    /// The adapter for `program`, with the version it prints. Blocks up to 10 seconds.
    pub fn discover(program: &Path, bridge: Option<Bridge>) -> Result<ClaudeCode, DiscoveryError> {
        ClaudeCode::discover_within(program, bridge, VERSION_DEADLINE)
    }

    /// [`ClaudeCode::discover`], giving up on `--version` after `deadline`.
    pub fn discover_within(
        program: &Path,
        bridge: Option<Bridge>,
        deadline: Duration,
    ) -> Result<ClaudeCode, DiscoveryError> {
        let failed = |reason: String| DiscoveryError {
            program: program.to_path_buf(),
            reason,
        };
        let output = run_with_deadline(program, "--version", deadline).map_err(failed)?;
        let version = output
            .split_whitespace()
            .next()
            .filter(|word| is_dotted_version(word))
            .ok_or_else(|| failed(format!("`--version` printed {output:?}, not a version")))?;
        Ok(ClaudeCode::new(
            program.to_path_buf(),
            Some(version.to_owned()),
            bridge,
        ))
    }

    /// The first executable `claude` in the directories of `path` (a `PATH` value).
    pub fn find_on_path(path: &OsStr) -> Option<PathBuf> {
        std::env::split_paths(path)
            .map(|dir| dir.join("claude"))
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

    /// The effort levels the installed `claude --help` lists; none when it could not be read
    /// or lists none. Read once per build of the program, so a Claude Code update that adds
    /// or removes a level shows without restarting the daemon.
    pub fn effort_levels(&self) -> Vec<String> {
        self.help_read().levels
    }

    fn help_read(&self) -> HelpRead {
        let mut kept = self
            .help
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stamp = std::fs::metadata(&self.program)
            .and_then(|metadata| Ok((metadata.modified()?, metadata.len())))
            .ok();
        if let Some(read) = kept.as_ref().filter(|read| read.stamp == stamp) {
            return read.clone();
        }
        let version = run_with_deadline(&self.program, "--version", HELP_DEADLINE)
            .ok()
            .and_then(|output| {
                output
                    .split_whitespace()
                    .next()
                    .filter(|word| is_dotted_version(word))
                    .map(str::to_owned)
            })
            .or_else(|| self.version.clone());
        let levels = run_with_deadline(&self.program, "--help", HELP_DEADLINE)
            .ok()
            .and_then(|help| effort_levels_in(&help))
            .unwrap_or_default();
        let read = HelpRead {
            stamp,
            version,
            levels,
        };
        *kept = Some(read.clone());
        read
    }

    /// `--settings` JSON that runs `bridge` on every hook event it normalizes, and, after
    /// each [`hooks::INBOX_EVENTS`] event, its inbox waiter in the background (#703).
    fn hook_settings(bridge: &Bridge) -> String {
        let command = format!(
            "{} {}",
            shell_quoted(&bridge.hook_program),
            shell_quoted(&bridge.cockpit_program)
        );
        let report = serde_json::json!({ "type": "command", "command": command });
        let inbox = serde_json::json!({
            "type": "command",
            "command": format!("{command} {}", hooks::INBOX_FLAG),
            "asyncRewake": true,
            "timeout": hooks::INBOX_TIMEOUT_SECONDS,
        });
        let events: serde_json::Map<String, serde_json::Value> = hooks::HOOK_EVENTS
            .iter()
            .map(|event| {
                let mut commands = vec![report.clone()];
                if hooks::INBOX_EVENTS.contains(event) {
                    commands.push(inbox.clone());
                }
                (
                    (*event).to_owned(),
                    serde_json::json!([{ "hooks": commands }]),
                )
            })
            .collect();
        serde_json::json!({ "hooks": events }).to_string()
    }

    /// `--mcp-config` JSON that registers `cockpit mcp` (#419).
    fn mcp_config(bridge: &Bridge) -> String {
        serde_json::json!({
            "mcpServers": {
                MCP_SERVER_NAME: {
                    "type": "stdio",
                    "command": bridge.cockpit_program.to_string_lossy(),
                    "args": ["mcp"],
                }
            }
        })
        .to_string()
    }
}

impl Provider for ClaudeCode {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn sign_in(&self) -> cockpit_provider_api::SignIn {
        self.sign_in_within(Duration::from_secs(10))
    }

    fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    fn capabilities(&self) -> Capabilities {
        // Events, outcomes and collected messages all go through the hook bridge.
        let bridged = self.bridge.is_some().then_some([
            Capability::Events,
            Capability::Outcome,
            Capability::Inbox,
        ]);
        let provenance = self.provenance();
        [
            Capability::Resume,
            Capability::Submit,
            Capability::Cancel,
            Capability::Effort,
            Capability::Model,
            Capability::Compact,
            Capability::RemoteControl,
            Capability::ScreenReading,
        ]
        .into_iter()
        .chain(bridged.into_iter().flatten())
        .fold(Capabilities::none(), |capabilities, capability| {
            capabilities.with(capability, provenance.clone())
        })
    }

    fn needs(&self) -> Result<Needs, InvalidValue> {
        needs::needs()
    }

    fn credentials(&self) -> Result<Credentials, InvalidValue> {
        needs::credentials()
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        request.require(&self.capabilities())?;
        let mut arguments: Vec<OsString> = Vec::new();
        // Its optional name would take the next plain word, so flags always follow it.
        if request.remote_control {
            arguments.push("--remote-control".into());
        }
        let conversation = match request.conversation {
            Conversation::New => {
                let id = new_session_id()?;
                arguments.extend(["--session-id".into(), id.as_str().into()]);
                id
            }
            Conversation::Resume(id) => {
                arguments.extend(["--resume".into(), id.as_str().into()]);
                id.clone()
            }
        };
        if let Some(effort) = request.effort {
            check_effort_shape(effort)?;
            arguments.extend(["--effort".into(), effort.into()]);
        }
        if let Some(model) = request.model {
            check_model(model)?;
            arguments.extend(["--model".into(), model.into()]);
        }
        // Variadic too: a flag or the `--` before the first message follows it.
        if let Some(root) = &self.instruction_skills {
            arguments.extend(["--add-dir".into(), root.into()]);
        }
        if let Some(bridge) = &self.bridge {
            arguments.extend([
                "--settings".into(),
                ClaudeCode::hook_settings(bridge).into(),
                // Variadic, so a flag or the `--` before the first message must follow it.
                "--mcp-config".into(),
                ClaudeCode::mcp_config(bridge).into(),
            ]);
        }
        if let Some(message) = request.first_message {
            arguments.extend(["--".into(), message.into()]);
        }
        Ok(LaunchCommand {
            program: self.program.clone(),
            arguments,
            environment: LAUNCH_ENVIRONMENT
                .iter()
                .map(|(name, value)| ((*name).into(), (*value).into()))
                .collect(),
            conversation: Some(conversation),
        })
    }

    /// Asks the installed Claude Code which efforts it has (its `--help`, read once per build of the program),
    /// so a level a Claude Code update adds works without a Torlek release and a level it does
    /// not have is refused naming the ones it lists. A model is not asked about: Claude Code
    /// refuses one the account cannot use itself.
    fn check_choices(
        &self,
        _model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), ProviderError> {
        let Some(effort) = effort else {
            return Ok(());
        };
        let HelpRead {
            levels, version, ..
        } = self.help_read();
        if levels.is_empty() || levels.iter().any(|level| level == effort) {
            return Ok(());
        }
        let version = version.map_or_else(String::new, |version| format!(" {version}"));
        Err(ProviderError {
            reason: format!(
                "effort {effort:?} is not one of {}, the levels `claude --help` lists \
                 (Claude Code{version})",
                levels.join(", ")
            ),
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
            Action::Cancel => Ok(vec![TerminalInput::Key(Key::Escape)]),
            Action::Compact => match activity {
                Some(Activity::Waiting) => Ok(typed_then_entered("/compact")),
                _ => Err(refused("compact runs only while it waits at its prompt")),
            },
            Action::Submit(message) => {
                if message.trim().is_empty() || message.chars().any(char::is_control) {
                    return Err(refused("a message is one line of text"));
                }
                match activity {
                    Some(Activity::Waiting) => Ok(typed_then_entered(message)),
                    // Claude Code queues a message typed during a turn with Ctrl-X Enter.
                    Some(Activity::Working) => Ok(vec![
                        TerminalInput::Text((*message).to_owned()),
                        TerminalInput::Key(Key::Control('x')),
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

    fn read_screen(&self, screen: &str) -> Option<Observation> {
        screen::read(screen)
    }

    /// Its `SessionStart` hook runs as Claude Code starts, before any prompt: within a few
    /// seconds on 2.1.317, MCP servers included.
    fn reports_at_launch(&self) -> bool {
        self.bridge.is_some()
    }

    /// A clone whose settings differ from the trusted checkout's, or cannot be compared, has
    /// any trust recorded earlier revoked: a reused clone never keeps the trust a previous
    /// branch earned.
    fn trust_workspace(&self, request: &TrustRequest<'_>) -> Result<WorkspaceTrust, ProviderError> {
        let refused = |reason: String| ProviderError { reason };
        let (config, when_missing) = match request.session_home {
            Some(session_home) => (
                trust::session_config_file(request.environment, session_home),
                trust::WhenMissing::Create,
            ),
            None => (
                trust::config_file(request.environment),
                trust::WhenMissing::Refuse,
            ),
        };
        let config = config.map_err(refused)?;
        let vetted = trust::unexpected_settings(
            request.workspace,
            request.trusted_checkout,
            request.working_dir,
        )
        .and_then(|unexpected| {
            if unexpected.is_empty() {
                return Ok(());
            }
            let names: Vec<String> = unexpected
                .iter()
                .map(|path| path.display().to_string())
                .collect();
            Err(format!(
                "the clone's harness settings differ from {}: {}",
                request.trusted_checkout.display(),
                names.join(", ")
            ))
        });
        if let Err(reason) = vetted {
            let revoked = match trust::record(&config, request.workspace, false, when_missing) {
                Ok(trust::Recorded::Written) => "; its earlier trust is revoked".to_owned(),
                Ok(trust::Recorded::Unchanged) => String::new(),
                Err(error) => format!("; its earlier trust, if any, could not be revoked: {error}"),
            };
            return Err(refused(format!("{reason}{revoked}")));
        }
        match trust::record(&config, request.workspace, true, when_missing).map_err(refused)? {
            trust::Recorded::Unchanged => Ok(WorkspaceTrust::AlreadyTrusted),
            trust::Recorded::Written => Ok(WorkspaceTrust::Recorded),
        }
    }
}

/// Claude Code's program could not be used.
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

fn typed_then_entered(text: &str) -> Vec<TerminalInput> {
    vec![
        TerminalInput::Text(text.to_owned()),
        TerminalInput::Key(Key::Enter),
    ]
}

fn is_dotted_version(word: &str) -> bool {
    let parts: Vec<&str> = word.split('.').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

/// `path` as one POSIX shell word: Claude Code runs hook commands through a shell.
fn shell_quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// Claude Code takes an alias or any model id, and itself refuses one the account cannot use,
/// so only its shape is checked here: one word that cannot read as a flag.
fn check_model(model: &str) -> Result<(), ProviderError> {
    let is_plain = model.len() <= MAX_MODEL_BYTES
        && model.starts_with(|first: char| first.is_ascii_alphanumeric())
        && model
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._:/@[]".contains(character));
    if is_plain {
        return Ok(());
    }
    Err(ProviderError {
        reason: format!(
            "model {model:?} is not a Claude Code model name: 1 to {MAX_MODEL_BYTES} ASCII \
             letters, digits and `-._:/@[]`, starting with a letter or digit"
        ),
    })
}

/// An effort is one plain word that cannot read as a flag; which words Claude Code has is
/// asked of it ([`Provider::check_choices`]).
fn check_effort_shape(effort: &str) -> Result<(), ProviderError> {
    let is_plain = effort.len() <= MAX_EFFORT_BYTES
        && effort.starts_with(|first: char| first.is_ascii_alphanumeric())
        && effort
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._:+/@".contains(character));
    if is_plain {
        return Ok(());
    }
    Err(ProviderError {
        reason: format!(
            "effort {effort:?} is not a Claude Code effort level: 1 to {MAX_EFFORT_BYTES} ASCII \
             letters, digits and `-._:+/@`, starting with a letter or digit"
        ),
    })
}

/// The levels in the parenthesis after `--effort` in `claude --help`, which may wrap onto
/// lines of its own: `--effort <level>  Effort level for the current session (low, medium)`.
/// The first parenthesis that holds two or more plain words separated by commas; a remark
/// such as `(applies per session)` is passed over. None when there is no such list.
fn effort_levels_in(help: &str) -> Option<Vec<String>> {
    let mut lines = help.lines().skip_while(|line| !line.contains("--effort"));
    let first = lines.next()?;
    let description: Vec<&str> = std::iter::once(first)
        .chain(lines.take_while(|line| !line.trim_start().starts_with('-')))
        .collect();
    description
        .join(" ")
        .split('(')
        .skip(1)
        .filter_map(|after_open| after_open.split_once(')').map(|(listed, _)| listed))
        .map(|listed| {
            listed
                .split(',')
                .map(|level| level.trim().to_owned())
                .collect::<Vec<String>>()
        })
        .find(|levels| {
            levels.len() >= 2 && levels.iter().all(|level| check_effort_shape(level).is_ok())
        })
}

/// A random (version 4) UUID, the form `--session-id` takes.
fn new_session_id() -> Result<ConversationId, ProviderError> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| ProviderError {
            reason: format!("cannot read /dev/urandom for a session id: {error}"),
        })?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let id = format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    Ok(ConversationId::new(id).expect("a UUID is a valid conversation id"))
}

/// The trimmed standard output of `program argument`, or why it failed. Kills it at the
/// deadline, and stops waiting for its output there too: a wrapper may leave a background
/// process holding its pipes.
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
