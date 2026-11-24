#![doc = include_str!("../README.md")]

pub mod hooks;
pub mod needs;
mod probe;

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
    Action, Activity, Capabilities, Capability, Conversation, ConversationId, Credentials,
    InvalidValue, Key, LaunchCommand, LaunchRequest, Needs, Probe, Provenance, Provider,
    ProviderError, ProviderName, SignIn, TerminalInput,
};

/// The Qwen Code versions the adapter's conformance tests pass against (`tested-versions.txt`,
/// #732). Empty until the owner runs the live conformance test (#1446), so every capability
/// is `discovered` or `probed`.
pub const TESTED_VERSIONS: &str = include_str!("../tested-versions.txt");

/// The Qwen Code version the fixtures and the launch words were read from: tag `v0.28.4`,
/// commit `e8424c88ddd463c56602063efde57c328b917cd4`.
pub const CONFORMANCE_VERSION: &str = "0.28.4";

/// Longest `--model` value: a model id of the owner's `modelProviders` (`qwen3-coder-plus`,
/// `Qwen/Qwen3-Coder-480B-A35B-Instruct`) or a local server's name for its model.
const MAX_MODEL_BYTES: usize = 200;

/// Every flag a launch may pass; the probe and the conformance test find each in
/// `qwen --help`.
pub const LAUNCH_FLAGS: [&str; 4] = [
    "--session-id",
    "--resume",
    "--model",
    "--prompt-interactive",
];

/// The variable that moves Qwen Code's system-defaults settings file, the lowest of its
/// settings scopes: a launch points it at the [`Bridge::defaults_file`], so the bridge's hooks
/// load without writing any settings of the owner's or the project's, which all still win
/// over it.
pub const SYSTEM_DEFAULTS_VARIABLE: &str = "QWEN_CODE_SYSTEM_DEFAULTS_PATH";

/// How long `qwen --version` may take at discovery.
const VERSION_DEADLINE: Duration = Duration::from_secs(10);

/// What a launch ships into the sandbox: the system-defaults file that subscribes the hook
/// program to Qwen Code's hook events, the hook program, and the `torlek` program it reports
/// through. The daemon writes the file at its start ([`defaults_settings`]), outside the
/// session's own folders, so an agent cannot change which commands its hooks run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    pub defaults_file: PathBuf,
    pub hook_program: PathBuf,
    pub cockpit_program: PathBuf,
}

/// The argument that makes `torlek-qwen-hook` print [`hook_version_line`] and exit.
pub const HOOK_VERSION_FLAG: &str = "--version";

/// What `torlek-qwen-hook --version` prints: its name and the version it was built as.
pub fn hook_version_line() -> String {
    format!("torlek-qwen-hook {}", env!("CARGO_PKG_VERSION"))
}

/// How long the hook bridge may take to answer [`HOOK_VERSION_FLAG`] at the daemon's start.
const HOOK_CHECK_DEADLINE: Duration = Duration::from_secs(10);

impl Bridge {
    /// Whether this bridge can run here: the three files exist, the defaults file is an
    /// absolute path holding this bridge's [`defaults_settings`], `torlek` has its execute
    /// bit, and the hook runs and answers [`HOOK_VERSION_FLAG`] with this release's
    /// [`hook_version_line`]. The reason when not. Blocks up to 10 seconds.
    pub fn check(&self) -> Result<(), String> {
        self.check_within(HOOK_CHECK_DEADLINE)
    }

