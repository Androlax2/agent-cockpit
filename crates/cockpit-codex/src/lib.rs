#![doc = include_str!("../README.md")]

pub mod hooks;

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cockpit_provider_api::{
    Capabilities, Capability, Conversation, LaunchCommand, LaunchRequest, Provenance, Provider,
    ProviderError, ProviderName,
};

/// The Codex version this adapter's conformance tests pin; capabilities are `tested` only on
/// it.
pub const CONFORMANCE_VERSION: &str = "0.161.0";

/// Every flag a launch may pass; the conformance test finds each in `codex --help` and
/// `codex resume --help`. `--no-daemon` keeps the conversation out of Codex's shared
/// background server, whose hooks run with the environment, launch token included, of
/// whichever launch started it (the failure #184 fixed for Claude Code).
pub const LAUNCH_FLAGS: [&str; 3] = ["--no-daemon", "--enable", "--config"];

/// The feature that runs hooks; a launch enables it so an owner who turned it off still
/// gets the session's events.
pub const HOOKS_FEATURE: &str = "hooks";

/// How long `codex --version` may take at discovery.
const VERSION_DEADLINE: Duration = Duration::from_secs(10);

/// What a launch ships into the sandbox: the hook program Codex runs on each hook event, and
/// the `cockpit` program it reports through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bridge {
    pub hook_program: PathBuf,
    pub cockpit_program: PathBuf,
}

/// The argument that makes `cockpit-codex-hook` print [`hook_version_line`] and exit.
pub const HOOK_VERSION_FLAG: &str = "--version";

/// What `cockpit-codex-hook --version` prints: its name and the version it was built as.
pub fn hook_version_line() -> String {
    format!("cockpit-codex-hook {}", env!("CARGO_PKG_VERSION"))
}

/// How long the hook bridge may take to answer [`HOOK_VERSION_FLAG`] at the daemon's start.
const HOOK_CHECK_DEADLINE: Duration = Duration::from_secs(10);

impl Bridge {
    /// Whether this bridge can run here (#203): both programs are files, `cockpit` has its
    /// execute bit, and the hook runs and answers [`HOOK_VERSION_FLAG`] with this release's
    /// [`hook_version_line`]. The reason when not. Blocks up to 10 seconds.
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
        if !answer.starts_with("cockpit-codex-hook ") {
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
    /// Without it, launches subscribe no hooks and `events` is not reported.
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
        // `codex-cli 0.161.0`; warnings go to standard error.
        let version = match output.split_whitespace().collect::<Vec<_>>()[..] {
            ["codex-cli", version] if is_dotted_version(version) => version,
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

    fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Resume, and `events` with the bridge. Not submit or cancel: they go through Codex's
    /// app-server, which waits on #407. Not outcome: a failed turn sends no hook, so it would
    /// never end. Not effort, compact, billing, commands, evidence, remote control or screen
    /// reading: untested.
    fn capabilities(&self) -> Capabilities {
        let provenance = self.provenance();
        let bridged = self.bridge.is_some().then_some(Capability::Events);
        [Capability::Resume]
            .into_iter()
            .chain(bridged)
            .fold(Capabilities::none(), |capabilities, capability| {
                capabilities.with(capability, provenance.clone())
            })
    }

    /// `codex [resume] --no-daemon [--enable hooks -c <hooks>…] -- [<thread id>] [<first
    /// message>]`. A new conversation's thread id is Codex's own and unknown at launch: the
    /// first report names it. Every word after `--` is positional, so neither can be read as
    /// a flag.
    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        request.require(&self.capabilities())?;
        let mut arguments: Vec<OsString> = Vec::new();
        if let Conversation::Resume(_) = request.conversation {
            arguments.push("resume".into());
        }
        arguments.push("--no-daemon".into());
        if let Some(bridge) = &self.bridge {
            arguments.extend(["--enable".into(), HOOKS_FEATURE.into()]);
            for value in hooks::subscription(&bridge.hook_command()) {
                arguments.extend(["--config".into(), value.into()]);
            }
        }
        let positional: Vec<&str> = match request.conversation {
            Conversation::New => Vec::new(),
            Conversation::Resume(id) => vec![id.as_str()],
        }
        .into_iter()
        .chain(request.first_message)
        .collect();
        if !positional.is_empty() {
            arguments.push("--".into());
            arguments.extend(positional.into_iter().map(OsString::from));
        }
        Ok(LaunchCommand {
            program: self.program.clone(),
            arguments,
            environment: Vec::new(),
            conversation: match request.conversation {
                Conversation::New => None,
                Conversation::Resume(id) => Some(id.clone()),
            },
        })
    }
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
