//! `cockpit doctor [--project DIR]` (#50): the daemon's readiness report, one line per
//! requirement with its status and, when it is not available, the action that makes it so.
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

/// The daemon probes Docker within 10 seconds; the rest is reads.
const DOCTOR_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let project = parse(arguments)?;
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
        &cockpit_config::Environment::from_process(),
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
