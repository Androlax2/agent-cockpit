//! What the removed sandbox engines (Greywall and Greyproxy, #828) left in a profile: the
//! `torlek-greyproxy-<profile>` login service and the folders they kept in the profile's
//! state directory. `torlek doctor` reports them; only the owner's explicit
//! `torlek doctor --remove-old-sandbox` stops the service and deletes them. A unit of that
//! name without the marker the old release wrote in it is not Torlek's and is left alone.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use cockpit_config::{Environment, Profile};
use cockpit_protocol::{ReadinessStatusView, RequirementView};

use crate::daemon_install::service_manager as run;
use crate::installations::ServiceManager;

/// The state folders the engines kept: the proxy's configuration, CA and data, the verified
/// engine releases, and the per-launch settings files.
const STATE_FOLDERS: [&str; 3] = ["greyproxy", "engines", "sandbox"];

pub(crate) const REMOVE_FLAG: &str = "--remove-old-sandbox";

/// The leftovers of one profile, as found on disk.
pub(crate) struct Leftovers {
    profile: Profile,
    manager: ServiceManager,
    /// The service's unit file, when the old release's is still installed.
    unit: Option<PathBuf>,
    folders: Vec<PathBuf>,
}

impl Leftovers {
    /// Reads the profile's unit directory and state directory; changes nothing.
    pub(crate) fn find(environment: &Environment) -> Result<Leftovers, String> {
        let home = environment.home_dir().map_err(|error| error.to_string())?;
        let state_dir = cockpit_config::Layout::resolve(environment, &home)
            .state_dir
            .value;
        let manager = ServiceManager::of_this_platform();
        let profile = environment.profile.value.clone();
        let unit = manager
            .unit_dir(environment)?
            .join(unit_file_name(manager, &profile));
        Ok(Leftovers {
            unit: carries_marker(&unit, &profile).then_some(unit),
            folders: STATE_FOLDERS
                .iter()
                .map(|name| state_dir.join(name))
                .filter(|folder| exists(folder))
                .collect(),
            profile,
            manager,
        })
    }

    fn is_empty(&self) -> bool {
        self.unit.is_none() && self.folders.is_empty()
    }

    /// The doctor's line for them, or none when nothing is left.
    pub(crate) fn requirement(&self) -> Option<RequirementView> {
        if self.is_empty() {
            return None;
        }
        let found: Vec<String> = self
            .unit
            .iter()
            .chain(&self.folders)
            .map(|path| path.display().to_string())
            .collect();
        Some(RequirementView {
            name: "old sandbox".to_owned(),
            status: ReadinessStatusView::Available,
            detail: format!(
                "the removed Greywall and Greyproxy engines left {}; nothing this release \
                 starts uses them, but a session an earlier release launched in Greywall needs \
                 them until it ends",
                found.join(", ")
            ),
            next: Some(format!(
                "once no such session runs, `{} doctor {REMOVE_FLAG}` stops \
                 the service and deletes them",
                crate::profile::command()
            )),
        })
    }

    /// Stops and removes the service, then deletes the folders, saying each step. Stops at
    /// the first step that fails, leaving the rest in place.
    pub(crate) fn remove(self) -> ExitCode {
        if self.is_empty() {
            outln!(
                "profile {}: no old sandbox service or state to remove",
                self.profile
            );
            return ExitCode::SUCCESS;
        }
        match self.remove_each() {
            Ok(()) => ExitCode::SUCCESS,
            Err(problem) => {
                eprintln!("torlek doctor {REMOVE_FLAG}: {problem}");
                ExitCode::FAILURE
            }
        }
    }

    fn remove_each(&self) -> Result<(), String> {
        if let Some(unit) = &self.unit {
            self.stop_service(unit).map_err(|problem| {
                format!(
                    "{problem}; stop the service yourself and delete {}, then run this again \
                     to delete the folders",
                    unit.display()
                )
            })?;
            fs::remove_file(unit).map_err(|error| format!("{}: {error}", unit.display()))?;
            if self.manager == ServiceManager::Systemd {
                run("systemctl", &["--user", "daemon-reload"])?;
            }
            outln!("removed {}", unit.display());
        }
        for folder in &self.folders {
            let metadata = fs::symlink_metadata(folder)
                .map_err(|error| format!("{}: {error}", folder.display()))?;
            if !metadata.is_dir() {
                return Err(format!(
                    "{} is not a folder; left in place for you to inspect",
                    folder.display()
                ));
            }
            fs::remove_dir_all(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
            outln!("removed {}", folder.display());
        }
        Ok(())
    }

    fn stop_service(&self, unit: &Path) -> Result<(), String> {
        match self.manager {
            ServiceManager::Systemd => run(
                "systemctl",
                &[
                    "--user",
                    "disable",
                    "--now",
                    &unit_file_name(self.manager, &self.profile),
                ],
            ),
            ServiceManager::Launchd => {
                let uid = fs::metadata(unit)
                    .map_err(|error| format!("{}: {error}", unit.display()))?
                    .uid();
                let target = format!("gui/{uid}/{}", launchd_label(&self.profile));
                let loaded = Command::new("launchctl")
                    .args(["print", &target])
                    .output()
                    .map_err(|error| format!("cannot run launchctl print {target}: {error}"))?;
                if loaded.status.success() {
                    run("launchctl", &["bootout", &target])?;
                }
                Ok(())
            }
        }
    }
}

fn unit_file_name(manager: ServiceManager, profile: &Profile) -> String {
    match manager {
        ServiceManager::Systemd => format!("torlek-greyproxy-{profile}.service"),
        ServiceManager::Launchd => format!("{}.plist", launchd_label(profile)),
    }
}

fn launchd_label(profile: &Profile) -> String {
    format!("io.github.androlax2.torlek.greyproxy.{profile}")
}

/// The marker line the old release wrote near the top of either unit kind.
fn carries_marker(unit: &Path, profile: &Profile) -> bool {
    let marker = format!("Torlek managed Greyproxy, profile {profile}");
    let marked = [format!("# {marker}"), format!("<!-- {marker} -->")];
    fs::read_to_string(unit).is_ok_and(|text| {
        text.lines()
            .take(3)
            .any(|line| marked.iter().any(|mark| mark == line))
    })
}

/// Present as anything, a dangling link included.
fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
