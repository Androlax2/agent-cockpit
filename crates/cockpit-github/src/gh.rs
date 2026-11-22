//! Running `gh`: bounded in time and output, with an environment of its own so neither the
//! daemon's variables nor the owner's `gh` sign-in reach it, and the token only in `GH_TOKEN`.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cockpit_forge_api::{ForgeError, Token};

/// Longest a call may take; a forge call is a few round trips.
pub(crate) const CALL_DEADLINE: Duration = Duration::from_secs(30);
/// Most bytes of an answer kept; a list of 100 issues is far below it.
const MAX_OUTPUT_BYTES: u64 = 8 * 1024 * 1024;
/// How long `gh`'s output may take to drain once it exited, apart from the call's deadline:
/// a call that ended just in time has its answer read.
const OUTPUT_GRACE: Duration = Duration::from_secs(5);
/// Most characters of `gh`'s standard error repeated in an error.
const MAX_REASON_CHARS: usize = 300;

/// Variables passed through from the daemon: where programs are, and how the host reaches
/// the network. Nothing else, so no `GH_*`, `GITHUB_*` or `HOME` of the owner's reaches `gh`.
const PASSED_THROUGH: [&str; 9] = [
    "PATH",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// One `gh` program and the private folder it keeps its configuration in.
#[derive(Debug, Clone)]
pub(crate) struct Gh {
    pub(crate) program: PathBuf,
    pub(crate) config_dir: PathBuf,
}

/// What a finished call printed.
pub(crate) struct Answer {
    pub(crate) success: bool,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

impl Gh {
    /// Runs `gh` with `arguments`, never through a shell, in its configuration folder so no
    /// checkout's remote is inferred. `token` is set as `GH_TOKEN` only; `input`, a body, goes
    /// to standard input, out of the argument list other processes can read.
    pub(crate) fn run(
        &self,
        arguments: &[String],
        token: Option<&Token>,
        input: Option<&str>,
        deadline: Duration,
    ) -> Result<Answer, ForgeError> {
        let mut command = Command::new(&self.program);
        command
            .args(arguments)
            .env_clear()
            .envs(self.environment())
            .current_dir(&self.config_dir)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(token) = token {
            command.env("GH_TOKEN", token.expose());
        }
        let unavailable = |reason: String| ForgeError::Unavailable {
            reason: format!("`gh {}`: {reason}", describe(arguments)),
        };
        if !self.config_dir.is_dir() {
            return Err(unavailable(format!(
                "its configuration folder {} is not a folder",
                self.config_dir.display()
            )));
        }
        let mut child = command.spawn().map_err(|cause| {
            unavailable(format!("cannot run {}: {cause}", self.program.display()))
        })?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            let input = input.to_owned();
            // A child that exits without reading closes the pipe; its answer says why.
            std::thread::spawn(move || {
                let _ = stdin.write_all(input.as_bytes());
            });
        }
        let stdout = read_in_background(child.stdout.take());
        let stderr = read_in_background(child.stderr.take());
        let ends_at = Instant::now() + deadline;
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|cause| unavailable(format!("cannot wait for it: {cause}")))?
            {
                break status;
            }
            if Instant::now() > ends_at {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unavailable(format!(
                    "did not answer within {} s; what it asked may have been done",
                    deadline.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let drained_by = Instant::now() + OUTPUT_GRACE;
        let remaining = || drained_by.saturating_duration_since(Instant::now());
        let gave_up = || {
            unavailable(
                "it exited but its output did not end; what it asked may have been done".to_owned(),
            )
        };
        let stdout = stdout.recv_timeout(remaining()).map_err(|_| gave_up())?;
        let stderr = stderr.recv_timeout(remaining()).map_err(|_| gave_up())?;
        Ok(Answer {
            success: status.success(),
            stdout,
            stderr,
        })
    }

    fn environment(&self) -> Vec<(OsString, OsString)> {
        let mut environment: Vec<(OsString, OsString)> = PASSED_THROUGH
            .iter()
            .filter_map(|name| std::env::var_os(name).map(|value| ((*name).into(), value)))
            .collect();
        let private = |name: &str, folder: &str| -> (OsString, OsString) {
            (name.into(), self.config_dir.join(folder).into_os_string())
        };
        environment.extend([
            (
                "GH_CONFIG_DIR".into(),
                self.config_dir.clone().into_os_string(),
            ),
            ("HOME".into(), self.config_dir.clone().into_os_string()),
            private("XDG_CONFIG_HOME", "config"),
            private("XDG_STATE_HOME", "state"),
            private("XDG_CACHE_HOME", "cache"),
            private("XDG_DATA_HOME", "data"),
            ("GH_HOST".into(), crate::GIT_HOST.into()),
            ("GH_PROMPT_DISABLED".into(), "1".into()),
            ("GH_NO_UPDATE_NOTIFIER".into(), "1".into()),
            ("GH_NO_EXTENSION_UPDATE_NOTIFIER".into(), "1".into()),
            ("NO_COLOR".into(), "1".into()),
            ("GH_PAGER".into(), "".into()),
        ]);
        environment
    }
}

/// The call's verb for an error (`issue view`), never its arguments: a body or a title is the
/// caller's text.
fn describe(arguments: &[String]) -> String {
    arguments
        .iter()
        .take(2)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ")
}

fn read_in_background(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = (&mut pipe).take(MAX_OUTPUT_BYTES).read_to_end(&mut bytes);
            // Keep draining, so a chatty child never blocks on a full pipe.
            let _ = std::io::copy(&mut pipe, &mut std::io::sink());
        }
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}

/// The error a failed call stands for, read from `gh`'s standard error, with the token masked
/// and the text bounded.
pub(crate) fn failure(
    arguments: &[String],
    answer: &Answer,
    token: Option<&Token>,
    what: &str,
) -> ForgeError {
    let mut said: String = answer
        .stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(token) = token {
        said = said.replace(token.expose(), "<token>");
    }
    let said: String = said.chars().take(MAX_REASON_CHARS).collect();
    if said.contains("Could not resolve to a Repository") {
        return ForgeError::NotFound {
            what: "the repository".to_owned(),
        };
    }
    let is_not_found = said.contains("Could not resolve to")
        || said.contains("HTTP 404")
        || said.contains("no pull requests found")
        || said.contains("Not Found");
    if is_not_found {
        return ForgeError::NotFound {
            what: what.to_owned(),
        };
    }
    // GitHub's GraphQL refusals (a head with no commits, a resource the token cannot reach)
    // carry no HTTP status; not-found ones were read above. None succeeds when retried.
    let lowered = said.to_lowercase();
    let is_refused = [
        "http 401",
        "http 403",
        "http 422",
        "bad credentials",
        "authentication",
        "already exists",
        "graphql:",
    ]
    .iter()
    .any(|sign| lowered.contains(sign));
    let reason = format!("`gh {}`: {said}", describe(arguments));
    if is_refused {
        ForgeError::Refused { reason }
    } else {
        ForgeError::Unavailable { reason }
    }
}

/// The version `gh --version` prints first (`gh version 2.104.0 (2026-11-12)`).
pub(crate) fn parse_version(output: &str) -> Option<&str> {
    let version = output.lines().next()?.strip_prefix("gh version ")?;
    let version = version.split_whitespace().next()?;
    version
        .chars()
        .all(|character| character.is_ascii_digit() || character == '.')
        .then_some(version)
}
