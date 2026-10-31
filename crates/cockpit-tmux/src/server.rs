use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Error;
use crate::names::{PaneId, SessionName};
use crate::version::{TmuxVersion, check_version};

/// Cockpit's own tmux configuration, passed with `-f` so the user's `~/.tmux.conf` never
/// applies. tmux reads it only when a command starts the server.
pub const CONFIG: &str = "\
# Cockpit's private tmux server (cockpit-tmux). The user's ~/.tmux.conf is never read.
# Keep a pane that exited, with its last output and exit status, until Cockpit closes it.
set-option -g remain-on-exit on
# Agent transcripts are long; the default 2000 lines loses most of a build log.
set-option -g history-limit 50000
# A one-word command runs through this shell, quoted for sh; never the user's login shell.
set-option -g default-shell /bin/sh
# The TUI runs in a pane: an Escape key must not wait for the rest of a sequence.
set-option -s escape-time 0
";

/// Which tmux server socket every command addresses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Socket {
    /// `-L name`: a socket named `name` in tmux's per-user directory.
    Named(String),
    /// `-S path`: a socket at an explicit path (tests use one in a temporary directory).
    Path(PathBuf),
}

impl Socket {
    /// The product's server, `-L cockpit` (report 05).
    pub fn cockpit() -> Socket {
        Socket::Named("cockpit".to_string())
    }

    fn arguments(&self) -> [&OsStr; 2] {
        match self {
            Socket::Named(name) => ["-L".as_ref(), name.as_ref()],
            Socket::Path(path) => ["-S".as_ref(), path.as_os_str()],
        }
    }
}

/// Where the tmux binary, the server socket and Cockpit's tmux configuration are.
#[derive(Clone, Debug)]
pub struct ServerSpec {
    /// A bare name is looked up on `PATH`.
    pub tmux: PathBuf,
    pub socket: Socket,
    /// Written by [`Server::open`] with [`CONFIG`]; its directory must exist.
    pub config_path: PathBuf,
}

/// The parts of a new tmux session. `command` runs as given: several words directly, a
/// single word quoted for `/bin/sh` (tmux runs a lone word through its `default-shell`).
#[derive(Clone, Debug)]
pub struct SessionLaunch {
    pub name: SessionName,
    pub working_dir: PathBuf,
    pub command: Vec<OsString>,
    /// Added to the server's environment for this session only (`new-session -e`).
    pub environment: Vec<(String, OsString)>,
    /// The size the session's window has until a client attaches.
    pub columns: u16,
    pub rows: u16,
}

/// Whether a pane's process is still running. Observation only: a pane that exited
/// proves nothing about the work (report 05); `cockpit-processes` owns termination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneState {
    Running,
    /// `status` is `None` when the process ended by a signal.
    Exited {
        status: Option<i32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: PaneId,
    /// The process tmux started in the pane; `None` once it has ended.
    pub pid: Option<u32>,
    pub state: PaneState,
}

/// A handle on Cockpit's private tmux server. It holds no server process: the server
/// outlives every handle and every daemon, which is what reattach after a daemon restart
/// relies on. Dropping a handle changes nothing on the server.
#[derive(Debug)]
pub struct Server {
    spec: ServerSpec,
    version: TmuxVersion,
}

impl Server {
    /// Checks the tmux version (the readiness check) and writes [`CONFIG`]. Starts nothing:
    /// the first session starts the server, and an already running one is reused as is.
    pub fn open(spec: ServerSpec) -> Result<Server, Error> {
        let version = check_version(&spec.tmux)?;
        write_private_file(&spec.config_path, CONFIG)?;
        Ok(Server { spec, version })
    }

    pub fn version(&self) -> TmuxVersion {
        self.version
    }

    pub(crate) fn tmux_binary(&self) -> &Path {
        &self.spec.tmux
    }

    pub fn socket(&self) -> &Socket {
        &self.spec.socket
    }

