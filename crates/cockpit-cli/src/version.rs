//! `cockpit version [--all]`: this binary's version and source revision; `--all` adds the
//! active profile and build (#196), the release bundle's manifest installed beside it
//! (report 13, #96), and what the running daemon says about itself: its release, the
//! database schema it holds, and the bundle each live session started with (#64).

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, ErrorCode, Reply, VersionReportView};

/// Long enough for a busy daemon, short enough that `version --all` does not hang on one
/// that is stuck.
const DAEMON_DEADLINE: Duration = Duration::from_secs(10);

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let shows_all = match arguments {
        [] => false,
        [flag] if flag == "--all" => true,
        _ => return Err("version takes no argument but --all".to_string()),
    };
    outln!("{}", line());
    if !shows_all {
        return Ok(ExitCode::SUCCESS);
    }
    let profile = crate::profile::active();
    outln!("profile {} ({})", profile.value, profile.source);
    outln!("build {}", crate::profile::build().as_str());
    let manifest_path = match crate::profile::this_program().and_then(|program| {
        cockpit_config::release_manifest(&program).map_err(|error| error.to_string())
    }) {
        Ok(path) => path,
        Err(problem) => {
            eprintln!("torlek: cannot locate this binary's bundle: {problem}");
            return Ok(ExitCode::FAILURE);
        }
    };
    match fs::read_to_string(&manifest_path) {
        Ok(manifest) => {
            outln!("bundle manifest {}:", manifest_path.display());
            out!("{manifest}");
        }
        Err(error) if error.kind() == ErrorKind::NotFound => outln!(
            "bundle manifest: none at {} (a build outside a release bundle)",
            manifest_path.display()
        ),
        Err(error) => {
            eprintln!("torlek: {}: {error}", manifest_path.display());
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(print_running_daemon())
}

/// What the daemon that answers the owner's socket says, or why there is nothing to say.
/// A daemon that is not running is a normal state, not a failure; one that answers
/// unexpectedly is.
fn print_running_daemon() -> ExitCode {
    if crate::agent::is_in_a_session() {
        outln!(
            "daemon: not asked from inside a session; run `torlek version --all` in your own terminal"
        );
        return ExitCode::SUCCESS;
    }
    let socket = match crate::agent::locate_socket_dir() {
        Ok(directory) => directory.join(cockpit_config::DAEMON_SOCKET),
        Err(problem) => {
            outln!("daemon: not reachable: {problem}");
            return ExitCode::SUCCESS;
        }
    };
    match ask_daemon(&socket) {
        Ok(Answer::Report(report)) => {
            print_report(&report);
            ExitCode::SUCCESS
        }
        Ok(Answer::BeforeVersionReports { release, manifest }) => {
            outln!(
                "daemon: release {release}, instruction manifest {manifest}; it predates version \
                 reports, so its schema and sessions are not shown"
            );
            print_restart_hint(&release);
            ExitCode::SUCCESS
        }
        Err(ClientError::Connect { source, .. }) => {
            outln!(
                "daemon: not running (cannot reach {}: {source})",
                socket.display()
            );
            outln!("schema: not checked; the daemon opens the database");
            ExitCode::SUCCESS
        }
        Err(error) => crate::daemon::failed(&error),
    }
}

enum Answer {
    Report(Box<VersionReportView>),
    /// A daemon from before `version-report` existed; the instruction catalog every daemon
    /// since the bundle shipped (#49) answers still names its release and manifest.
    BeforeVersionReports {
        release: String,
        manifest: String,
    },
}

fn ask_daemon(socket: &std::path::Path) -> Result<Answer, ClientError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|source| ClientError::Connect {
            socket_path: socket.to_owned(),
            source,
        })?;
    runtime.block_on(async {
        let connection = Connection::open(socket, DAEMON_DEADLINE).await?;
        match connection
            .request(Command::VersionReport, DAEMON_DEADLINE)
            .await
        {
            Ok(Reply::VersionReport { report }) => Ok(Answer::Report(report)),
            Err(ClientError::Daemon(error)) if error.code == ErrorCode::UnknownCommand => {
                match connection
                    .request(Command::InstructionCatalog, DAEMON_DEADLINE)
                    .await?
                {
                    Reply::InstructionCatalog {
                        version, manifest, ..
                    } => Ok(Answer::BeforeVersionReports {
                        release: version,
                        manifest,
                    }),
                    other => Err(ClientError::UnexpectedReply(other)),
                }
            }
            Ok(other) => Err(ClientError::UnexpectedReply(other)),
            Err(error) => Err(error),
        }
    })
}

fn print_report(report: &VersionReportView) {
    let revision = report
        .source_revision
        .as_deref()
        .map_or_else(String::new, |revision| format!(" (revision {revision})"));
    let protocols: Vec<String> = report
        .protocol_versions
        .iter()
        .map(u32::to_string)
        .collect();
    outln!(
        "daemon: release {}{revision}, protocol {}",
        report.release,
        protocols.join(", ")
    );
    outln!(
        "daemon: instruction manifest {}",
        report.instruction_manifest
    );
    print_restart_hint(&report.release);
    print_schema(report);
    print_sessions(report);
}

/// The daemon runs until it is stopped, so an install of another release changes nothing it
/// does (report 13): say so.
fn print_restart_hint(running_release: &str) {
    let installed = env!("CARGO_PKG_VERSION");
    if running_release != installed {
        outln!(
            "daemon: runs {running_release}, not the installed {installed}; stop and start it \
             to use the installed release"
        );
    }
}

fn print_schema(report: &VersionReportView) {
    let newer: Vec<String> = report
        .schema
        .iter()
        .filter(|owner| owner.applied > owner.known)
        .map(|owner| {
            format!(
                "{} is at {} where the daemon knows {}",
                owner.owner, owner.applied, owner.known
            )
        })
        .collect();
    if newer.is_empty() {
        outln!(
            "schema: compatible, {} capabilities at versions the daemon knows",
            report.schema.len()
        );
    } else {
        outln!("schema: INCOMPATIBLE, {}", newer.join("; "));
    }
}

fn print_sessions(report: &VersionReportView) {
    if report.sessions.is_empty() {
        outln!("sessions: none running");
        return;
    }
    for live in &report.sessions {
        let state = &live.state;
        match &live.bundle {
            None => outln!("session {} ({state}): bundle not recorded", live.session),
            Some(bundle) => {
                let is_current = bundle.release == report.release
                    && bundle.instruction_manifest == report.instruction_manifest;
                outln!(
                    "session {} ({state}): helpers {}, instructions {}: {}",
                    live.session,
                    bundle.release,
                    bundle.instruction_manifest,
                    if is_current {
                        "the running daemon's bundle"
                    } else {
                        "previous bundle"
                    }
                );
            }
        }
    }
}

/// `torlek VERSION (revision REVISION)`: this binary's first line, which a support bundle's
/// version manifest also names as the installed CLI.
pub(crate) fn line() -> String {
    format!("torlek {} ({})", env!("CARGO_PKG_VERSION"), revision())
}

/// The release workflow sets `TORLEK_SOURCE_REVISION` while compiling; rustc tracks it, so a
/// changed revision rebuilds.
fn revision() -> String {
    match option_env!("TORLEK_SOURCE_REVISION") {
        Some(revision) => format!("revision {revision}"),
        None => "revision unknown: not a release build".to_string(),
    }
}
