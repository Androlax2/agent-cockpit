//! The profile's notifier as a systemd user unit (#1126): `torlek daemon install` writes
//! `torlek[-<name>]-notify.service`, bound to `graphical-session.target`, so `torlek notify`
//! starts with the owner's desktop session and stops with it. A host with no graphical session
//! never reaches the target, so nothing runs there. `[notify] autostart = false` is the one
//! opt-out: the install then removes the unit it wrote and writes none. `torlek doctor` reads
//! the unit and the notifier's status file back.
//!
//! Before this unit existed the owner could have written a unit of the same name and target by
//! hand; that one is replaced, and the install says so. Any other file of the name is refused.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use cockpit_config::Profile;

use crate::daemon_install::{service_manager, systemd_quoted, unit_variables, utf8, write_unit};

/// Names the profile in a unit this installer wrote. Not `X-Torlek-Profile=`: that line marks
/// a daemon unit, and a notifier unit must not be listed as one.
const NOTIFY_MARKER: &str = "X-Torlek-Notify-Profile=";

const GRAPHICAL_SESSION: &str = "graphical-session.target";

/// The notifier rewrites its status file every five seconds; one that is older than this is
/// no longer running.
const STATUS_FILE_FRESH_FOR: Duration = Duration::from_secs(15);

/// What the unit's path holds before an install.
#[derive(Debug, PartialEq, Eq)]
enum Existing {
    Absent,
    /// Written by `torlek daemon install` for this profile.
    Ours,
    /// A unit of this name that runs `torlek notify`, made by hand: the stopgap this unit
    /// replaces.
    HandMade,
    Foreign,
}

/// What an install does about the notifier, decided before anything is written.
#[derive(Debug)]
pub(crate) struct Plan {
    unit: PathBuf,
    unit_name: String,
    existing: Existing,
    autostart: bool,
}

impl Plan {
    /// Fails, before the install writes anything, when the unit's name is taken by a file that
    /// is neither this installer's nor the hand-made notifier unit and the install would write it.
    pub(crate) fn new(unit_dir: &Path, profile: &Profile, autostart: bool) -> Result<Plan, String> {
        let unit_name = profile.notify_systemd_unit();
        let unit = unit_dir.join(&unit_name);
        let existing = classify(&unit, profile)?;
        if autostart && existing == Existing::Foreign {
            return Err(format!(
                "{} exists and is neither written by `torlek daemon install` nor a unit that runs \
                 `torlek notify`: remove it, or set `autostart = false` under `[notify]`",
                unit.display()
            ));
        }
        Ok(Plan {
            unit,
            unit_name,
            existing,
            autostart,
        })
    }

    /// Writes and enables the unit, or removes it under the opt-out. Run after the daemon's
    /// own unit is in place.
    pub(crate) fn apply(&self, profile: &Profile, program: &Path) -> Result<(), String> {
        if self.autostart {
            self.install(profile, program)
        } else {
            self.remove()
        }
    }

    fn install(&self, profile: &Profile, program: &Path) -> Result<(), String> {
        let text = unit_text(profile, program)?;
        let unit_dir = self
            .unit
            .parent()
            .ok_or_else(|| format!("{} has no directory", self.unit.display()))?;
        if self.existing == Existing::HandMade {
            outln!(
                "replacing the hand-made {}: same name and target, now written by `torlek daemon install`",
                self.unit.display()
            );
        }
        write_unit(unit_dir, &self.unit, &text)?;
        outln!("wrote {}", self.unit.display());
        service_manager("systemctl", &["--user", "daemon-reload"])?;
        service_manager("systemctl", &["--user", "enable", &self.unit_name])?;
        // Only a running desktop session gets the notifier now; `restart` also replaces a
        // running one. Without a session the target starts it at the next login.
        if is_active(
            "systemctl",
            &["--user", "is-active", "--quiet", GRAPHICAL_SESSION],
        ) {
            service_manager("systemctl", &["--user", "restart", &self.unit_name])?;
            outln!(
                "the notifier of profile {profile} runs as {}",
                self.unit_name
            );
        } else {
            outln!(
                "no graphical session is running: {} starts with the next one",
                self.unit_name
            );
        }
        Ok(())
    }