    /// [`Bridge::check`], giving up on the hook after `deadline`.
    pub fn check_within(&self, deadline: Duration) -> Result<(), String> {
        let missing: Vec<String> = [
            &self.defaults_file,
            &self.hook_program,
            &self.cockpit_program,
        ]
        .into_iter()
        .filter(|file| !file.is_file())
        .map(|file| file.display().to_string())
        .collect();
        if !missing.is_empty() {
            return Err(format!("{} missing", missing.join(" and ")));
        }
        // The hook command names both paths as text; another spelling would run nothing.
        if let Some(file) = [&self.hook_program, &self.cockpit_program]
            .into_iter()
            .find(|file| file.to_str().is_none())
        {
            return Err(format!(
                "{} is not UTF-8, so the hook command cannot name it",
                file.display()
            ));
        }
        if !self.defaults_file.is_absolute() {
            return Err(format!(
                "{} is not an absolute path, which Qwen Code would resolve from the project",
                self.defaults_file.display()
            ));
        }
        let defaults = std::fs::read_to_string(&self.defaults_file)
            .map_err(|error| format!("{}: {error}", self.defaults_file.display()))?;
        if defaults != defaults_settings(&self.hook_program, &self.cockpit_program) {
            return Err(format!(
                "{} does not hold this bridge's hooks",
                self.defaults_file.display()
            ));
        }
        if !is_executable(&self.cockpit_program) {
            return Err(format!(
                "{} has no execute bit, so the hook cannot run it",
                self.cockpit_program.display()
            ));
        }
        let answer = run_with_deadline(&self.hook_program, HOOK_VERSION_FLAG, deadline)
            .map_err(|reason| format!("{}: {reason}", self.hook_program.display()))?;
        let expected = hook_version_line();
        if answer != expected {
            return Err(format!(
                "{}: `{HOOK_VERSION_FLAG}` printed {answer:?}, not this release's {expected:?}",
                self.hook_program.display()
            ));
        }
        Ok(())
    }
}

/// The system-defaults settings a launch points [`SYSTEM_DEFAULTS_VARIABLE`] at: `hook_program
/// cockpit_program` on every event of [`hooks::HOOK_EVENTS`], and no update check, since the
/// install is read-only in the sandbox and an update is the owner's. Qwen Code merges hooks of
/// every scope, so the owner's and the project's own hooks still run beside these.
pub fn defaults_settings(hook_program: &Path, cockpit_program: &Path) -> String {
    let command = format!(
        "{} {}",
        shell_quoted(hook_program),
        shell_quoted(cockpit_program)
    );
    let report = serde_json::json!([{
        "hooks": [{ "type": "command", "command": command, "name": "torlek" }]
    }]);
    let events: serde_json::Map<String, serde_json::Value> = hooks::HOOK_EVENTS
        .iter()
        .map(|event| ((*event).to_owned(), report.clone()))
        .collect();
    let settings = serde_json::json!({
        "general": { "enableAutoUpdate": false },
        "hooks": events,
    });
    format!("{settings}\n")
}

