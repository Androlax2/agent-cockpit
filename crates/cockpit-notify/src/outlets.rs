//! Where notices go: a desktop notification over the session's D-Bus and a short sound, each
//! through a program the desktop already has, so a host without them loses only the notice
//! (#62: a headless run works with notify absent).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use crate::event::Event;
use crate::notifier::Notice;

/// A program that has not finished by then is stopped: a stuck notification daemon or sound
/// server must not pile up children.
const PROGRAM_DEADLINE: Duration = Duration::from_secs(10);
/// The most notices one desktop notification lists; the rest are counted.
const LISTED_NOTICES: usize = 5;
const APP_NAME: &str = "Torlek";

/// What the notifier needs to know of the host to find its outlets.
#[derive(Debug, Clone, Default)]
pub struct Host {
    /// `PATH`.
    pub path: Option<OsString>,
    /// Whether a session bus is there to carry a notification: `DBUS_SESSION_BUS_ADDRESS` is
    /// set or `$XDG_RUNTIME_DIR/bus` exists.
    pub has_session_bus: bool,
    /// `XDG_DATA_DIRS`, where the freedesktop sound theme lives.
    pub data_dirs: Vec<PathBuf>,
}

impl Host {
    pub fn from_env() -> Host {
        let has_bus_address =
            std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|address| !address.is_empty());
        let has_bus_socket = std::env::var_os("XDG_RUNTIME_DIR")
            .is_some_and(|runtime| Path::new(&runtime).join("bus").exists());
        let data_dirs = match std::env::var_os("XDG_DATA_DIRS").filter(|dirs| !dirs.is_empty()) {
            Some(dirs) => std::env::split_paths(&dirs).collect(),
            None => vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ],
        };
        Host {
            path: std::env::var_os("PATH"),
            has_session_bus: has_bus_address || has_bus_socket,
            data_dirs,
        }
    }

    fn program(&self, name: &str) -> Option<PathBuf> {
        let path = self.path.as_ref()?;
        std::env::split_paths(path)
            .map(|dir| dir.join(name))
            .find(|candidate| is_executable(candidate))
    }
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// `notify-send`, which speaks the freedesktop notification protocol over the session bus.
#[derive(Debug, Clone)]
pub struct Desktop {
    program: PathBuf,
}

impl Desktop {
    /// The desktop outlet, or why there is none.
    pub fn find(host: &Host) -> Result<Desktop, String> {
        if !host.has_session_bus {
            return Err("no session bus (DBUS_SESSION_BUS_ADDRESS is unset)".to_owned());
        }
        let program = host
            .program("notify-send")
            .ok_or("notify-send is not on PATH")?;
        Ok(Desktop { program })
    }

    /// One notification for `notices` that landed together: the most urgent first, the rest
    /// listed below it.
    fn arguments(notices: &[&Notice]) -> Vec<String> {
        let mut ordered = notices.to_vec();
        ordered.sort_by_key(|notice| notice.event);
        let Some(first) = ordered.first() else {
            return Vec::new();
        };
        let urgency = if first.event == Event::Failure {
            "critical"
        } else {
            "normal"
        };
        let (summary, body) = if let [only] = ordered.as_slice() {
            (headline(only), only.detail.clone().unwrap_or_default())
        } else {
            let mut lines: Vec<String> = ordered
                .iter()
                .take(LISTED_NOTICES)
                .map(|notice| match &notice.detail {
                    Some(detail) => format!("{}: {detail}", headline(notice)),
                    None => headline(notice),
                })
                .collect();
            let unlisted = ordered.len().saturating_sub(LISTED_NOTICES);
            if unlisted > 0 {
                lines.push(format!("and {unlisted} more"));
            }
            let summary = format!("{} ({} more)", headline(first), ordered.len() - 1);
            (summary, lines.join("\n"))
        };
        let mut arguments = vec![
            format!("--app-name={APP_NAME}"),
            format!("--urgency={urgency}"),
            "--".to_owned(),
            summary,
        ];
        if !body.is_empty() {
            arguments.push(body);
        }
        arguments
    }
}

fn headline(notice: &Notice) -> String {
    let what = match notice.event {
        Event::Failure => "failed",
        Event::Hold => "waits for your permission",
        Event::Question => "needs your answer",
        Event::Quota => "hit its usage limit",
        Event::Finished => "finished",
    };
    format!("{} {what}", notice.name)
}

