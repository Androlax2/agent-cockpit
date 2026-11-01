//! `cockpit-codex-session`: what a Codex session runs in its terminal (#407). It starts the
//! session's private app-server (`codex app-server --listen unix://<session dir>/codex.sock`,
//! with the launch's hooks), then the TUI against it (`codex --remote`), both with the
//! launch's environment, so hooks keep the launch token and Cockpit delivers messages
//! through the socket. It reports each failed turn, which sends no hook, with
//! `cockpit agent report`, stops the app-server when the TUI exits, and exits with the TUI's
//! status.
//!
//! The app-server's output and the launcher's own errors after the TUI starts go to
//! `codex-app-server.log` beside the socket: the terminal is the TUI's.
//! `cockpit-codex-session --version` is the daemon's start check (#203).

use std::ffi::OsString;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_codex::app_server::AppServer;
use cockpit_provider_api::ConversationId;

/// How long the app-server may take to accept connections.
const SERVER_START_DEADLINE: Duration = Duration::from_secs(30);
/// How long the app-server may take to stop after `SIGTERM` before it is killed.
const SERVER_STOP_DEADLINE: Duration = Duration::from_secs(5);
/// How long the failure watcher's calls may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

const LOG_FILE: &str = "codex-app-server.log";

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args_os().skip(1)) {
        Ok(Parsed::Version) => {
            println!("{}", cockpit_codex::version_line("cockpit-codex-session"));
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Run(options)) => options,
        Err(reason) => {
            eprintln!("cockpit-codex-session: {reason}");
            return ExitCode::FAILURE;
        }
    };
    match run(&options) {
        Ok(code) => code,
        Err(reason) => {
            eprintln!("cockpit-codex-session: {reason}");
            ExitCode::FAILURE
        }
    }
}

struct Options {
    cockpit: PathBuf,
    codex: PathBuf,
    socket: PathBuf,
    configs: Vec<String>,
    resume: Option<ConversationId>,
    message: Option<String>,
}

enum Parsed {
    Version,
    Run(Options),
}

impl Options {
    fn parse(mut arguments: impl Iterator<Item = OsString>) -> Result<Parsed, String> {
        let usage = "usage: cockpit-codex-session --cockpit <program> --codex <program> \
                     --socket <path> [--config <value>]… [--resume <thread id>] \
                     [--message <text>] | --version";
        let (mut cockpit, mut codex, mut socket) = (None, None, None);
        let mut configs = Vec::new();
        let (mut resume, mut message) = (None, None);
        while let Some(flag) = arguments.next() {
            if flag == cockpit_codex::HOOK_VERSION_FLAG {
                return Ok(Parsed::Version);
            }
            let value = arguments
                .next()
                .ok_or_else(|| format!("{} needs a value; {usage}", flag.to_string_lossy()))?;
            let text = || {
                value
                    .clone()
                    .into_string()
                    .map_err(|_| format!("{} is not UTF-8", flag.to_string_lossy()))
            };
            match flag.to_str() {
                Some("--cockpit") => cockpit = Some(PathBuf::from(&value)),
                Some("--codex") => codex = Some(PathBuf::from(&value)),
                Some("--socket") => socket = Some(PathBuf::from(&value)),
                Some("--config") => configs.push(text()?),
                Some("--resume") => {
                    resume = Some(ConversationId::new(text()?).map_err(|error| error.to_string())?);
                }
                Some("--message") => message = Some(text()?),
                _ => {
                    return Err(format!(
                        "unknown option {}; {usage}",
                        flag.to_string_lossy()
                    ));
                }
            }
        }
        let (Some(cockpit), Some(codex), Some(socket)) = (cockpit, codex, socket) else {
            return Err(usage.to_owned());
        };
        Ok(Parsed::Run(Options {
            cockpit,
            codex,
            socket,
            configs,
            resume,
            message,
        }))
    }
}

