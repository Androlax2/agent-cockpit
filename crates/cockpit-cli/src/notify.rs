//! `torlek notify`: runs the `torlek-notify` installed beside `torlek` in this terminal, with
//! the daemon's socket, the profile's state directory and the owner's `[notify]` settings
//! (#62), so the notifier reads no configuration of its own (crate graph: notify ->
//! client/protocol/diagnostics only). The status file for bars goes beside the socket, in the
//! profile's runtime directory: it describes the running notifier, not lasting state.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::ExitCode;

use crate::daemon::fail;

const NOTIFY_BINARY: &str = "torlek-notify";
const STATUS_FILE: &str = "status.json";

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    if arguments == ["--version"] {
        return Ok(execute(&["--version".into()]));
    }
    if let Some(extra) = arguments.first() {
        return Err(format!("unknown argument {}", extra.to_string_lossy()));
    }
    match notifier_arguments() {
        Ok(arguments) => Ok(execute(&arguments)),
        Err(code) => Ok(code),
    }
}

fn notifier_arguments() -> Result<Vec<OsString>, ExitCode> {
    let config = cockpit_config::load(&crate::profile::environment(), None, &[])
        .map_err(|error| fail(&error.to_string()))?;
    let runtime = config
        .layout
        .socket_dir()
        .map_err(|error| fail(&error.to_string()))?;
    Ok(vec![
        "--socket".into(),
        runtime.join(cockpit_config::DAEMON_SOCKET).into(),
        "--state-dir".into(),
        config.layout.state_dir.value.into(),
        "--desktop".into(),
        config.notify.desktop.to_argument().into(),
        "--sound".into(),
        config.notify.sound.to_argument().into(),
        "--status-file".into(),
        runtime.join(STATUS_FILE).into(),
    ])
}

/// Replaces this process with the notifier; returns only when it cannot start.
fn execute(arguments: &[OsString]) -> ExitCode {
    let notify = match crate::profile::sibling_program(NOTIFY_BINARY) {
        Ok(notify) => notify,
        Err(problem) => return fail(&problem),
    };
    let error = std::process::Command::new(&notify).args(arguments).exec();
    fail(&format!("cannot run {}: {error}", notify.display()))
}
