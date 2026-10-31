#![doc = include_str!("../README.md")]

pub mod hooks;
mod screen;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cockpit_provider_api::{
    Action, Activity, Capabilities, Capability, Conversation, ConversationId, Key, LaunchCommand,
    LaunchRequest, Observation, Provenance, Provider, ProviderError, ProviderName, TerminalInput,
};

/// The Claude Code version this adapter's conformance tests pin; capabilities are `tested`
/// only on it.
pub const CONFORMANCE_VERSION: &str = "2.1.316";

/// Claude Code's `--effort` levels.
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Every flag a launch may pass; the conformance test finds each in `claude --help`.
pub const LAUNCH_FLAGS: [&str; 5] = [
    "--remote-control",
    "--session-id",
    "--resume",
    "--settings",
    "--effort",
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

/// The Claude Code adapter.
#[derive(Debug)]
pub struct ClaudeCode {
    name: ProviderName,
    program: PathBuf,
    version: Option<String>,
    /// Without it, launches subscribe no hooks and `events` is not reported.
    bridge: Option<Bridge>,
}

impl ClaudeCode {
    /// The adapter for `program`, running `version` when known.
    pub fn new(program: PathBuf, version: Option<String>, bridge: Option<Bridge>) -> ClaudeCode {
        ClaudeCode {
            name: ProviderName::new("claude").expect("`claude` is a valid provider name"),
            program,
            version,
            bridge,
        }
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

    /// `--settings` JSON that runs `bridge` on every hook event it normalizes.
    fn hook_settings(bridge: &Bridge) -> String {
        let command = format!(
            "{} {}",
            shell_quoted(&bridge.hook_program),
            shell_quoted(&bridge.cockpit_program)
        );
        let hook = serde_json::json!([{ "hooks": [{ "type": "command", "command": command }] }]);
        let events: serde_json::Map<String, serde_json::Value> = hooks::HOOK_EVENTS
            .iter()
            .map(|event| ((*event).to_owned(), hook.clone()))
            .collect();
        serde_json::json!({ "hooks": events }).to_string()
    }
}

impl Provider for ClaudeCode {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    fn capabilities(&self) -> Capabilities {
        // `outcome` waits until the agent channel carries a turn's outcome (#146).
        let events = self.bridge.is_some().then_some(Capability::Events);
        let provenance = self.provenance();
        [
            Capability::Resume,
            Capability::Submit,
            Capability::Cancel,
            Capability::Effort,
            Capability::Compact,
            Capability::RemoteControl,
            Capability::ScreenReading,
        ]
        .into_iter()
        .chain(events)
        .fold(Capabilities::none(), |capabilities, capability| {
            capabilities.with(capability, provenance.clone())
        })
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
            if !EFFORT_LEVELS.contains(&effort) {
                return Err(ProviderError {
                    reason: format!(
                        "effort {effort:?} is not one of {}",
                        EFFORT_LEVELS.join(", ")
                    ),
                });
            }
            arguments.extend(["--effort".into(), effort.into()]);
        }
        if let Some(bridge) = &self.bridge {
            arguments.extend([
                "--settings".into(),
                ClaudeCode::hook_settings(bridge).into(),
            ]);
        }
        if let Some(message) = request.first_message {
            arguments.extend(["--".into(), message.into()]);
        }
        Ok(LaunchCommand {
            program: self.program.clone(),
            arguments,
            environment: Vec::new(),
            conversation: Some(conversation),
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
