//! The root route of approved runs (#1387, option B): the root step `torlek setup root-run`
//! runs to install the root-run helper on the install script's path, and the readiness
//! report's `root route` line, which says whether sudo could reach the helper: installed and
//! root's alone, Torlek's askpass on, and a systemd that takes the helper's floor.

use std::path::Path;

use cockpit_protocol::Reply;
use cockpit_readiness::{Requirement, Status};
use cockpit_sandbox::{RootRunHelper, RootRunUnready};

use crate::readiness::Context;

const NAME: &str = "root route";

/// The system's `systemctl`, as the helper runs it.
const SYSTEMCTL: &str = "/usr/bin/systemctl";

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

/// What the `root route` line is assessed from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Facts {
    helper: Result<RootRunHelper, RootRunUnready>,
    /// Whether runs' sudo asks through Torlek's askpass (#1374).
    has_askpass: bool,
    /// Whether the system's systemd takes the helper's floor, or why not.
    systemd: Result<(), String>,
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
    let facts = tokio::task::spawn_blocking(move || Facts {
        helper: cockpit_sandbox::root_run_helper(Path::new("/"), &current_helper, 0),
        has_askpass,
        systemd: system_systemd(),
    })
    .await;
    match facts {
        Ok(facts) => assess(&facts),
        Err(error) => Requirement::needs(
            NAME,
            Status::Unknown,
            format!("whether sudo can reach the root-run helper is unknown: {error}"),
            "run `torlek doctor` again",
        ),
    }
}

fn system_systemd() -> Result<(), String> {
    let output = std::process::Command::new(SYSTEMCTL)
        .arg("--version")
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|error| format!("cannot run {SYSTEMCTL}: {error}"))?;
    cockpit_sandbox::systemd_takes_the_floor(&String::from_utf8_lossy(&output.stdout))
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
            return Requirement::needs(
                NAME,
                Status::MissingPrerequisite,
                format!(
                    "{} is not root's alone, so sudo never runs it: {reason}",
                    path.display()
                ),
                "make it and each folder above it owned by root and writable by root alone, \
                 or remove it: `torlek setup root-run --uninstall` removes the setup step's \
                 copy",
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
    if let Err(why) = &facts.systemd {
        return not_usable(why.clone()).with_advice("to use it, upgrade systemd to 257 or later");
    }
    let installed_by = if helper.is_packaged {
        "Torlek's package"
    } else {
        "`torlek setup root-run`"
    };
    Requirement::available(
        NAME,
        format!(
            "the root-run helper {installed_by} installed at {} is root's alone; sudo asks \
             your password each time; confined runs do not use it yet (#1387)",
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
            systemd: Ok(()),
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
            systemd: Err("systemd 255 is older than 257".to_owned()),
            ..ready()
        });

        assert_eq!(line.status, Status::Available);
        assert!(line.detail.contains("systemd 255"), "{}", line.detail);
    }
}
