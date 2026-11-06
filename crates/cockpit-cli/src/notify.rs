//! `torlek notify`: runs the `torlek-notify` installed beside `torlek` in this terminal, with
//! the daemon's socket, the profile's state directory and the owner's `[notify]` settings
//! (#62), so the notifier reads no configuration of its own (crate graph: notify ->
//! client/protocol/diagnostics only). The status file for bars goes beside the socket, in the
//! profile's runtime directory: it describes the running notifier, not lasting state.
//!
//! `torlek notify away on|off` sets the `away` hold (report 19) with a file in the same
//! directory, which the notifier checks at every notice: like the session it marks, it ends
//! at logout when that directory is `$XDG_RUNTIME_DIR`.

use std::ffi::OsString;
use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::daemon::fail;

const NOTIFY_BINARY: &str = "torlek-notify";
const STATUS_FILE: &str = "status.json";
const AWAY_FILE: &str = "away";

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let words: Vec<String> = arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => Ok(notifier_arguments().map_or_else(|code| code, |arguments| execute(&arguments))),
        ["--version"] => Ok(execute(&["--version".into()])),
        ["away", "on"] => Ok(set_away(true).unwrap_or_else(|code| code)),
        ["away", "off"] => Ok(set_away(false).unwrap_or_else(|code| code)),
        ["away", ..] => Err("use away on or away off".to_owned()),
        [extra, ..] => Err(format!("unknown argument {extra}")),
    }
}

/// The profile's configuration and runtime directory, or the exit code that reported why
/// there is none.
fn config_and_runtime() -> Result<(cockpit_config::EffectiveConfig, PathBuf), ExitCode> {
    let config = cockpit_config::load(&crate::profile::environment(), None, &[])
        .map_err(|error| fail(&error.to_string()))?;
    let runtime = config
        .layout
        .socket_dir()
        .map_err(|error| fail(&error.to_string()))?;
    Ok((config, runtime))
}

/// Needs only the runtime directory, so a configuration error cannot keep the owner away.
fn set_away(is_away: bool) -> Result<ExitCode, ExitCode> {
    let runtime = crate::agent::socket_dir()?;
    let away = runtime.join(AWAY_FILE);
    if is_away {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&runtime)
            .and_then(|()| {
                OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(false)
                    .mode(0o600)
                    .open(&away)
            })
            .map_err(|error| fail(&format!("cannot write {}: {error}", away.display())))?;
        outln!("away: on; no notification or sound until `torlek notify away off`");
        return Ok(ExitCode::SUCCESS);
    }
    match std::fs::remove_file(&away) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(fail(&format!("cannot remove {}: {error}", away.display())));
        }
    }
    outln!("away: off; a running notifier tells once, quietly, what still needs you");
    Ok(ExitCode::SUCCESS)
}

fn notifier_arguments() -> Result<Vec<OsString>, ExitCode> {
    let (config, runtime) = config_and_runtime()?;
    let mut arguments: Vec<OsString> = vec![
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
        "--away-file".into(),
        runtime.join(AWAY_FILE).into(),
    ];
    for (name, program) in &config.notify.holds {
        let mut hold = OsString::from(format!("{name}="));
        hold.push(&program.value);
        arguments.extend(["--hold".into(), hold]);
    }
    Ok(arguments)
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
