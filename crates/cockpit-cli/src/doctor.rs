//! `cockpit doctor [--project DIR]`: the active profile and the other installations seen
//! (#196, #134), then the daemon's readiness report (#50), one line per requirement with its
//! status and, when it is not available, the action that makes it so.
//! Exits 1 when a requirement blocks (missing-prerequisite or unsupported).
//!
//! Without a daemon the report is what this command can read alone: the configuration, the
//! daemon itself as a missing prerequisite, and the host as unknown. Assessing the rest needs
//! the facts the daemon gathers at its start; a second copy of that here would drift.

use std::ffi::OsString;
use std::process::ExitCode;

use cockpit_client::ClientError;
use cockpit_protocol::{Command, ReadinessStatusView, Reply, RequirementView};

use crate::daemon;
use crate::installations::{Legacy, ServiceManager, installed_units, is_answering, other_daemons};

/// The daemon probes Docker within 10 seconds; the rest is reads.
const DOCTOR_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let project = parse(arguments)?;
    if let Err(problem) = print_installations() {
        return Ok(daemon::fail(&problem));
    }
    let requirements = match ask_daemon(project.clone()) {
        Ok(requirements) => requirements,
        Err(Unanswered::Unreachable(reason)) => offline(project.as_deref(), reason),
        Err(Unanswered::Failed(code)) => return Ok(code),
    };
    for requirement in &requirements {
        render(requirement);
    }
    let blocks = requirements
        .iter()
        .any(|requirement| requirement.status.blocks());
    Ok(if blocks {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn parse(arguments: &[OsString]) -> Result<Option<String>, String> {
    let words = daemon::utf8_words(arguments)?;
    match words[..] {
        [] => Ok(None),
        ["--project", dir] if !dir.starts_with("--") => {
            let dir = daemon::absolute_dir(dir)?;
            if !std::path::Path::new(&dir).is_dir() {
                return Err(format!("--project {dir}: not a directory"));
            }
            Ok(Some(dir))
        }
        ["--project"] => Err("--project needs a value".to_owned()),
        _ => Err(format!(
            "doctor takes only --project DIR, not {}",
            words.join(" ")
        )),
    }
}

enum Unanswered {
    /// No daemon listens on the socket: the report is assembled here instead.
    Unreachable(String),
    /// A daemon answered with an error, already printed.
    Failed(ExitCode),
}

fn ask_daemon(project: Option<String>) -> Result<Vec<RequirementView>, Unanswered> {
    let answer = daemon::exchange(Command::Readiness { project }, DOCTOR_DEADLINE)
        .map_err(Unanswered::Failed)?;
    match answer {
        Ok(Reply::Readiness { requirements }) => Ok(requirements),
        Ok(other) => Err(Unanswered::Failed(daemon::unexpected(&other))),
        Err(error @ ClientError::Connect { .. }) => Err(Unanswered::Unreachable(error.to_string())),
        Err(error) => Err(Unanswered::Failed(daemon::failed(&error))),
    }
}

fn offline(project: Option<&str>, unreachable: String) -> Vec<RequirementView> {
    let config = match cockpit_config::load(
        &crate::profile::environment(),
        project.map(std::path::Path::new),
        &[],
    ) {
        Ok(config) => {
            let mut files = vec![config_file("owner", &config.user_file)];
            files.extend(
                config
                    .project_file
                    .as_ref()
                    .map(|file| config_file("project", file)),
            );
            RequirementView {
                name: "config".to_owned(),
                status: ReadinessStatusView::Available,
                detail: files.join(", "),
                next: None,
            }
        }
        Err(error) => RequirementView {
            name: "config".to_owned(),
            status: ReadinessStatusView::MissingPrerequisite,
            detail: error.to_string(),
            next: Some(
                "fix the file and line named here; `cockpit config show` reads it again".to_owned(),
            ),
        },
    };
    vec![
        config,
        RequirementView {
            name: "daemon".to_owned(),
            status: ReadinessStatusView::MissingPrerequisite,
            detail: unreachable,
            next: Some("start `cockpit-daemon`, then run `cockpit doctor` again".to_owned()),
        },
        RequirementView {
            name: "host".to_owned(),
            status: ReadinessStatusView::Unknown,
            detail: "tmux, the sandbox, providers, resource controls and the project are \
                     assessed by the daemon"
                .to_owned(),
            next: Some("start the daemon".to_owned()),
        },
    ]
}

fn config_file(role: &str, file: &cockpit_config::ConfigFile) -> String {
    let presence = if file.is_present { "read" } else { "absent" };
    format!("{role} {} ({presence})", file.path.display())
}

fn render(requirement: &RequirementView) {
    outln!(
        "{:<20}  {}: {}",
        requirement.status.as_str(),
        requirement.name,
        requirement.detail
    );
    if let Some(next) = &requirement.next {
        outln!("{:<20}    next: {next}", "");
    }
}

/// Which Cockpit this command reaches and which others it sees (#196, #134): the active profile
/// and build, its directories, daemon socket, tmux server and unit, then every other profile's
/// unit and daemon, and a legacy Cockpit. Reads files and connects to sockets only.
fn print_installations() -> Result<(), String> {
    let profile = crate::profile::active();
    let environment = crate::profile::environment();
    let home = environment.home_dir().map_err(|error| error.to_string())?;
    let layout = cockpit_config::Layout::resolve(&environment, &home);
    let manager = ServiceManager::of_this_platform();
    let unit_dir = manager.unit_dir(&environment)?;
    let units = installed_units(&unit_dir);

    outln!("profile {} ({})", profile.value, profile.source);
    outln!("build {}", crate::profile::build().as_str());
    outln!("config dir {}", layout.config_dir.value.display());
    outln!("state dir {}", layout.state_dir.value.display());
    let socket = layout
        .socket_dir()
        .map_err(|error| error.to_string())?
        .join(cockpit_config::DAEMON_SOCKET);
    outln!(
        "daemon {}: {}",
        socket.display(),
        if is_answering(&socket) {
            "answering"
        } else {
            "not running"
        }
    );
    outln!("tmux server {}", profile.value.qualified_name());
    let own_unit = units.iter().find(|unit| unit.profile == profile.value);
    match own_unit {
        Some(unit) => outln!("unit {}: installed", unit.path.display()),
        None => outln!(
            "unit {}: not installed (`cockpit --profile {} daemon install`)",
            manager.unit_file_name(&profile.value),
            profile.value
        ),
    }

    let mut others = Vec::new();
    for unit in units.iter().filter(|unit| unit.profile != profile.value) {
        others.push(format!(
            "profile {}: unit {}",
            unit.profile,
            unit.path.display()
        ));
    }
    for (other, daemon) in other_daemons(&environment, &profile.value) {
        others.push(format!(
            "profile {other}: daemon {}: {}",
            daemon.socket.display(),
            if daemon.is_answering {
                "answering"
            } else {
                "not running"
            }
        ));
    }
    let program = crate::profile::this_program()?;
    let legacy = Legacy::find(&unit_dir, std::env::var_os("PATH").as_deref(), &program);
    if legacy.is_found() {
        others.push(format!("legacy cockpit: {}", legacy.describe()));
    }
    if others.is_empty() {
        outln!("other installations: none");
    } else {
        outln!("other installations:");
        for other in others {
            outln!("- {other}");
        }
    }
    Ok(())
}