/// A short sound from the freedesktop sound theme: `canberra-gtk-play` plays it by name from
/// the owner's theme; without it, `pw-play` or `paplay` plays the theme's file.
#[derive(Debug, Clone)]
pub enum Sound {
    Canberra(PathBuf),
    Player {
        program: PathBuf,
        data_dirs: Vec<PathBuf>,
    },
}

impl Sound {
    /// The sound outlet, or why there is none.
    pub fn find(host: &Host) -> Result<Sound, String> {
        if let Some(canberra) = host.program("canberra-gtk-play") {
            return Ok(Sound::Canberra(canberra));
        }
        let player = ["pw-play", "paplay"]
            .into_iter()
            .find_map(|name| host.program(name))
            .ok_or("none of canberra-gtk-play, pw-play or paplay is on PATH")?;
        Ok(Sound::Player {
            program: player,
            data_dirs: host.data_dirs.clone(),
        })
    }

    /// The program and arguments that play `event`'s sound; `None` when its file is missing.
    fn command(&self, event: Event) -> Option<(PathBuf, Vec<OsString>)> {
        let id = sound_id(event);
        match self {
            Sound::Canberra(program) => Some((
                program.clone(),
                vec![
                    format!("--id={id}").into(),
                    format!("--description={APP_NAME}: {event}").into(),
                ],
            )),
            Sound::Player { program, data_dirs } => {
                let file = data_dirs
                    .iter()
                    .map(|dir| {
                        dir.join("sounds/freedesktop/stereo")
                            .join(format!("{id}.oga"))
                    })
                    .find(|file| file.is_file())?;
                Some((program.clone(), vec![file.into_os_string()]))
            }
        }
    }
}

/// One distinct sound per event, by its freedesktop sound-naming name.
pub fn sound_id(event: Event) -> &'static str {
    match event {
        Event::Failure => "dialog-error",
        Event::Hold => "dialog-warning",
        Event::Question => "message-new-instant",
        Event::Quota => "alarm-clock-elapsed",
        Event::Finished => "complete",
    }
}

/// The outlets found on this host and the events the owner turned on for each.
#[derive(Debug, Clone, Default)]
pub struct Outlets {
    pub desktop: Option<Desktop>,
    pub desktop_events: BTreeSet<Event>,
    pub sound: Option<Sound>,
    pub sound_events: BTreeSet<Event>,
}

impl Outlets {
    /// Tells the owner about `notices` that landed together: one desktop notification and one
    /// sound, the most urgent event's, each for the events turned on for it. Never waits for
    /// the programs, and a program that fails is only logged.
    pub fn tell(&self, notices: &[Notice]) {
        if let Some(desktop) = &self.desktop {
            let shown: Vec<&Notice> = notices
                .iter()
                .filter(|notice| self.desktop_events.contains(&notice.event))
                .collect();
            if !shown.is_empty() {
                let arguments = Desktop::arguments(&shown);
                start(
                    desktop.program.clone(),
                    arguments.into_iter().map(Into::into).collect(),
                );
            }
        }
        if let Some(sound) = &self.sound {
            let loudest = notices
                .iter()
                .map(|notice| notice.event)
                .filter(|event| self.sound_events.contains(event))
                .min();
            match loudest.map(|event| (event, sound.command(event))) {
                Some((_, Some((program, arguments)))) => start(program, arguments),
                Some((event, None)) => {
                    tracing::warn!(%event, "the sound theme has no file for this event");
                }
                None => {}
            }
        }
    }
}

/// Runs `program` in the background, stopped at [`PROGRAM_DEADLINE`].
fn start(program: PathBuf, arguments: Vec<OsString>) {
    let mut command = tokio::process::Command::new(&program);
    command
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    tokio::spawn(async move {
        match tokio::time::timeout(PROGRAM_DEADLINE, command.output()).await {
            Err(_) => tracing::warn!(program = %program.display(), "stopped: it took too long"),
            Ok(Err(error)) => tracing::warn!(program = %program.display(), %error, "cannot run"),
            Ok(Ok(output)) if !output.status.success() => tracing::warn!(
                program = %program.display(),
                status = %output.status,
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "failed"
            ),
            Ok(Ok(_)) => {}
        }
    });
}
