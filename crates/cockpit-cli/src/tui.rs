//! `cockpit tui`: asks the daemon for the manager session (a column running `cockpit-tui`
//! and a viewer beside it) and attaches this terminal to it. The CLI resolves the socket, the
//! profile and its state directory so the TUI needs no configuration of its own (crate graph:
//! tui -> client/protocol/diagnostics only).

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::ExitCode;

use cockpit_protocol::{Command, Reply};

use crate::daemon::{CONTROL_DEADLINE, fail, request, unexpected};

const TUI_BINARY: &str = "cockpit-tui";

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    if let Some(extra) = arguments.first() {
        return Err(format!("unknown argument {}", extra.to_string_lossy()));
    }
    let command = match manager_open() {
        Ok(command) => command,
        Err(code) => return Ok(code),
    };
    let reply = match request(command, CONTROL_DEADLINE) {
        Ok(reply) => reply,
        Err(code) => return Ok(code),
    };
    let Reply::Manager { attach, .. } = reply else {
        return Ok(unexpected(&reply));
    };
    let Some((program, rest)) = attach.split_first() else {
        return Ok(fail("the daemon answered no command to attach with"));
    };
    let error = std::process::Command::new(program).args(rest).exec();
    Ok(fail(&format!("cannot run {program}: {error}")))
}

fn manager_open() -> Result<Command, ExitCode> {
    let tui = tui_binary().map_err(|problem| fail(&problem))?;
    let socket = crate::agent::socket_dir()?.join(cockpit_config::DAEMON_SOCKET);
    let environment = crate::profile::environment();
    let home = environment
        .home_dir()
        .map_err(|error| fail(&error.to_string()))?;
    let state_dir = cockpit_config::Layout::resolve(&environment, &home)
        .state_dir
        .value;
    let dir = std::env::current_dir()
        .map_err(|error| fail(&format!("cannot read the working directory: {error}")))?;
    Ok(Command::ManagerOpen {
        dir: utf8(&dir)?,
        column: vec![
            tui.clone(),
            "--column".to_owned(),
            "--socket".to_owned(),
            utf8(&socket)?,
            "--profile".to_owned(),
            environment.profile.value.name().to_owned(),
            "--state-dir".to_owned(),
            utf8(&state_dir)?,
        ],
        viewer: vec![tui, "--viewer".to_owned()],
    })
}

/// `cockpit-tui` installed beside this `cockpit`; never one found on PATH, which could be
/// another build.
fn tui_binary() -> Result<String, String> {
    let cockpit = std::env::current_exe()
        .map_err(|error| format!("cannot find this program's path: {error}"))?;
    let tui = cockpit.with_file_name(TUI_BINARY);
    if !tui.is_file() {
        return Err(format!(
            "{} is missing: install {TUI_BINARY} beside cockpit",
            tui.display()
        ));
    }
    tui.to_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{}: not UTF-8", tui.display()))
}

fn utf8(path: &Path) -> Result<String, ExitCode> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| fail(&format!("{}: not UTF-8", path.display())))
}