fn run(options: &Options) -> Result<ExitCode, String> {
    let log_path = options.socket.with_file_name(LOG_FILE);
    let log = File::create(&log_path)
        .map_err(|error| format!("cannot create {}: {error}", log_path.display()))?;
    let log = Arc::new(Mutex::new(log));
    let mut server = start_server(options, &log)?;
    if let Err(reason) = wait_until_listening(&options.socket, &mut server) {
        stop(server);
        return Err(format!("{reason}; its output is in {}", log_path.display()));
    }
    watch_in_background(options, Arc::clone(&log));
    let tui = Command::new(&options.codex)
        .args(cockpit_codex::tui_arguments(
            &options.socket,
            options.resume.as_ref(),
            options.message.as_deref(),
        ))
        .status();
    stop(server);
    let status = tui.map_err(|error| format!("cannot run {}: {error}", options.codex.display()))?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from))
}

/// The app-server, after a stale socket of an earlier launch of this session is removed.
fn start_server(options: &Options, log: &Arc<Mutex<File>>) -> Result<Child, String> {
    if options.socket.symlink_metadata().is_ok() {
        std::fs::remove_file(&options.socket).map_err(|error| {
            format!(
                "cannot remove the stale {}: {error}",
                options.socket.display()
            )
        })?;
    }
    let output = |log: &Arc<Mutex<File>>| {
        log.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .try_clone()
            .map(Stdio::from)
            .map_err(|error| format!("cannot share the log: {error}"))
    };
    Command::new(&options.codex)
        .args(cockpit_codex::server_arguments(
            &options.socket,
            &options.configs,
        ))
        .stdin(Stdio::null())
        .stdout(output(log)?)
        .stderr(output(log)?)
        .spawn()
        .map_err(|error| format!("cannot run {}: {error}", options.codex.display()))
}

fn wait_until_listening(socket: &Path, server: &mut Child) -> Result<(), String> {
    let ends_at = Instant::now() + SERVER_START_DEADLINE;
    loop {
        if std::os::unix::net::UnixStream::connect(socket).is_ok() {
            return Ok(());
        }
        if let Ok(Some(status)) = server.try_wait() {
            return Err(format!("Codex's app-server exited ({status})"));
        }
        if Instant::now() > ends_at {
            return Err(format!(
                "Codex's app-server did not listen on {} within {} s",
                socket.display(),
                SERVER_START_DEADLINE.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Reports each failed turn until the connection ends; why it ended goes to the log.
fn watch_in_background(options: &Options, log: Arc<Mutex<File>>) {
    let socket = options.socket.clone();
    let cockpit = options.cockpit.clone();
    std::thread::spawn(move || {
        let note = |line: String| {
            let mut log = log
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _ = writeln!(log, "cockpit-codex-session: {line}");
        };
        let mut server = match AppServer::connect(&socket, "cockpit-codex-session", CALL_TIMEOUT) {
            Ok(server) => server,
            Err(error) => return note(format!("failed turns are not reported: {error}")),
        };
        if let Err(error) = server.set_timeout(None) {
            return note(format!("failed turns are not reported: {error}"));
        }
        let ended = cockpit_codex::turns::watch_failures(&mut server, |report| {
            let status = Command::new(&cockpit)
                .args(&report)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match status {
                Ok(status) if status.success() => {}
                Ok(status) => note(format!("`cockpit {}` failed ({status})", report.join(" "))),
                Err(error) => note(format!("cannot run {}: {error}", cockpit.display())),
            }
        });
        note(format!("stopped following failed turns: {ended}"));
    });
}

/// Stops the app-server: `SIGTERM`, which Codex's npm wrapper passes to the server it runs,
/// then `SIGKILL` after a grace period.
fn stop(mut server: Child) {
    let pid = rustix::process::Pid::from_child(&server);
    let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
    let ends_at = Instant::now() + SERVER_STOP_DEADLINE;
    while Instant::now() < ends_at {
        if let Ok(Some(_)) = server.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = server.kill();
    let _ = server.wait();
}