    /// `tmux -f <config> <socket>` with the caller's tmux variables removed: they name the
    /// tmux the daemon may run inside, never this server.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.spec.tmux);
        command
            .arg("-f")
            .arg(&self.spec.config_path)
            .args(self.spec.socket.arguments())
            .env_remove("TMUX")
            .env_remove("TMUX_PANE");
        command
    }

    fn run(&self, arguments: &[OsString]) -> Result<Vec<u8>, Error> {
        let output =
            self.command()
                .args(arguments)
                .output()
                .map_err(|source| Error::TmuxNotRunnable {
                    binary: self.spec.tmux.clone(),
                    source,
                })?;
        if !output.status.success() {
            return Err(Error::CommandFailed {
                arguments: arguments.to_vec(),
                status: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(output.stdout)
    }

    /// Runs a command whose output is one pane id.
    fn run_for_pane_id(&self, arguments: &[OsString]) -> Result<PaneId, Error> {
        let printed = self.run(arguments)?;
        let printed = String::from_utf8_lossy(&printed);
        PaneId::parse(printed.trim_end()).map_err(|_| Error::UnexpectedOutput {
            arguments: arguments.to_vec(),
            output: printed.into_owned(),
        })
    }

    /// Creates a detached session running `launch.command`, starting the server if needed,
    /// and returns the id of its one pane. Fails if the name is taken.
    pub fn new_session(&self, launch: &SessionLaunch) -> Result<PaneId, Error> {
        if launch.command.is_empty() {
            return Err(Error::InvalidArgument {
                what: "session command (empty)",
                value: String::new(),
            });
        }
        // tmux starts the pane in $HOME, and still succeeds, when the directory is missing.
        if !launch.working_dir.is_dir() {
            return Err(Error::InvalidArgument {
                what: "session working directory (not a directory)",
                value: launch.working_dir.display().to_string(),
            });
        }
        let mut arguments: Vec<OsString> = vec![
            "new-session".into(),
            "-d".into(),
            "-P".into(),
            "-F".into(),
            "#{pane_id}".into(),
            "-s".into(),
            launch.name.as_str().into(),
            "-c".into(),
            launch.working_dir.clone().into(),
            "-x".into(),
            launch.columns.to_string().into(),
            "-y".into(),
            launch.rows.to_string().into(),
        ];
        for (name, value) in &launch.environment {
            arguments.push("-e".into());
            arguments.push(environment_assignment(name, value)?);
        }
        arguments.push("--".into());
        arguments.extend(direct_command(&launch.command));
        self.run_for_pane_id(&arguments)
    }

    /// Every session on the server whose name a [`SessionName`] admits, sorted by name;
    /// empty when no server runs. A session with any other name is not Cockpit's (an agent
    /// can reach this server through its pane's `TMUX`), so it is left out.
    pub fn sessions(&self) -> Result<Vec<SessionName>, Error> {
        let arguments: Vec<OsString> = vec![
            "list-sessions".into(),
            "-F".into(),
            "#{session_name}".into(),
        ];
        let printed = match self.run(&arguments) {
            Ok(printed) => printed,
            Err(error) if is_no_server(&error) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut names: Vec<SessionName> = String::from_utf8_lossy(&printed)
            .lines()
            .filter_map(|name| SessionName::new(name).ok())
            .collect();
        names.sort();
        Ok(names)
    }

    /// The panes of every window of `session`.
    pub fn panes(&self, session: &SessionName) -> Result<Vec<PaneInfo>, Error> {
        let arguments: Vec<OsString> = vec![
            "list-panes".into(),
            "-s".into(),
            "-t".into(),
            session.target().into(),
            "-F".into(),
            "#{pane_id} #{pane_pid} #{pane_dead} #{pane_dead_status}".into(),
        ];
        let printed = self.run(&arguments)?;
        String::from_utf8_lossy(&printed)
            .lines()
            .map(|line| {
                parse_pane_line(line).ok_or_else(|| Error::UnexpectedOutput {
                    arguments: arguments.clone(),
                    output: line.to_string(),
                })
            })
            .collect()
    }

    /// Closes a session and its panes; tmux sends their processes `SIGHUP`. Ending the
    /// process tree for certain belongs to `cockpit-processes`.
    pub fn close_session(&self, session: &SessionName) -> Result<(), Error> {
        self.run(&["kill-session".into(), "-t".into(), session.target().into()])
            .map(drop)
    }

    pub fn close_pane(&self, pane: &PaneId) -> Result<(), Error> {
        self.run(&["kill-pane".into(), "-t".into(), pane.as_str().into()])
            .map(drop)
    }

    /// Splits a pane off `parent`, to its right (`side_by_side`) or below it, holding
    /// `percent` of its size, and returns the new pane's id.
    pub fn split_pane(
        &self,
        parent: &PaneId,
        split: Split,
        command: &[OsString],
    ) -> Result<PaneId, Error> {
        if command.is_empty() {
            return Err(Error::InvalidArgument {
                what: "pane command (empty)",
                value: String::new(),
            });
        }
        let percent = split.percent();
        if !(1..=99).contains(&percent) {
            return Err(Error::InvalidArgument {
                what: "split size (1-99 percent)",
                value: percent.to_string(),
            });
        }
        let mut arguments: Vec<OsString> = vec![
            "split-window".into(),
            split.direction_flag().into(),
            "-l".into(),
            format!("{percent}%").into(),
            "-t".into(),
            parent.as_str().into(),
            "-P".into(),
            "-F".into(),
            "#{pane_id}".into(),
            "--".into(),
        ];
        arguments.extend(direct_command(command));
        self.run_for_pane_id(&arguments)
    }

    /// The approval pane: a split under the viewer, focused so the human answers there.
    /// [`CONFIG`] keeps an exited pane, so its owner closes it with [`Server::close_pane`].
    pub fn split_approval_pane(
        &self,
        viewer: &PaneId,
        command: &[OsString],
    ) -> Result<PaneId, Error> {
        self.split_pane(viewer, Split::Below { percent: 40 }, command)
    }

    /// Nested attach: replaces whatever runs in the `viewer` pane with a tmux client of
    /// this server attached to `session`. `TMUX` is emptied for that client only, which is
    /// how tmux allows the nesting.
    pub fn show_in_viewer(&self, viewer: &PaneId, session: &SessionName) -> Result<(), Error> {
        let mut arguments: Vec<OsString> = vec![
            "respawn-pane".into(),
            "-k".into(),
            "-t".into(),
            viewer.as_str().into(),
            "-e".into(),
            "TMUX=".into(),
            "--".into(),
            self.spec.tmux.clone().into(),
        ];
        arguments.extend(self.spec.socket.arguments().map(OsString::from));
        arguments.extend([
            "attach-session".into(),
            "-t".into(),
            session.target().into(),
        ]);
        self.run(&arguments).map(drop)
    }

    /// A pane's visible screen plus up to `history_lines` lines above it, with its colors
    /// and attributes as escape sequences. Untrusted display data: never read state from it.
    pub fn capture(&self, pane: &PaneId, history_lines: u32) -> Result<Vec<u8>, Error> {
        self.run(&capture_arguments(pane, history_lines))
    }

    /// The server's process id, or `None` when no server runs.
    pub fn server_pid(&self) -> Result<Option<u32>, Error> {
        let arguments: Vec<OsString> = vec!["display-message".into(), "-p".into(), "#{pid}".into()];
        let printed = match self.run(&arguments) {
            Ok(printed) => printed,
            Err(error) if is_no_server(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        let printed = String::from_utf8_lossy(&printed);
        printed
            .trim_end()
            .parse()
            .map(Some)
            .map_err(|_| Error::UnexpectedOutput {
                arguments,
                output: printed.into_owned(),
            })
    }
}

/// Where [`Server::split_pane`] puts the new pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    Below { percent: u8 },
    SideBySide { percent: u8 },
}

impl Split {
    fn percent(self) -> u8 {
        match self {
            Split::Below { percent } | Split::SideBySide { percent } => percent,
        }
    }

    fn direction_flag(self) -> &'static str {
        match self {
            Split::Below { .. } => "-v",
            Split::SideBySide { .. } => "-h",
        }
    }
}

pub(crate) fn capture_arguments(pane: &PaneId, history_lines: u32) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = vec![
        "capture-pane".into(),
        "-p".into(),
        "-e".into(),
        "-t".into(),
        pane.as_str().into(),
    ];
    if history_lines > 0 {
        arguments.push("-S".into());
        arguments.push(format!("-{history_lines}").into());
    }
    arguments
}

/// tmux runs a lone argument through `sh -c` but several arguments directly, so a
/// one-word command is quoted for that shell and run with `exec`.
fn direct_command(command: &[OsString]) -> Vec<OsString> {
    match command {
        [program] => {
            let mut line = OsString::from("exec ");
            line.push(shell_quoted(program));
            vec![line]
        }
        _ => command.to_vec(),
    }
}

fn shell_quoted(word: &OsStr) -> OsString {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let mut quoted = vec![b'\''];
    for &byte in word.as_bytes() {
        if byte == b'\'' {
            quoted.extend_from_slice(b"'\\''");
        } else {
            quoted.push(byte);
        }
    }
    quoted.push(b'\'');
    OsString::from_vec(quoted)
}

fn environment_assignment(name: &str, value: &OsStr) -> Result<OsString, Error> {
    let is_valid = name
        .bytes()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if !is_valid {
        return Err(Error::InvalidArgument {
            what: "environment variable name",
            value: name.to_string(),
        });
    }
    let mut assignment = OsString::from(name);
    assignment.push("=");
    assignment.push(value);
    Ok(assignment)
}

fn parse_pane_line(line: &str) -> Option<PaneInfo> {
    let mut fields = line.split(' ');
    let id = PaneId::parse(fields.next()?).ok()?;
    let pid = match fields.next()? {
        "" => None,
        printed => Some(printed.parse().ok()?),
    };
    let state = match (fields.next()?, fields.next()?) {
        ("0", _) => PaneState::Running,
        ("1", status) => PaneState::Exited {
            status: status.parse().ok(),
        },
        _ => return None,
    };
    fields
        .next()
        .is_none()
        .then_some(PaneInfo { id, pid, state })
}

/// tmux's two messages for a socket with no server behind it: no file, or a stale one.
fn is_no_server(error: &Error) -> bool {
    matches!(error, Error::CommandFailed { stderr, .. }
        if stderr.starts_with("no server running") || stderr.starts_with("error connecting to"))
}

/// Writes `contents` to `path` with mode 0600, replacing it atomically.
fn write_private_file(path: &Path, contents: &str) -> Result<(), Error> {
    let mut staging = path.as_os_str().to_owned();
    staging.push(".new");
    let staging = PathBuf::from(staging);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&staging)
        .map_err(Error::io("create", &staging))?;
    file.write_all(contents.as_bytes())
        .map_err(Error::io("write", &staging))?;
    fs::rename(&staging, path).map_err(Error::io("replace", path))
}
