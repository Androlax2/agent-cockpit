//! The root route of approved runs (#1387, option B): the root step `torlek setup root-run`
//! runs to install the root-run helper on the install script's path, the readiness report's
//! `root route` line, which says whether sudo could reach the helper: installed and root's
//! alone, Torlek's askpass on, and a systemd that takes the helper's floor, and how a
//! confined run's root route finds the helper at each `sudo` ([`locate_helper`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cockpit_protocol::Reply;
use cockpit_readiness::{Requirement, Status};
use cockpit_sandbox::{RootRunHelper, RootRunUnready};

use crate::readiness::Context;

const NAME: &str = "root route";

/// The system's `systemctl`, as the helper runs it.
const SYSTEMCTL: &str = "/usr/bin/systemctl";

/// How long `systemctl --version` may take before the report says systemd is unknown.
const SYSTEMCTL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// The root step `torlek setup root-run` runs with sudo: it copies `helper`, the
/// `torlek-sandbox` this daemon runs, which readiness compares the installed copy with.
pub(crate) fn helper_step(helper: &Path, uninstall: bool) -> Reply {
    Reply::RootRunHelperStep {
        script: if uninstall {
            cockpit_sandbox::root_run_removal_script()
        } else {
            cockpit_sandbox::root_run_setup_script()
        },
        helper: helper.display().to_string(),
    }
}

/// Finds the root-run helper sudo is to run as it is at each `sudo` of a confined run, so a
/// helper installed or removed while the daemon runs counts at once. `current_helper` is the
/// `torlek-sandbox` this daemon runs.
pub(crate) fn locate_helper(current_helper: PathBuf) -> cockpit_approvals::LocateHelper {
    Arc::new(move || {
        cockpit_sandbox::root_run_helper(Path::new("/"), &current_helper, 0)
            .map(|helper| helper.path)
            .map_err(|unready| why_unready(&unready))
    })
}

/// Why a `sudo` of a confined run reaches no root part, in one line.
fn why_unready(unready: &RootRunUnready) -> String {
    match unready {
        RootRunUnready::NotInstalled => "no root-run helper is installed: install Torlek's \
                                         package or run `torlek setup root-run`"
            .to_owned(),
        RootRunUnready::OutOfDate { path } => format!(
            "the root-run helper at {} is not this release's: run `torlek setup root-run` again",
            path.display()
        ),
        RootRunUnready::Untrusted { path, reason } => format!(
            "the root-run helper at {} is not root's alone: {reason}",
            path.display()
        ),
        RootRunUnready::NoCurrentHelper { reason } => format!(
            "this daemon's own torlek-sandbox cannot be read ({reason}): restart the daemon"
        ),
        RootRunUnready::Unknown(reason) => {
            format!("the root-run helper cannot be read: {reason}")
        }
    }
}

/// What the `root route` line is assessed from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Facts {
    helper: Result<RootRunHelper, RootRunUnready>,
    /// Whether runs' sudo asks through Torlek's askpass (#1374).
    has_askpass: bool,
    systemd: Systemd,
}

/// The system's systemd, as the helper's floor needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Systemd {
    TakesTheFloor,
    TooOld(u32),
    /// `systemctl --version` did not run, or printed no version.
    Unknown(String),
}

/// The `root route` line. Only confined runs need a route to root, so it says nothing more
/// when runs are unconfined or refused.
pub(crate) async fn requirement(context: &Context) -> Requirement {
    if context.run_settings.confinement == cockpit_approvals::RunConfinement::Off {
        return Requirement::available(
            NAME,
            "not used: approved runs are unconfined, so their sudo is the one in your shell",
        );
    }
    if context.processes.confinement_unavailable().await.is_some() {
        return Requirement::available(
            NAME,
            "not used: every approved run is refused, as the `confined runs` line says",
        );
    }
    let current_helper = context.containers.relay_program.clone();
    let has_askpass = context.run_settings.sudo.is_some();
    let helper = tokio::task::spawn_blocking(move || {
        cockpit_sandbox::root_run_helper(Path::new("/"), &current_helper, 0)
    })
    .await
    .unwrap_or_else(|error| Err(RootRunUnready::Unknown(error.to_string())));
    assess(&Facts {
        helper,
        has_askpass,
        systemd: system_systemd().await,
    })
}

