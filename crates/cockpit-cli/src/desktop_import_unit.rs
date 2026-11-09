//! The profile's desktop-session import as a systemd user unit (#1236): `torlek daemon install`
//! writes `torlek[-<name>]-desktop-import.service`, bound to `graphical-session.target`, which
//! runs `torlek desktop-session import` at each login. The daemon starts before the compositor
//! exists and never invents display variables (decision 2026-11-01-303), so without this unit
//! an owner who does not know the verb never gets image paste (#848). The unit is also part of
//! the daemon's: a daemon restart forgets the import (#307), and restarts the unit that
//! hands it over again.
//!
//! A host with no graphical session never reaches the target, so nothing runs there. Any file
//! of the unit's name that this installer did not write is refused.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use cockpit_config::Profile;

use crate::daemon_install::{service_manager, systemd_quoted, unit_variables, utf8, write_unit};
use crate::notifier_unit::is_active;

/// Names the profile in a unit this installer wrote. Not `X-Torlek-Profile=`: that line marks
/// a daemon unit, and this unit must not be listed as one.
const MARKER: &str = "X-Torlek-Desktop-Import-Profile=";

const GRAPHICAL_SESSION: &str = "graphical-session.target";

/// What an install does about the import unit, decided before anything is written.
#[derive(Debug)]
pub(crate) struct Plan {
    unit: PathBuf,
    unit_name: String,
}

impl Plan {
    /// Fails, before the install writes anything, when the unit's name is taken by a file this
    /// installer did not write for the profile.
    pub(crate) fn new(unit_dir: &Path, profile: &Profile) -> Result<Plan, String> {
        let unit_name = profile.desktop_import_systemd_unit();
        let unit = unit_dir.join(&unit_name);
        if !is_ours_or_absent(&unit, profile)? {
            return Err(format!(
                "{} exists and was not written by `torlek daemon install` for profile {profile}: \
                 remove it or choose another profile",
                unit.display()
            ));
        }
        Ok(Plan { unit, unit_name })
    }

    /// Writes and enables the unit; run after the daemon's own unit is in place.
    pub(crate) fn apply(&self, profile: &Profile, program: &Path) -> Result<(), String> {
        let text = unit_text(profile, program)?;
        let unit_dir = self
            .unit
            .parent()
            .ok_or_else(|| format!("{} has no directory", self.unit.display()))?;
        write_unit(unit_dir, &self.unit, &text)?;
        outln!("wrote {}", self.unit.display());
        service_manager("systemctl", &["--user", "daemon-reload"])?;
        service_manager("systemctl", &["--user", "enable", &self.unit_name])?;
        // Only a running desktop session has anything to import now; `restart` also hands over
        // the values to the daemon this install just restarted. Without a session the target
        // starts the unit at the next login.
        if is_active(
            "systemctl",
            &["--user", "is-active", "--quiet", GRAPHICAL_SESSION],
        ) {
            // Not waited for: the daemon this install just restarted may not listen yet, and
            // the unit then tries again by itself.
            service_manager(
                "systemctl",
                &["--user", "restart", "--no-block", &self.unit_name],
            )?;
            outln!(
                "the desktop session of profile {profile} is being imported by {}",
                self.unit_name
            );
        } else {
            outln!(
                "no graphical session is running: {} imports it at the next login",
                self.unit_name
            );
        }
        Ok(())
    }
}

fn is_ours_or_absent(unit: &Path, profile: &Profile) -> Result<bool, String> {
    let text = match fs::read_to_string(unit) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(format!("{}: {error}", unit.display())),
    };
    let marked = text
        .lines()
        .find_map(|line| line.trim().strip_prefix(MARKER))
        .and_then(|name| Profile::parse(name.trim()).ok());
    Ok(marked.as_ref() == Some(profile))
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
    let daemon_unit = profile.systemd_unit();
    // The daemon may not listen yet when the session starts: a failed import is tried again.
    Ok(format!(
        "# Written by `torlek daemon install`.\n\
         [Unit]\n\
         Description=Torlek desktop session import (profile {name})\n\
         {MARKER}{name}\n\
         PartOf={GRAPHICAL_SESSION} {daemon_unit}\n\
         After={GRAPHICAL_SESSION} {daemon_unit}\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         RemainAfterExit=yes\n\
         {environment}\
         ExecStart={program} --profile {name} desktop-session import\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy={GRAPHICAL_SESSION}\n"
    ))
}
