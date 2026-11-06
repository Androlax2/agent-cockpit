use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::Error;
use crate::names::{PaneId, SessionName};
use crate::version::{TmuxVersion, check_version};

/// Cockpit's own tmux configuration, passed with `-f` so the user's `~/.tmux.conf` never
/// applies. tmux reads it when a command starts the server, and [`Server::open`] applies it
/// again to a server already running. Every option is global and safe to set live.
///
/// `history_limit` is the lines of history a pane keeps, the server's largest use of memory
/// (#406). tmux 3.8 applies a changed limit to the panes already open as well, trimming
/// them at once; only the panes opened afterwards are relied on.
pub fn config(history_limit: u32) -> String {
    format!(
        "\
# Cockpit's private tmux server (cockpit-tmux). The user's ~/.tmux.conf is never read.
# Keep a pane that exited, with its last output and exit status, until Cockpit closes it.
set-option -g remain-on-exit on
# Agent transcripts are long; the default 2000 lines loses most of a build log.
set-option -g history-limit {history_limit}
# Anything tmux runs through a shell uses sh, never the user's login shell.
set-option -g default-shell /bin/sh
# A pane opened by hand (prefix c in the viewer) must not reach this server either.
set-option -g default-command 'unset TMUX TMUX_PANE; exec /bin/sh'
# The viewer is Cockpit's own screen: tmux's status bar only takes a row from it (#302).
set-option -g status off
# The TUI runs in a pane: an Escape key must not wait for the rest of a sequence.
set-option -s escape-time 0
# Copy out (#848): `on` answers a pane's OSC 52 query with the newest paste buffer, what the
# owner copied, maybe from another session, unless `get-clipboard` turns that off. A tmux
# without the option (3.6 has none) keeps `external`: copies stop at the viewer, nothing leaks.
set-option -sq get-clipboard off
%if \"#{{get-clipboard}}\"
# The viewer is a client of this server nested in a pane, so a copy reaches the owner's
# terminal only when tmux takes OSC 52 from that client too, which `external` refuses.
set-option -s set-clipboard on
%endif
# The nested client's terminal is a pane of this server (default-terminal tmux-256color, or
# screen): give it the clipboard feature by name rather than through its terminfo or tmux's
# detection. Indexed, so applying the file again replaces these entries instead of adding more.
set-option -s terminal-features[10] 'tmux*:clipboard'
set-option -s terminal-features[11] 'screen*:clipboard'
"
    )
}

/// The owner's image paste key as tmux names it: Ctrl+V, which Claude Code reads as its own
/// image paste and kitty sends as `C-v` (CMD+V where Command is mapped to Control).
const IMAGE_PASTE_KEY: &str = "C-v";
/// Set on the viewer pane only: the key binding's test for where the key was pressed.
const VIEWER_OPTION: &str = "@torlek-viewer";
/// [`Server::bind_image_paste`]'s script, beside the config.
const IMAGE_PASTE_SCRIPT: &str = "image-paste";

/// The paste buffer [`Server::paste`] fills and empties.
const PASTE_BUFFER: &str = "torlek-paste";

/// How long [`Server::close_session`] waits for a server it emptied to exit.
const SERVER_EXIT_WAIT: Duration = Duration::from_secs(5);

/// Where the tmux binary, the server socket and Cockpit's tmux configuration are.
#[derive(Clone, Debug)]
pub struct ServerSpec {
    /// A bare name is looked up on `PATH`.
    pub tmux: PathBuf,
    /// The server's socket, always named by path (`-S`): `-L` takes `$TMUX_TMPDIR` only when
    /// it exists and silently falls back to `/tmp`, where another installation's server
    /// lives (#845).
    pub socket: PathBuf,
    /// Written by [`Server::open`] with [`config`]; its directory must exist.
    pub config_path: PathBuf,
    /// The lines of history each pane keeps ([`config`]).
    pub history_limit: u32,
}

/// The parts of a new tmux session. `command` runs directly, never through a shell.
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

/// How the viewer's client attaches: typing reaches the session, or nothing typed does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Viewing {
    ReadWrite,
    ReadOnly,
}

