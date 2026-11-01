//! The Cockpits installed for this user, as `cockpit daemon install` writes them and
//! `cockpit doctor` finds them (#196, #134): each profile's unit, its daemon's socket, and a
//! legacy Cockpit's unit or command.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use cockpit_config::{Environment, Profile};

/// The line `cockpit daemon install` puts in every unit it writes: a `cockpit.service` without
/// it is the legacy Cockpit's.
const PROFILE_MARKER: &str = "X-Cockpit-Profile=";

/// The service manager that starts the daemon at login.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceManager {
    Systemd,
    Launchd,
}

impl ServiceManager {
    pub(crate) fn of_this_platform() -> Self {
        if cfg!(target_os = "macos") {
            Self::Launchd
        } else {
            Self::Systemd
        }
    }

    /// Where this user's units are: systemd's user unit directory, or `~/Library/LaunchAgents`.
    pub(crate) fn unit_dir(self, environment: &Environment) -> Result<PathBuf, String> {
        let home = environment.home_dir().map_err(|error| error.to_string())?;
        Ok(match self {
            Self::Systemd => match environment
                .xdg_config_home
                .as_deref()
                .map(Path::new)
                .filter(|dir| dir.is_absolute())
            {
                Some(config_home) => config_home.join("systemd/user"),
                None => home.join(".config/systemd/user"),
            },
            Self::Launchd => home.join("Library/LaunchAgents"),
        })
    }

    pub(crate) fn unit_file_name(self, profile: &Profile) -> String {
        match self {
            Self::Systemd => profile.systemd_unit(),
            Self::Launchd => format!("{}.plist", profile.launchd_label()),
        }
    }

    pub(crate) fn marker(profile: &Profile) -> String {
        format!("{PROFILE_MARKER}{}", profile.name())
    }
}

/// A unit `cockpit daemon install` wrote, and the profile it names.
#[derive(Debug)]
pub(crate) struct InstalledUnit {
    pub(crate) path: PathBuf,
    pub(crate) profile: Profile,
}

/// Every unit in `unit_dir` that names its profile, in file-name order.
pub(crate) fn installed_units(unit_dir: &Path) -> Vec<InstalledUnit> {
    let Ok(entries) = fs::read_dir(unit_dir) else {
        return Vec::new();
    };
    let mut units: Vec<InstalledUnit> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter_map(|path| {
            let profile = profile_marked_in(&fs::read_to_string(&path).ok()?)?;
            Some(InstalledUnit { path, profile })
        })
        .collect();
    units.sort_by(|left, right| left.path.cmp(&right.path));
    units
}

fn profile_marked_in(unit: &str) -> Option<Profile> {
    unit.lines()
        .find_map(|line| line.trim().strip_prefix(PROFILE_MARKER))
        .and_then(|name| Profile::parse(name.trim()).ok())
}

/// What shows a legacy Cockpit is installed: until the cutover (#66) the default profile is
/// its, so `cockpit daemon install` refuses it while any of these is found.
#[derive(Debug, Default)]
pub(crate) struct Legacy {
    /// `cockpit.service` without the profile line `cockpit daemon install` writes.
    pub(crate) unit: Option<PathBuf>,
    /// A `cockpit` on `PATH` that is not this program, even one this program shadows.
    pub(crate) command: Option<PathBuf>,
}

impl Legacy {
    pub(crate) fn find(unit_dir: &Path, path: Option<&OsStr>, this_program: &Path) -> Self {
        let unit = unit_dir.join(Profile::default_profile().systemd_unit());
        let unit = fs::read_to_string(&unit)
            .ok()
            .filter(|text| profile_marked_in(text).is_none())
            .map(|_| unit);
        let this_program = this_program.canonicalize().ok();
        let command = path
            .into_iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join(cockpit_config::DEFAULT_PROFILE))
            .find(|candidate| candidate.is_file() && candidate.canonicalize().ok() != this_program);
        Self { unit, command }
    }

    pub(crate) fn is_found(&self) -> bool {
        self.unit.is_some() || self.command.is_some()
    }

    /// `unit /x/cockpit.service; command /y/cockpit`, naming what was found.
    pub(crate) fn describe(&self) -> String {
        let mut found = Vec::new();
        if let Some(unit) = &self.unit {
            found.push(format!(
                "unit {} (not written by cockpit daemon install)",
                unit.display()
            ));
        }
        if let Some(command) = &self.command {
            found.push(format!("command {} on PATH", command.display()));
        }
        found.join("; ")
    }
}

/// A profile whose daemon socket exists beside the active one's, and whether it answers.
#[derive(Debug)]
pub(crate) struct RunningDaemon {
    pub(crate) socket: PathBuf,
    pub(crate) is_answering: bool,
}

/// Whether a daemon accepts connections on `socket` now.
pub(crate) fn is_answering(socket: &Path) -> bool {
    UnixStream::connect(socket).is_ok()
}

/// The daemon sockets of the other profiles: `<base>/cockpit[-<name>]/daemon.sock` beside the
/// active profile's socket directory, with the profile each belongs to.
pub(crate) fn other_daemons(
    environment: &Environment,
    active: &Profile,
) -> Vec<(Profile, RunningDaemon)> {
    let Some(base) = socket_base(environment) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut daemons: Vec<(Profile, RunningDaemon)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let profile = profile_of_dir_name(&entry.file_name())?;
            let socket = socket_in(&entry.path(), environment);
            (profile != *active && socket.exists()).then(|| {
                let is_answering = is_answering(&socket);
                (
                    profile,
                    RunningDaemon {
                        socket,
                        is_answering,
                    },
                )
            })
        })
        .collect();
    daemons.sort_by(|left, right| left.0.name().cmp(right.0.name()));
    daemons
}

/// `cockpit` is the default profile, `cockpit-<name>` profile `<name>`.
fn profile_of_dir_name(name: &OsStr) -> Option<Profile> {
    if name == cockpit_config::DEFAULT_PROFILE {
        return Some(Profile::default_profile());
    }
    Profile::of_command_name(name)
}

/// Where every profile's directory sits: `$XDG_RUNTIME_DIR`, or the state home when the
/// sockets fall back to `<state dir>/run`.
fn socket_base(environment: &Environment) -> Option<PathBuf> {
    let home = environment.home_dir().ok()?;
    let layout = cockpit_config::Layout::resolve(environment, &home);
    let socket_dir = layout.socket_dir().ok()?;
    match layout.runtime_dir.value {
        Some(runtime_dir) => runtime_dir.parent().map(Path::to_path_buf),
        None => socket_dir
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf),
    }
}

fn socket_in(profile_dir: &Path, environment: &Environment) -> PathBuf {
    let runtime_is_set = environment
        .xdg_runtime_dir
        .as_deref()
        .is_some_and(|dir| !dir.is_empty());
    let socket_dir = if runtime_is_set {
        profile_dir.to_path_buf()
    } else {
        profile_dir.join("run")
    };
    socket_dir.join(cockpit_config::DAEMON_SOCKET)
}