fn is_executable(program: &Path) -> bool {
    std::fs::metadata(program).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

/// The Qwen Code adapter.
#[derive(Debug)]
pub struct QwenCode {
    name: ProviderName,
    program: PathBuf,
    version: Option<String>,
    /// Without it, launches subscribe no hooks and neither `events` nor `outcome` is reported.
    bridge: Option<Bridge>,
    /// What the runtime probe found in this version, when it ran (#732).
    probe: Option<Probe>,
}

impl QwenCode {
    /// The adapter for `program`, running `version` when known.
    pub fn new(program: PathBuf, version: Option<String>, bridge: Option<Bridge>) -> QwenCode {
        QwenCode {
            name: ProviderName::new("qwen").expect("`qwen` is a valid provider name"),
            program,
            version,
            bridge,
            probe: None,
        }
    }

    /// What [`probe()`](fn@probe) found in the running version. Without a version it was taken
    /// for, or on a tested version, the probe changes nothing.
    pub fn with_probe(mut self, probe: Probe) -> QwenCode {
        self.probe = Some(probe);
        self
    }

    /// The adapter for `program`, with the version it prints. Blocks up to 10 seconds.
    pub fn discover(program: &Path, bridge: Option<Bridge>) -> Result<QwenCode, DiscoveryError> {
        QwenCode::discover_within(program, bridge, VERSION_DEADLINE)
    }

    /// [`QwenCode::discover`], giving up on `--version` after `deadline`.
    pub fn discover_within(
        program: &Path,
        bridge: Option<Bridge>,
        deadline: Duration,
    ) -> Result<QwenCode, DiscoveryError> {
        let failed = |reason: String| DiscoveryError {
            program: program.to_path_buf(),
            reason,
        };
        let output = run_with_deadline(program, "--version", deadline).map_err(failed)?;
        // `qwen --version` prints the bare version (`0.28.4`).
        let version = output
            .split_whitespace()
            .next()
            .filter(|word| is_dotted_version(word))
            .ok_or_else(|| failed(format!("`--version` printed {output:?}, not a version")))?;
        Ok(QwenCode::new(
            program.to_path_buf(),
            Some(version.to_owned()),
            bridge,
        ))
    }

    /// The first executable `qwen` in the directories of `path` (a `PATH` value).
    pub fn find_on_path(path: &OsStr) -> Option<PathBuf> {
        std::env::split_paths(path)
            .map(|dir| dir.join("qwen"))
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

impl Provider for QwenCode {
    fn name(&self) -> &ProviderName {
        &self.name
    }

    fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    fn capabilities(&self) -> Capabilities {
        // Events and outcomes both come from the hook bridge.
        let bridged = self
            .bridge
            .is_some()
            .then_some([Capability::Events, Capability::Outcome]);
        Capabilities::from_probe(
            self.version.as_deref(),
            TESTED_VERSIONS,
            self.probe.as_ref(),
            [
                Capability::Resume,
                Capability::Submit,
                Capability::Cancel,
                Capability::Model,
                Capability::Compact,
            ]
            .into_iter()
            .chain(bridged.into_iter().flatten()),
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

    /// Never read (#1446, owner decision Q3): Qwen Code 0.28.4 has no sign-in status command
    /// (`qwen auth` was removed), and its sign-in may be an OAuth file, a key variable, a
    /// `.env` file or the owner's `settings.json`, or none at all for a local server.
    fn sign_in(&self) -> SignIn {
        SignIn::unknown(
            "Qwen Code has no command that reports its sign-in",
            "start qwen and run /auth, or set the provider key",
        )
    }

    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        request.require(&self.capabilities())?;
        let mut arguments: Vec<OsString> = Vec::new();
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
        if let Some(model) = request.model {
            check_model(model)?;
            arguments.extend(["--model".into(), model.into()]);
        }
        if let Some(message) = request.first_message {
            // One word with its flag: a message starting with `-` is never read as a flag.
            let mut word = OsString::from("--prompt-interactive=");
            word.push(message);
            arguments.push(word);
        }
        let environment = self
            .bridge
            .iter()
            .map(|bridge| {
                (
                    SYSTEM_DEFAULTS_VARIABLE.into(),
                    bridge.defaults_file.clone().into_os_string(),
                )
            })
            .collect();
        Ok(LaunchCommand {
            program: self.program.clone(),
            arguments,
            environment,
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
                Some(Activity::Waiting) => Ok(typed_then_entered("/compress")),
                _ => Err(refused("compact runs only while it waits at its prompt")),
            },
            Action::Submit(message) => {
                if message.trim().is_empty() || message.chars().any(char::is_control) {
                    return Err(refused("a message is one line of text"));
                }
                match activity {
                    Some(Activity::Waiting) => Ok(typed_then_entered(message)),
                    // Enter during a turn would steer it; Ctrl-Q queues the message for the
                    // next turn instead.
                    Some(Activity::Working) => Ok(vec![
                        TerminalInput::Text((*message).to_owned()),
                        TerminalInput::Key(Key::Control('q')),
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

    /// Its `SessionStart` hook runs as Qwen Code starts, before any prompt.
    fn reports_at_launch(&self) -> bool {
        self.bridge.is_some()
    }

    fn transient_limit_reasons(&self) -> &'static [&'static str] {
        &[hooks::RATE_LIMIT]
    }
}

/// Qwen Code's program could not be used.
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

/// `path` as one POSIX shell word: Qwen Code runs a command hook with `bash -c`.
fn shell_quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// Qwen Code refuses a model that no `modelProviders` entry or auth type offers, so only the
/// shape is checked here: one word that cannot read as a flag.
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
            "model {model:?} is not a Qwen Code model name: 1 to {MAX_MODEL_BYTES} ASCII \
             letters, digits and `-._:/@`, starting with a letter or digit"
        ),
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