/// A pane's size, its window's, whether the window is zoomed, whether the pane is the
/// window's active one and whether its input is off ([`Viewing::ReadOnly`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneGeometry {
    pub pane_width: u16,
    pub pane_height: u16,
    pub window_width: u16,
    pub window_height: u16,
    pub is_window_zoomed: bool,
    pub is_active: bool,
    pub is_input_off: bool,
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
    /// Checks the tmux version (the readiness check), writes [`config`] and applies it to
    /// the server if one already runs, so every option [`config`] sets takes effect on the
    /// sessions it holds (an option removed from it stays set until the server exits).
    /// Starts nothing: the first session starts the server.
    /// Why: `docs/decisions/2026-11-01-308-tmux-config-on-a-running-server.md`.
    pub fn open(spec: ServerSpec) -> Result<Server, Error> {
        let version = check_version(&spec.tmux)?;
        let server = Server { spec, version };
        server.check_running_version()?;
        write_private_file(&server.spec.config_path, &config(server.spec.history_limit))?;
        server.apply_config_to_running_server()?;
        Ok(server)
    }

    fn check_running_version(&self) -> Result<(), Error> {
        // Upgrading the binary does not replace the server that reaps existing agents.
        let printed = match self.run_if_server(&words(&["display-message", "-p", "#{version}"])) {
            Ok(Some(printed)) => printed,
            Ok(None) => return Ok(()),
            Err(error) if is_exiting_server(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
        let found = String::from_utf8_lossy(&printed).trim().to_string();
        let printed = format!("tmux {found}");
        if TmuxVersion::parse(&printed).is_some_and(|version| version.is_supported(&printed)) {
            return Ok(());
        }
        Err(Error::UnsupportedServerVersion {
            socket: self.spec.socket.clone(),
            found,
        })
    }

    /// `source-file` starts no server. An exiting one is left alone: the next server reads
    /// the new file as it starts.
    fn apply_config_to_running_server(&self) -> Result<(), Error> {
        let arguments = vec![
            OsString::from("source-file"),
            self.spec.config_path.clone().into_os_string(),
        ];
        match self.run_if_server(&arguments) {
            Ok(_) => Ok(()),
            Err(error) if is_exiting_server(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub fn version(&self) -> TmuxVersion {
        self.version
    }

    pub(crate) fn tmux_binary(&self) -> &Path {
        &self.spec.tmux
    }

    pub fn socket(&self) -> &Path {
        &self.spec.socket
    }

    /// `tmux -f <config> <socket>` with the caller's tmux variables removed: they name the
    /// tmux the daemon may run inside, never this server.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.spec.tmux);
        command
            .arg("-f")
            .arg(&self.spec.config_path)
            .arg("-S")
            .arg(&self.spec.socket)
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

    /// [`Server::run`], with `None` when no server runs.
    fn run_if_server(&self, arguments: &[OsString]) -> Result<Option<Vec<u8>>, Error> {
        match self.run(arguments) {
            Ok(printed) => Ok(Some(printed)),
            Err(error) if is_no_server(&error) => Ok(None),
            Err(error) => Err(error),
        }
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
    /// and returns the id of its one pane. Fails if the name is taken. When it reaches a
    /// server another caller's close just emptied, it waits until that server has exited
    /// and tries again, for up to `SERVER_EXIT_WAIT`: the exiting server created nothing.
    /// Why: `docs/decisions/2026-10-31-197-tmux-last-session-close.md` (the #202 section).
    pub fn new_session(&self, launch: &SessionLaunch) -> Result<PaneId, Error> {
        // tmux starts the pane in $HOME, and still succeeds, when the directory is missing.
        if !launch.working_dir.is_dir() {
            return Err(Error::InvalidArgument {
                what: "session working directory (not a directory)",
                value: launch.working_dir.display().to_string(),
            });
        }
        let mut arguments = words(&[
            "new-session",
            "-d",
            "-P",
            "-F",
            "#{pane_id}",
            "-s",
            launch.name.as_str(),
            "-x",
            &launch.columns.to_string(),
            "-y",
            &launch.rows.to_string(),
            "-c",
        ]);
        arguments.push(launch.working_dir.clone().into());
        for (name, value) in &launch.environment {
            arguments.push("-e".into());
            arguments.push(environment_assignment(name, value)?);
        }
        arguments.push("--".into());
        arguments.extend(pane_command(&launch.command)?);
        let deadline = Instant::now() + SERVER_EXIT_WAIT;
        loop {
            match self.run_for_pane_id(&arguments) {
                Err(error) if is_exiting_server(&error) && Instant::now() < deadline => {
                    self.wait_until_server_holds_a_session_or_exited()?;
                }
                created => return created,
            }
        }
    }

    /// Every session on the server whose name a [`SessionName`] admits, sorted by name;
    /// empty when no server runs. A session with any other name is not Cockpit's, so it is
    /// left out.
    pub fn sessions(&self) -> Result<Vec<SessionName>, Error> {
        let printed = match self.run_if_server(&words(&["list-sessions", "-F", "#{session_name}"]))
        {
            Ok(Some(printed)) => printed,
            Ok(None) => return Ok(Vec::new()),
            // A server killed a moment ago closes the connection as it exits: it holds nothing.
            Err(error) if is_exiting_server(&error) => return Ok(Vec::new()),
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
        let arguments = words(&[
            "list-panes",
            "-s",
            "-t",
            &session.target(),
            "-F",
            "#{pane_id} #{pane_pid} #{pane_dead} #{pane_dead_status}",
        ]);
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
    /// process tree for certain belongs to `cockpit-processes`. When it was the server's
    /// last session, returns only once the server has exited, so the next `new-session`
    /// starts a fresh one; fails after `SERVER_EXIT_WAIT` if it has not.
    /// Why: `docs/decisions/2026-10-31-197-tmux-last-session-close.md`.
    pub fn close_session(&self, session: &SessionName) -> Result<(), Error> {
        self.run(&words(&["kill-session", "-t", &session.target()]))?;
        self.wait_until_server_holds_a_session_or_exited()
    }

    /// tmux exits once its last session is gone, but not at once: a `new-session` that
    /// reaches the exiting server fails with "server exited unexpectedly".
    fn wait_until_server_holds_a_session_or_exited(&self) -> Result<(), Error> {
        let arguments = words(&["list-sessions", "-F", "#{session_id}"]);
        let deadline = Instant::now() + SERVER_EXIT_WAIT;
        loop {
            // An exiting server can answer with a failed command; that is still exiting.
            let answer = match self.run_if_server(&arguments) {
                Ok(None) => return Ok(()),
                Ok(Some(printed)) if !printed.is_empty() => return Ok(()),
                Ok(Some(_)) => "no sessions".to_string(),
                Err(error @ Error::CommandFailed { .. }) => error.to_string(),
                Err(error) => return Err(error),
            };
            if Instant::now() >= deadline {
                return Err(Error::EmptiedServerKeptRunning {
                    waited: SERVER_EXIT_WAIT,
                    last_answer: answer,
                });
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn close_pane(&self, pane: &PaneId) -> Result<(), Error> {
        self.run(&words(&["kill-pane", "-t", pane.as_str()]))
            .map(drop)
    }

    /// Gives `pane` the keys: it becomes its window's active pane, where an attached
    /// client's keys go.
    pub fn select_pane(&self, pane: &PaneId) -> Result<(), Error> {
        self.run(&words(&["select-pane", "-t", pane.as_str()]))
            .map(drop)
    }

    /// Whether a copy in a pane goes on to the owner's terminal (#848): the config turns
    /// `set-clipboard` on only where tmux can refuse panes' clipboard queries. Starts the
    /// server to read it when none runs.
    pub fn passes_copies_out(&self) -> Result<bool, Error> {
        let printed = self.run(&words(&[
            "start-server",
            ";",
            "show-options",
            "-sv",
            "set-clipboard",
        ]))?;
        Ok(String::from_utf8_lossy(&printed).trim_end() == "on")
    }

    /// The pane of `session` that has the keys: its current window's active pane.
    pub fn active_pane(&self, session: &SessionName) -> Result<PaneId, Error> {
        let arguments = words(&[
            "display-message",
            "-p",
            "-t",
            &session.target(),
            "#{pane_id}",
        ]);
        let printed = self.run(&arguments)?;
        let printed = String::from_utf8_lossy(&printed);
        PaneId::parse(printed.trim_end()).map_err(|_| Error::UnexpectedOutput {
            arguments,
            output: printed.into_owned(),
        })
    }

    /// Splits a pane off `parent`, to its right (`side_by_side`) or below it, holding
    /// `percent` of its size, and returns the new pane's id. Focus stays where it was, so
    /// keys keep going to the pane the human typed in.
    pub fn split_pane(
        &self,
        parent: &PaneId,
        split: Split,
        command: &[OsString],
    ) -> Result<PaneId, Error> {
        self.split(parent, split, command, false)
    }

    fn split(
        &self,
        parent: &PaneId,
        split: Split,
        command: &[OsString],
        takes_focus: bool,
    ) -> Result<PaneId, Error> {
        let percent = split.percent();
        if !(1..=99).contains(&percent) {
            return Err(Error::InvalidArgument {
                what: "split size (1-99 percent)",
                value: percent.to_string(),
            });
        }
        let mut arguments = words(&[
            "split-window",
            split.direction_flag(),
            "-l",
            &format!("{percent}%"),
            "-t",
            parent.as_str(),
            "-P",
            "-F",
            "#{pane_id}",
        ]);
        if !takes_focus {
            arguments.push("-d".into());
        }
        arguments.push("--".into());
        arguments.extend(pane_command(command)?);
        self.run_for_pane_id(&arguments)
    }

    /// The approval pane: a split under the viewer, focused so the human answers there.
    /// [`config`] keeps an exited pane, so its owner closes it with [`Server::close_pane`].
    pub fn split_approval_pane(
        &self,
        viewer: &PaneId,
        command: &[OsString],
    ) -> Result<PaneId, Error> {
        self.split(viewer, Split::Below { percent: 40 }, command, true)
    }

    /// Nested attach: replaces whatever runs in the `viewer` pane with a tmux client of
    /// this server attached to `session`. The client names the socket itself; like every
    /// pane command it runs without `TMUX`, which is how tmux allows the nesting.
    ///
    /// The session's window takes the viewer's size first: resized as the client attaches,
    /// tmux rewraps a line wider than the viewer and the client draws a screen that lost
    /// lines. `window-size` is unset again so the window follows the viewer afterwards.
    /// Why: `docs/decisions/2026-11-01-338-tmux-viewer-sized-before-attach.md`.
    ///
    /// [`Viewing::ReadOnly`] turns the viewer pane's input off (`select-pane -d`): tmux drops
    /// every key typed into it before the client sees one (#855). Not the client's own
    /// `read-only` flag: tmux then refuses every `send-keys` while that client is the most
    /// recently active, which would stop messages reaching the session shown.
    /// [`Viewing::ReadWrite`] turns it on again. Neither moves the keys to another pane.
    pub fn show_in_viewer(
        &self,
        viewer: &PaneId,
        session: &SessionName,
        viewing: Viewing,
    ) -> Result<(), Error> {
        let (columns, rows) = self.pane_size(viewer)?;
        let (columns, rows) = (columns.to_string(), rows.to_string());
        let window = session.target();
        let mut arguments = words(&[
            "select-pane",
            input_flag(viewing),
            "-t",
            viewer.as_str(),
            ";",
            "resize-window",
            "-t",
            &window,
            "-x",
            &columns,
            "-y",
            &rows,
            ";",
            "set-option",
            "-w",
            "-u",
            "-t",
            &window,
            "window-size",
            ";",
            "respawn-pane",
            "-k",
            "-t",
            viewer.as_str(),
            "--",
        ]);
        arguments.extend(pane_command(&self.attach_command(session))?);
        self.run(&arguments).map(drop)
    }

    /// Turns `pane`'s input on or off as [`Server::show_in_viewer`] does, without showing a
    /// session in it again.
    pub fn set_viewing(&self, pane: &PaneId, viewing: Viewing) -> Result<(), Error> {
        self.run(&words(&[
            "select-pane",
            input_flag(viewing),
            "-t",
            pane.as_str(),
        ]))
        .map(drop)
    }

    /// Where `pane` stands in its window, read in one tmux command.
    pub fn pane_geometry(&self, pane: &PaneId) -> Result<PaneGeometry, Error> {
        let arguments = words(&[
            "display-message",
            "-p",
            "-t",
            pane.as_str(),
            "#{pane_width} #{pane_height} #{window_width} #{window_height} \
             #{window_zoomed_flag} #{pane_active} #{pane_input_off}",
        ]);
        let printed = self.run(&arguments)?;
        let printed = String::from_utf8_lossy(&printed);
        let numbers: Option<Vec<u16>> = printed
            .split_whitespace()
            .map(|word| word.parse::<u16>().ok())
            .collect();
        match numbers.as_deref() {
            Some(
                &[
                    pane_width,
                    pane_height,
                    window_width,
                    window_height,
                    zoomed,
                    active,
                    input_off,
                ],
            ) if zoomed <= 1 && active <= 1 && input_off <= 1 => Ok(PaneGeometry {
                pane_width,
                pane_height,
                window_width,
                window_height,
                is_window_zoomed: zoomed == 1,
                is_active: active == 1,
                is_input_off: input_off == 1,
            }),
            _ => Err(Error::UnexpectedOutput {
                arguments,
                output: printed.into_owned(),
            }),
        }
    }

    /// Zooms `pane` when its window is not zoomed, unzooms its window when it is: tmux's
    /// `resize-pane -Z`. Zooming also makes `pane` the active one.
    pub fn toggle_zoom(&self, pane: &PaneId) -> Result<(), Error> {
        self.run(&words(&["resize-pane", "-Z", "-t", pane.as_str()]))
            .map(drop)
    }

    /// Lays the panes of `first`'s window out one above the other, in pane order, `first`
    /// on top and `rows` high; the others share the rest. Unzooms the window first.
    pub fn stack_panes(&self, first: &PaneId, rows: u16) -> Result<(), Error> {
        let rows = rows.to_string();
        self.run(&words(&[
            "select-layout",
            "-t",
            first.as_str(),
            "even-vertical",
            ";",
            "resize-pane",
            "-t",
            first.as_str(),
            "-y",
            &rows,
        ]))
        .map(drop)
    }

    /// Lays the panes of `first`'s window out side by side, in pane order, `first` on the
    /// left holding `percent` of the width. Unzooms the window first.
    pub fn place_side_by_side(&self, first: &PaneId, percent: u8) -> Result<(), Error> {
        if !(1..=99).contains(&percent) {
            return Err(Error::InvalidArgument {
                what: "pane width (1-99 percent)",
                value: percent.to_string(),
            });
        }
        let width = format!("{percent}%");
        self.run(&words(&[
            "select-layout",
            "-t",
            first.as_str(),
            "even-horizontal",
            ";",
            "resize-pane",
            "-t",
            first.as_str(),
            "-x",
            &width,
        ]))
        .map(drop)
    }

    /// A pane's width and height.
    fn pane_size(&self, pane: &PaneId) -> Result<(u16, u16), Error> {
        let arguments = words(&[
            "display-message",
            "-p",
            "-t",
            pane.as_str(),
            "#{pane_width} #{pane_height}",
        ]);
        let printed = self.run(&arguments)?;
        let printed = String::from_utf8_lossy(&printed);
        let number = |word: &str| word.parse::<u16>().ok().filter(|number| *number > 0);
        printed
            .trim_end()
            .split_once(' ')
            .and_then(|(columns, rows)| Some((number(columns)?, number(rows)?)))
            .ok_or_else(|| Error::UnexpectedOutput {
                arguments,
                output: printed.into_owned(),
            })
    }

    /// The words of a tmux client of this server attached to `session`, for a terminal to
    /// run: clients attach without linking this crate (report 05).
    pub fn attach_command(&self, session: &SessionName) -> Vec<OsString> {
        let mut client = vec![self.spec.tmux.clone().into_os_string()];
        client.extend(["-S".into(), self.spec.socket.clone().into_os_string()]);
        client.extend(words(&["attach-session", "-t", &session.target()]));
        client
    }

    /// Types `keys` into `pane`, in order, as one tmux command list: tmux parses the whole
    /// list before it runs any of it, and every command names the same pane, so a refused
    /// list or a gone pane (`CommandFailed`) types nothing. Text is typed
    /// literally (`send-keys -l`), never read as key names.
    pub fn send_keys(&self, pane: &PaneId, keys: &[Keystroke]) -> Result<(), Error> {
        let mut arguments = Vec::new();
        for key in keys {
            if !arguments.is_empty() {
                arguments.push(";".into());
            }
            arguments.extend(words(&["send-keys", "-t", pane.as_str()]));
            match key {
                Keystroke::Text(text) => {
                    arguments.extend(words(&["-l", "--"]));
                    arguments.push(literal_argument(text).into());
                }
                Keystroke::Enter => arguments.push("Enter".into()),
                Keystroke::Escape => arguments.push("Escape".into()),
                Keystroke::Control(letter) if letter.is_ascii_lowercase() => {
                    arguments.push(format!("C-{letter}").into());
                }
                Keystroke::Control(letter) => {
                    return Err(Error::InvalidArgument {
                        what: "control key (a lower-case ASCII letter)",
                        value: letter.to_string(),
                    });
                }
            }
        }
        if arguments.is_empty() {
            return Ok(());
        }
        self.run(&arguments).map(drop)
    }

    /// Binds CMD+V (`C-v`, which kitty sends and Claude Code reads as its image paste) on this
    /// server: pressed while `viewer` has the keys, it runs `command`, and anywhere else it
    /// reaches the pane's program as before. `command` handing a `C-v` back to the viewer
    /// ([`Server::send_keys`]) reaches the session shown there. The words run through a script
    /// beside the config, each quoted, its output discarded and its errors shown in the
    /// viewer. Why: `docs/decisions/2026-11-05-848-image-paste-key.md`.
    pub fn bind_image_paste(&self, viewer: &PaneId, command: &[OsString]) -> Result<(), Error> {
        let script = self.spec.config_path.with_file_name(IMAGE_PASTE_SCRIPT);
        let script_path = script
            .to_str()
            .filter(|path| path.chars().all(is_plain_path_character))
            .ok_or_else(|| Error::InvalidArgument {
                what: "image paste script path (letters, digits and / . _ - + only)",
                value: script.display().to_string(),
            })?;
        write_owner_file(&script, &image_paste_script(command)?, 0o700)?;
        let is_viewer = format!("#{{{VIEWER_OPTION}}}");
        let run_script = format!("run-shell -b {script_path}");
        let pass_key_on = format!("send-keys {IMAGE_PASTE_KEY}");
        self.run(&words(&[
            "set-option",
            "-p",
            "-t",
            viewer.as_str(),
            VIEWER_OPTION,
            "1",
            ";",
            "bind-key",
            "-n",
            IMAGE_PASTE_KEY,
            "if-shell",
            "-F",
            &is_viewer,
            &run_script,
            &pass_key_on,
        ]))
        .map(drop)
    }

    /// Pastes `text` into `pane` as one bracketed paste when its program asked for that, the
    /// way a terminal pastes: a program such as Claude Code then takes a pasted image path as
    /// an attachment. The paste buffer it goes through is deleted, so the owner's paste
    /// (prefix `]`) never types it again. Text with a control character is refused.
    pub fn paste(&self, pane: &PaneId, text: &str) -> Result<(), Error> {
        if text.chars().any(char::is_control) {
            return Err(Error::InvalidArgument {
                what: "pasted text (no control characters)",
                value: text.escape_debug().to_string(),
            });
        }
        let mut arguments = words(&["set-buffer", "-b", PASTE_BUFFER, "--"]);
        arguments.push(literal_argument(text).into());
        arguments.extend(words(&[
            ";",
            "paste-buffer",
            "-p",
            "-d",
            "-b",
            PASTE_BUFFER,
            "-t",
            pane.as_str(),
        ]));
        self.run(&arguments).map(drop)
    }

    /// A pane's visible screen plus up to `history_lines` lines above it, with its colors
    /// and attributes as escape sequences. Untrusted display data: never read state from it.
    pub fn capture(&self, pane: &PaneId, history_lines: u32) -> Result<Vec<u8>, Error> {
        self.run(&capture_arguments(pane, history_lines))
    }

    /// A pane's visible screen plus up to `history_lines` lines above it as plain text, wrapped
    /// lines joined: what an agent printed before it exited. Untrusted display data.
    pub fn capture_text(&self, pane: &PaneId, history_lines: u32) -> Result<String, Error> {
        let printed = self.run(&capture_arguments_as(pane, history_lines, "-J"))?;
        Ok(String::from_utf8_lossy(&printed).into_owned())
    }

    /// The server's process id, or `None` when no server runs.
    pub fn server_pid(&self) -> Result<Option<u32>, Error> {
        let arguments = words(&["display-message", "-p", "#{pid}"]);
        let Some(printed) = self.run_if_server(&arguments)? else {
            return Ok(None);
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

/// One step of what [`Server::send_keys`] types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keystroke<'text> {
    /// Typed character for character.
    Text(&'text str),
    Enter,
    Escape,
    /// The control chord of a lower-case ASCII letter, such as Ctrl-X.
    Control(char),
}

/// tmux reads an argument ending in `;` as the end of a command, and one ending in `\;` as
/// a literal `;`, wherever it appears: a text's final `;` is escaped so it is typed.
fn literal_argument(text: &str) -> String {
    match text.strip_suffix(';') {
        Some(rest) => format!("{rest}\\;"),
        None => text.to_owned(),
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
    capture_arguments_as(pane, history_lines, "-e")
}

/// `capture-pane` printing with `form`: `-e` keeps colors as escape sequences, `-J` joins
/// wrapped lines as plain text.
fn capture_arguments_as(pane: &PaneId, history_lines: u32, form: &str) -> Vec<OsString> {
    let mut arguments = words(&["capture-pane", "-p", form, "-t", pane.as_str()]);
    if history_lines > 0 {
        arguments.extend(words(&["-S", &format!("-{history_lines}")]));
    }
    arguments
}

pub(crate) fn words(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// `command` as the arguments of a fixed `sh` script that unsets `TMUX` and `TMUX_PANE`, so
/// a plain `tmux` in the pane never reaches this server.
/// Why: `docs/decisions/2026-10-31-30-tmux-observation.md`.
fn pane_command(command: &[OsString]) -> Result<Vec<OsString>, Error> {
    if command.is_empty() {
        return Err(Error::InvalidArgument {
            what: "pane command (empty)",
            value: String::new(),
        });
    }
    let mut wrapped = words(&["/bin/sh", "-c", "unset TMUX TMUX_PANE; exec \"$@\"", "sh"]);
    wrapped.extend_from_slice(command);
    Ok(wrapped)
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
    // Some tmux releases (CI's Ubuntu tmux; not 3.8) still print the ended process's pid,
    // which the system may reuse.
    let pid = pid.filter(|_| state == PaneState::Running);
    fields
        .next()
        .is_none()
        .then_some(PaneInfo { id, pid, state })
}

/// tmux's two messages for a socket with no server behind it: no file, or a stale one.
fn is_no_server(error: &Error) -> bool {
    matches!(error, Error::CommandFailed { stderr, .. }
        if stderr.starts_with("no server running on ")
            || (stderr.starts_with("error connecting to ")
                && (stderr.trim_end().ends_with(" (No such file or directory)")
                    || stderr.trim_end().ends_with(" (Connection refused)"))))
}

/// tmux's messages for a server that closed the connection as it exits: it ran nothing.
fn is_exiting_server(error: &Error) -> bool {
    matches!(error, Error::CommandFailed { stderr, .. }
        if stderr.starts_with("server exited unexpectedly") || stderr.starts_with("lost server"))
}

/// Writes `contents` to `path` with mode 0600, replacing it atomically.
fn write_private_file(path: &Path, contents: &str) -> Result<(), Error> {
    write_owner_file(path, contents, 0o600)
}

/// Replaces `path` with `contents` through a staging file, readable only by the owner with
/// `mode`.
fn write_owner_file(path: &Path, contents: &str, mode: u32) -> Result<(), Error> {
    let mut staging = path.as_os_str().to_owned();
    staging.push(".new");
    let staging = PathBuf::from(staging);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&staging)
        .map_err(Error::io("create", &staging))?;
    file.write_all(contents.as_bytes())
        .map_err(Error::io("write", &staging))?;
    fs::rename(&staging, path).map_err(Error::io("replace", path))
}

/// What tmux's command parser and `run-shell`'s formats leave alone in a path.
fn is_plain_path_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '/' | '.' | '_' | '-' | '+')
}

/// A shell script that runs `command`, every word single-quoted. Its output is dropped; its
/// errors go where its output would, so `run-shell` shows them.
fn image_paste_script(command: &[OsString]) -> Result<String, Error> {
    if command.is_empty() {
        return Err(Error::InvalidArgument {
            what: "image paste command (at least the program)",
            value: String::new(),
        });
    }
    let mut quoted = Vec::with_capacity(command.len());
    for word in command {
        let word = word.to_str().ok_or_else(|| Error::InvalidArgument {
            what: "image paste command word (UTF-8)",
            value: word.to_string_lossy().into_owned(),
        })?;
        quoted.push(format!("'{}'", word.replace('\'', "'\\''")));
    }
    Ok(format!(
        "#!/bin/sh\n# Written by Torlek for CMD+V in the viewer (#848).\nexec {} 2>&1 >/dev/null\n",
        quoted.join(" ")
    ))
}

/// `select-pane`'s flag for a pane's input: `-e` on, `-d` off.
fn input_flag(viewing: Viewing) -> &'static str {
    match viewing {
        Viewing::ReadWrite => "-e",
        Viewing::ReadOnly => "-d",
    }
}