async fn system_systemd() -> Systemd {
    let mut command = tokio::process::Command::new(SYSTEMCTL);
    command
        .arg("--version")
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(SYSTEMCTL_DEADLINE, command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Systemd::Unknown(format!("cannot run {SYSTEMCTL}: {error}")),
        Err(_) => {
            return Systemd::Unknown(format!(
                "{SYSTEMCTL} --version did not answer in {} s",
                SYSTEMCTL_DEADLINE.as_secs()
            ));
        }
    };
    let printed = String::from_utf8_lossy(&output.stdout);
    match cockpit_sandbox::systemd_version(&printed) {
        Some(version) if version >= cockpit_sandbox::ROOT_RUN_MIN_SYSTEMD => Systemd::TakesTheFloor,
        Some(version) => Systemd::TooOld(version),
        None => Systemd::Unknown(format!(
            "{SYSTEMCTL} --version ({}) printed no version: {printed:?}",
            output.status
        )),
    }
}

/// A missing or out-of-date helper, no askpass or an old systemd leave confined runs without
/// sudo, which is optional, so none blocks; a helper more than root could have changed does,
/// so the owner looks at it.
fn assess(facts: &Facts) -> Requirement {
    let not_usable =
        |why: String| Requirement::available(NAME, format!("not usable, optional: {why}"));
    let helper = match &facts.helper {
        Ok(helper) => helper,
        Err(RootRunUnready::NotInstalled) => {
            return not_usable(format!(
                "no root-run helper: neither Torlek's package ({}) nor `torlek setup root-run` \
                 ({}) installed one",
                cockpit_sandbox::PACKAGED_ROOT_RUN_HELPER,
                cockpit_sandbox::SET_UP_ROOT_RUN_HELPER
            ))
            .with_advice(
                "to use it, install Torlek's Arch or Debian package, or run `torlek setup \
                 root-run`, which asks for your password once",
            );
        }
        Err(RootRunUnready::OutOfDate { path }) => {
            return not_usable(format!(
                "the root-run helper `torlek setup root-run` installed at {} is not this \
                 release's",
                path.display()
            ))
            .with_advice("run `torlek setup root-run` again");
        }
        Err(RootRunUnready::Untrusted { path, reason }) => {
            let fix = "make it and each folder above it owned by root and writable by root alone";
            let remove = if path == Path::new(cockpit_sandbox::SET_UP_ROOT_RUN_HELPER) {
                "or remove it with `torlek setup root-run --uninstall`"
            } else {
                "or reinstall Torlek's package"
            };
            return Requirement::needs(
                NAME,
                Status::MissingPrerequisite,
                format!(
                    "{} is not root's alone, so sudo never runs it: {reason}",
                    path.display()
                ),
                format!("{fix}, {remove}"),
            );
        }
        Err(RootRunUnready::NoCurrentHelper { reason }) => {
            return Requirement::needs(
                NAME,
                Status::Unknown,
                format!(
                    "whether the root-run helper is this release's is unknown: the daemon's own \
                     torlek-sandbox is gone ({reason})"
                ),
                "restart the daemon, so it runs the release you installed, then run `torlek \
                 doctor` again",
            );
        }
        Err(RootRunUnready::Unknown(reason)) => {
            return Requirement::needs(
                NAME,
                Status::Unknown,
                format!("whether the root-run helper is installed is unknown: {reason}"),
                "make the helper's folders readable, then run `torlek doctor` again",
            );
        }
    };
    if !facts.has_askpass {
        return not_usable(
            "Torlek's askpass for sudo is off: /usr/bin/sudo is not setuid root, or this is \
             not Linux; the daemon's log says why"
                .to_owned(),
        )
        .with_advice("to use it, install sudo at /usr/bin/sudo, then restart the daemon");
    }
    match &facts.systemd {
        Systemd::TakesTheFloor => {}
        Systemd::TooOld(version) => {
            return not_usable(format!(
                "systemd {version} is older than {}, which the helper's units need \
                 (PrivatePIDs=)",
                cockpit_sandbox::ROOT_RUN_MIN_SYSTEMD
            ))
            .with_advice("to use it, upgrade systemd to 257 or later");
        }
        Systemd::Unknown(reason) => {
            return Requirement::needs(
                NAME,
                Status::Unknown,
                format!("whether systemd takes the helper's units is unknown: {reason}"),
                "make /usr/bin/systemctl answer `--version`, then run `torlek doctor` again",
            );
        }
    }
    let installed_by = if helper.is_packaged {
        "Torlek's package"
    } else {
        "`torlek setup root-run`"
    };
    Requirement::available(
        NAME,
        format!(
            "the root-run helper {installed_by} installed at {} is root's alone; a `sudo \
             COMMAND` written in a confined run's command line runs through it, sudo asking \
             your password each time",
            helper.path.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn ready() -> Facts {
        Facts {
            helper: Ok(RootRunHelper {
                path: PathBuf::from(cockpit_sandbox::PACKAGED_ROOT_RUN_HELPER),
                is_packaged: true,
            }),
            has_askpass: true,
            systemd: Systemd::TakesTheFloor,
        }
    }

    #[test]
    fn an_installed_helper_with_askpass_and_systemd_257_is_available() {
        let line = assess(&ready());

        assert_eq!(line.status, Status::Available);
        assert!(line.detail.contains("Torlek's package"), "{}", line.detail);
        assert!(
            line.detail
                .contains(cockpit_sandbox::PACKAGED_ROOT_RUN_HELPER),
            "{}",
            line.detail
        );
        assert_eq!(line.next, None);
    }

    #[test]
    fn a_missing_helper_names_the_package_and_the_setup_step_without_blocking() {
        let line = assess(&Facts {
            helper: Err(RootRunUnready::NotInstalled),
            ..ready()
        });

        assert_eq!(line.status, Status::Available);
        assert!(line.detail.starts_with("not usable"), "{}", line.detail);
        assert!(line.next.unwrap().contains("torlek setup root-run"));
    }

    #[test]
    fn an_out_of_date_copy_asks_for_the_setup_step_again() {
        let line = assess(&Facts {
            helper: Err(RootRunUnready::OutOfDate {
                path: PathBuf::from(cockpit_sandbox::SET_UP_ROOT_RUN_HELPER),
            }),
            ..ready()
        });

        assert_eq!(line.status, Status::Available);
        assert!(
            line.detail.contains("not this release's"),
            "{}",
            line.detail
        );
        assert_eq!(
            line.next.as_deref(),
            Some("run `torlek setup root-run` again")
        );
    }

    #[test]
    fn a_helper_more_than_root_could_change_blocks() {
        let line = assess(&Facts {
            helper: Err(RootRunUnready::Untrusted {
                path: PathBuf::from(cockpit_sandbox::SET_UP_ROOT_RUN_HELPER),
                reason: "/usr/local/lib is writable by its group or other users (mode 2775)"
                    .to_owned(),
            }),
            ..ready()
        });

        assert_eq!(line.status, Status::MissingPrerequisite);
        assert!(line.detail.contains("mode 2775"), "{}", line.detail);
    }

    #[test]
    fn an_unreadable_install_is_unknown() {
        let line = assess(&Facts {
            helper: Err(RootRunUnready::Unknown("cannot read /usr/lib".to_owned())),
            ..ready()
        });

        assert_eq!(line.status, Status::Unknown);
    }

    #[test]
    fn without_askpass_the_helper_is_not_usable() {
        let line = assess(&Facts {
            has_askpass: false,
            ..ready()
        });

        assert_eq!(line.status, Status::Available);
        assert!(line.detail.contains("askpass"), "{}", line.detail);
    }

    #[test]
    fn an_older_systemd_is_named() {
        let line = assess(&Facts {
            systemd: Systemd::TooOld(255),
            ..ready()
        });

        assert_eq!(line.status, Status::Available);
        assert!(line.detail.contains("systemd 255"), "{}", line.detail);
        assert!(line.next.unwrap().contains("upgrade systemd"));
    }

    #[test]
    fn a_systemctl_that_printed_no_version_is_unknown_not_too_old() {
        let line = assess(&Facts {
            systemd: Systemd::Unknown("printed no version".to_owned()),
            ..ready()
        });

        assert_eq!(line.status, Status::Unknown);
        assert!(!line.next.unwrap().contains("upgrade"));
    }

    #[test]
    fn an_untrusted_packaged_helper_is_not_sent_to_the_setup_steps_removal() {
        let line = assess(&Facts {
            helper: Err(RootRunUnready::Untrusted {
                path: PathBuf::from(cockpit_sandbox::PACKAGED_ROOT_RUN_HELPER),
                reason: "/usr/lib/torlek/bin is writable by its group".to_owned(),
            }),
            ..ready()
        });

        let next = line.next.unwrap();
        assert!(next.contains("reinstall Torlek's package"), "{next}");
        assert!(!next.contains("--uninstall"), "{next}");
    }

    #[test]
    fn a_gone_daemon_helper_asks_for_a_restart() {
        let line = assess(&Facts {
            helper: Err(RootRunUnready::NoCurrentHelper {
                reason: "cannot read /old/torlek-sandbox".to_owned(),
            }),
            ..ready()
        });

        assert_eq!(line.status, Status::Unknown);
        assert!(line.next.unwrap().contains("restart the daemon"));
    }
}
