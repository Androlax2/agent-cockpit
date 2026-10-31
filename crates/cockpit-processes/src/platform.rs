//! What the operating system says about processes: boot identity, a leader's start token,
//! and whether a process group still has members.

use std::time::Duration;

use rustix::io::Errno;
use rustix::process::{Pid, Signal as OsSignal};

use crate::{Error, Signal};

/// How long a host query (`ps`, `sysctl`, `systemctl`) may take before it counts as failed.
pub(crate) const HOST_COMMAND_DEADLINE: Duration = Duration::from_secs(5);

/// The script the launched shell runs before it becomes the program: it writes its own pid
/// (the process group, since it leads one) and start token to `$1`, then execs the rest. The
/// token comes from the same source [`leader_start`] reads, so the two compare exactly.
/// The file is the launcher's confirmation that the program started, written by the
/// execution itself: a daemon that crashed between spawn and commit still finds it.
#[cfg(target_os = "linux")]
pub(crate) const LAUNCH_SCRIPT: &str = r#"read -r stat < /proc/$$/stat
stat=${stat##*) }
field=3
for value in $stat; do
  if [ "$field" = 22 ]; then start=$value; fi
  field=$((field + 1))
done
printf '%s\n%s\n' "$$" "$start" > "$1.partial" && /bin/mv -f "$1.partial" "$1" || exit 127
shift
exec "$@""#;

#[cfg(target_os = "macos")]
pub(crate) const LAUNCH_SCRIPT: &str = r#"start=$(LC_ALL=C /bin/ps -o lstart= -p $$)
printf '%s\n%s\n' "$$" "$start" > "$1.partial" && /bin/mv -f "$1.partial" "$1" || exit 127
shift
exec "$@""#;

#[cfg(target_os = "linux")]
pub(crate) async fn boot_id() -> Result<String, Error> {
    let path = "/proc/sys/kernel/random/boot_id";
    let id = tokio::fs::read_to_string(path)
        .await
        .map_err(Error::io("read", path))?;
    Ok(id.trim().to_owned())
}

#[cfg(target_os = "macos")]
pub(crate) async fn boot_id() -> Result<String, Error> {
    let output = host_command("/usr/sbin/sysctl", &["-n", "kern.bootsessionuuid"]).await?;
    Ok(output.trim().to_owned())
}

/// The start token of `pid`, or `None` when no such process exists.
#[cfg(target_os = "linux")]
pub(crate) async fn leader_start(pid: i32) -> Result<Option<String>, Error> {
    let path = format!("/proc/{pid}/stat");
    let stat = match tokio::fs::read_to_string(&path).await {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::io("read", path)(error)),
    };
    // Field 2 (the command) may hold spaces and parentheses; it ends at the last `)`.
    let after_command = stat
        .rsplit_once(") ")
        .map(|(_, rest)| rest)
        .ok_or_else(|| Error::HostCommand {
            command: format!("read {path}"),
            reason: format!("no command field in {stat:?}"),
        })?;
    let start = after_command
        .split_whitespace()
        .nth(22 - 3)
        .ok_or_else(|| Error::HostCommand {
            command: format!("read {path}"),
            reason: format!("fewer than 22 fields in {stat:?}"),
        })?;
    Ok(Some(start.to_owned()))
}

#[cfg(target_os = "macos")]
pub(crate) async fn leader_start(pid: i32) -> Result<Option<String>, Error> {
    let output = tokio::process::Command::new("/bin/ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .env("LC_ALL", "C")
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(HOST_COMMAND_DEADLINE, output)
        .await
        .map_err(|_| timed_out("/bin/ps"))?
        .map_err(Error::io("run", "/bin/ps"))?;
    let start = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    // `ps -p` exits 1 when the process does not exist.
    Ok((output.status.success() && !start.is_empty()).then_some(start))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupState {
    /// No process has this process group id.
    Empty,
    /// The group has members this user may signal.
    Present,
    /// The group has members of another user: whatever it is, it is not this user's.
    Foreign,
}

pub(crate) fn group_state(process_group: i32) -> GroupState {
    let Some(pid) = Pid::from_raw(process_group) else {
        return GroupState::Empty;
    };
    match rustix::process::test_kill_process_group(pid) {
        Ok(()) => GroupState::Present,
        Err(Errno::SRCH) => GroupState::Empty,
        // EPERM is the only other answer kill(2) gives a valid signal: members exist.
        Err(_) => GroupState::Foreign,
    }
}

/// Sends `signal` to every member of `process_group`. An empty group is not an error: it is
/// what termination waits for.
pub(crate) fn signal_group(process_group: i32, signal: Signal) -> Result<(), Error> {
    let Some(pid) = Pid::from_raw(process_group) else {
        return Ok(());
    };
    let os_signal = match signal {
        Signal::Term => OsSignal::TERM,
        Signal::Kill => OsSignal::KILL,
    };
    match rustix::process::kill_process_group(pid, os_signal) {
        Ok(()) | Err(Errno::SRCH) => Ok(()),
        Err(errno) => Err(Error::HostCommand {
            command: format!("kill -{os_signal:?} -{process_group}"),
            reason: errno.to_string(),
        }),
    }
}

/// Runs a host query with a deadline and returns its stdout, failing loud on a non-zero exit.
pub(crate) async fn host_command(program: &str, arguments: &[&str]) -> Result<String, Error> {
    let command_line = || format!("{program} {}", arguments.join(" "));
    let output = tokio::process::Command::new(program)
        .args(arguments)
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(HOST_COMMAND_DEADLINE, output)
        .await
        .map_err(|_| timed_out(&command_line()))?
        .map_err(|source| Error::HostCommand {
            command: command_line(),
            reason: source.to_string(),
        })?;
    if !output.status.success() {
        return Err(Error::HostCommand {
            command: command_line(),
            reason: format!(
                "{}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn timed_out(command: &str) -> Error {
    Error::HostCommand {
        command: command.to_owned(),
        reason: format!("no answer within {HOST_COMMAND_DEADLINE:?}"),
    }
}