    fn remove(&self) -> Result<(), String> {
        match self.existing {
            Existing::Absent => {
                outln!("no notifier unit installed: [notify] autostart is false");
            }
            Existing::Ours => {
                service_manager(
                    "systemctl",
                    &["--user", "disable", "--now", &self.unit_name],
                )?;
                fs::remove_file(&self.unit)
                    .map_err(|error| format!("{}: {error}", self.unit.display()))?;
                service_manager("systemctl", &["--user", "daemon-reload"])?;
                outln!(
                    "removed {}: [notify] autostart is false",
                    self.unit.display()
                );
            }
            Existing::HandMade | Existing::Foreign => {
                outln!(
                    "left {} alone: it was not written by `torlek daemon install`",
                    self.unit.display()
                );
            }
        }
        Ok(())
    }
}

fn classify(unit: &Path, profile: &Profile) -> Result<Existing, String> {
    let text = match fs::read_to_string(unit) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Existing::Absent),
        Err(error) => return Err(format!("{}: {error}", unit.display())),
    };
    let marked = text
        .lines()
        .find_map(|line| line.trim().strip_prefix(NOTIFY_MARKER))
        .and_then(|name| Profile::parse(name.trim()).ok());
    Ok(if marked.as_ref() == Some(profile) {
        Existing::Ours
    } else if marked.is_none() && runs_torlek_notify(&text) {
        Existing::HandMade
    } else {
        Existing::Foreign
    })
}

/// Whether an `ExecStart=` line runs `torlek notify` (or `torlek-<name> notify`, or
/// `torlek-notify`): the shape of the unit the owner made by hand.
fn runs_torlek_notify(unit: &str) -> bool {
    unit.lines()
        .filter_map(|line| line.trim().strip_prefix("ExecStart="))
        .any(|command| {
            let mut words = command
                .trim_start_matches(['-', '@', ':', '+', '!'])
                .split_whitespace()
                .map(|word| word.trim_matches('"'));
            let Some(program) = words.next() else {
                return false;
            };
            let name = Path::new(program)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            name == "torlek-notify"
                || (name.starts_with("torlek") && words.any(|word| word == "notify"))
        })
}

fn unit_text(profile: &Profile, program: &Path) -> Result<String, String> {
    let program = systemd_quoted(&utf8(program)?);
    let environment: String = unit_variables()?
        .iter()
        .map(|(name, value)| {
            format!(
                "Environment={}\n",
                systemd_quoted(&format!("{name}={value}"))
            )
        })
        .collect();
    let name = profile.name();
    Ok(format!(
        "# Written by `torlek daemon install`; `[notify] autostart = false` removes it.\n\
         [Unit]\n\
         Description=Torlek desktop notifications (profile {name})\n\
         {NOTIFY_MARKER}{name}\n\
         PartOf={GRAPHICAL_SESSION}\n\
         After={GRAPHICAL_SESSION}\n\
         \n\
         [Service]\n\
         Type=simple\n\
         {environment}\
         ExecStart={program} --profile {name} notify\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy={GRAPHICAL_SESSION}\n"
    ))
}

fn is_active(program: &str, arguments: &[&str]) -> bool {
    Command::new(program)
        .args(arguments)
        .status()
        .is_ok_and(|status| status.success())
}

/// What `torlek doctor` reports about the notifier.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum State {
    /// Its status file was written within the last few seconds, by a unit or by hand.
    Running,
    InstalledNotRunning,
    NotInstalled,
}

/// A notifier is running when it rewrote its status file lately; the unit's presence says
/// whether it will start with the desktop session.
pub(crate) fn state(unit: &Path, status_file: &Path) -> State {
    if is_fresh(status_file) {
        State::Running
    } else if unit.exists() {
        State::InstalledNotRunning
    } else {
        State::NotInstalled
    }
}

fn is_fresh(status_file: &Path) -> bool {
    fs::metadata(status_file)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age <= STATUS_FILE_FRESH_FOR)
}
