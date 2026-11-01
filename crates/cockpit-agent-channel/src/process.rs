//! Which processes a peer descends from, as the kernel reports them.

use std::fmt;

/// One process, told apart from any later process that reuses its pid: the boot it ran in,
/// its pid and its start time: clock ticks since that boot on Linux, seconds since the Unix
/// epoch on macOS. Compared only with identities read on the same host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub boot: String,
    pub pid: u32,
    pub start: u64,
}

/// Why a peer's ancestry could not be read. The channel then identifies nobody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessError {
    /// The process exited before its ancestry was read.
    Gone { pid: u32 },
    /// The kernel's process table could not be read or parsed.
    Unreadable { reason: String },
    /// Reading a process tree is implemented for Linux and macOS only.
    Unsupported,
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProcessError::Gone { pid } => write!(formatter, "process {pid} exited"),
            ProcessError::Unreadable { reason } => {
                write!(formatter, "the process table is unreadable: {reason}")
            }
            ProcessError::Unsupported => formatter
                .write_str("verifying a process tree is implemented for Linux and macOS only"),
        }
    }
}

impl std::error::Error for ProcessError {}

/// Deepest tree walked. Linux allows deeper trees in principle; a chain this long is refused
/// as unverifiable rather than cut short, which could skip the real root.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const MAX_DEPTH: usize = 4096;

/// `pid` and each of its ancestors, nearest first, ending at the first process without a
/// parent. The walk stops early at a parent that started after its child: that pid was
/// reused after the real parent exited, so nothing above it is this process's ancestry.
/// A parent that exits during the walk ends the chain too; any other unreadable entry is an
/// error, never a shorter chain. Blocking: it reads the kernel's process table.
pub fn ancestry(pid: u32) -> Result<Vec<ProcessIdentity>, ProcessError> {
    platform::ancestry(pid)
}

/// The ancestry of `process`, read only while its pid still names it: a pid reused by a later
/// process reads as [`ProcessError::Gone`], never as that process's tree. Blocking, like
/// [`ancestry`].
pub fn ancestry_of(process: &ProcessIdentity) -> Result<Vec<ProcessIdentity>, ProcessError> {
    let chain = ancestry(process.pid)?;
    if chain.first() != Some(process) {
        return Err(ProcessError::Gone { pid: process.pid });
    }
    Ok(chain)
}

/// The identity of the process `pid`. Blocking, like [`ancestry`].
pub fn identity_of(pid: u32) -> Result<ProcessIdentity, ProcessError> {
    platform::identity_of(pid)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::fs;
    use std::io::ErrorKind;

    use super::{ProcessError, ProcessIdentity, walk};

    pub(super) fn ancestry(pid: u32) -> Result<Vec<ProcessIdentity>, ProcessError> {
        let boot = boot_id()?;
        walk(pid, |pid| stat(pid, &boot))
    }

    pub(super) fn identity_of(pid: u32) -> Result<ProcessIdentity, ProcessError> {
        Ok(stat(pid, &boot_id()?)?.0)
    }

    fn boot_id() -> Result<String, ProcessError> {
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map(|id| id.trim().to_owned())
            .map_err(|error| ProcessError::Unreadable {
                reason: format!("boot id: {error}"),
            })
    }

    /// The process's identity and its parent's pid, from `/proc/<pid>/stat` (proc(5)).
    fn stat(pid: u32, boot: &str) -> Result<(ProcessIdentity, u32), ProcessError> {
        let text = match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(ProcessError::Gone { pid });
            }
            Err(error) => {
                return Err(ProcessError::Unreadable {
                    reason: format!("process {pid}: {error}"),
                });
            }
        };
        let unparsable = || ProcessError::Unreadable {
            reason: format!("process {pid}: unexpected stat line"),
        };
        // The command name, field 2, may hold spaces and parentheses: fields resume after
        // the last `)`. Field 3 is the state, 4 the parent pid, 22 the start time.
        let (_, fields) = text.rsplit_once(')').ok_or_else(unparsable)?;
        let fields: Vec<&str> = fields.split_whitespace().collect();
        let field = |number: usize| fields.get(number - 3).ok_or_else(unparsable);
        let parent = field(4)?.parse().map_err(|_| unparsable())?;
        let start = field(22)?.parse().map_err(|_| unparsable())?;
        Ok((
            ProcessIdentity {
                boot: boot.to_owned(),
                pid,
                start,
            },
            parent,
        ))
    }
}

/// A walk over a process table that names each process's parent: shared by the platforms so
/// the chain's rules (reused parent pid, a parent gone mid-walk, depth) are written once.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn walk(
    pid: u32,
    mut read: impl FnMut(u32) -> Result<(ProcessIdentity, u32), ProcessError>,
) -> Result<Vec<ProcessIdentity>, ProcessError> {
    let mut chain = Vec::new();
    let (mut current, mut parent) = read(pid)?;
    loop {
        chain.push(current);
        if parent == 0 {
            return Ok(chain);
        }
        if chain.len() == MAX_DEPTH {
            return Err(ProcessError::Unreadable {
                reason: format!("process {pid} has more than {MAX_DEPTH} ancestors"),
            });
        }
        let (ancestor, grandparent) = match read(parent) {
            Ok(entry) => entry,
            Err(ProcessError::Gone { .. }) => return Ok(chain),
            Err(error) => return Err(error),
        };
        if ancestor.start > chain.last().expect("the chain is never empty").start {
            return Ok(chain);
        }
        (current, parent) = (ancestor, grandparent);
    }
}

/// macOS has no `/proc`, and its `sysctl(3)`/`proc_pidinfo` are reachable only through
/// `unsafe` FFI, which the workspace denies: `ps(1)` and `sysctl(8)` read the same kernel
/// table, as `cockpit-processes` already does (docs/decisions/2026-11-01-336-macos-process-trees.md).
#[cfg(target_os = "macos")]
mod platform {
    use std::collections::HashMap;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::OnceLock;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::{ProcessError, ProcessIdentity, walk};

    /// How long `ps` or `sysctl` may take before the read counts as failed.
    const HOST_COMMAND_DEADLINE: Duration = Duration::from_secs(5);

    pub(super) fn ancestry(pid: u32) -> Result<Vec<ProcessIdentity>, ProcessError> {
        let boot = boot_id()?;
        // One snapshot of the whole table rather than one `ps` per ancestor: a deep agent
        // tree costs one process spawn, and every entry comes from the same instant.
        let (_, output) = run("/bin/ps", &["-A", "-o", "pid=,ppid=,lstart="])?;
        let mut table = HashMap::new();
        for line in output.lines().filter(|line| !line.trim().is_empty()) {
            let (process, parent) = parse_line(line, boot)?;
            table.insert(process.pid, (process, parent));
        }
        walk(pid, |pid| {
            table.get(&pid).cloned().ok_or(ProcessError::Gone { pid })
        })
    }

    pub(super) fn identity_of(pid: u32) -> Result<ProcessIdentity, ProcessError> {
        let boot = boot_id()?;
        let (succeeded, output) = run(
            "/bin/ps",
            &["-o", "pid=,ppid=,lstart=", "-p", &pid.to_string()],
        )?;
        let line = output.trim();
        // `ps -p` exits 1 and prints nothing when no such process exists.
        if !succeeded && line.is_empty() {
            return Err(ProcessError::Gone { pid });
        }
        let (process, _) = parse_line(line, boot)?;
        if process.pid != pid {
            return Err(unreadable(format!(
                "ps answered for process {} instead of {pid}",
                process.pid
            )));
        }
        Ok(process)
    }

    /// `kern.bootsessionuuid`, new at every boot. Read once: it cannot change while this
    /// process runs.
    fn boot_id() -> Result<&'static str, ProcessError> {
        static BOOT: OnceLock<String> = OnceLock::new();
        if let Some(boot) = BOOT.get() {
            return Ok(boot);
        }
        let (succeeded, output) = run("/usr/sbin/sysctl", &["-n", "kern.bootsessionuuid"])?;
        let boot = output.trim();
        if !succeeded || boot.is_empty() {
            return Err(unreadable(
                "boot id: sysctl kern.bootsessionuuid answered nothing".to_owned(),
            ));
        }
        Ok(BOOT.get_or_init(|| boot.to_owned()))
    }

    /// One `ps` line, `<pid> <ppid> <weekday> <month> <day> <hh:mm:ss> <year>`, as the
    /// identity and the parent's pid.
    fn parse_line(line: &str, boot: &str) -> Result<(ProcessIdentity, u32), ProcessError> {
        let unparsable = || unreadable(format!("unexpected ps line {line:?}"));
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [pid, parent, _weekday, month, day, time, year] = fields[..] else {
            return Err(unparsable());
        };
        let pid = pid.parse().map_err(|_| unparsable())?;
        let parent = parent.parse().map_err(|_| unparsable())?;
        let start = lstart_seconds(month, day, time, year).ok_or_else(unparsable)?;
        Ok((
            ProcessIdentity {
                boot: boot.to_owned(),
                pid,
                start,
            },
            parent,
        ))
    }

    /// Seconds since the Unix epoch of a `ps -o lstart` date printed in UTC and the C locale.
    fn lstart_seconds(month: &str, day: &str, time: &str, year: &str) -> Option<u64> {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        let month = MONTHS.iter().position(|name| *name == month)? as i64 + 1;
        let day: i64 = day.parse().ok()?;
        let year: i64 = year.parse().ok()?;
        let mut clock = time.split(':').map(|part| part.parse::<i64>().ok());
        let (hours, minutes, seconds) = (clock.next()??, clock.next()??, clock.next()??);
        if clock.next().is_some() {
            return None;
        }
        let days = days_from_civil(year, month, day);
        u64::try_from(days * 86_400 + hours * 3_600 + minutes * 60 + seconds).ok()
    }

    /// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
    /// `days_from_civil`, http://howardhinnant.github.io/date_algorithms.html).
    fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
        let year = if month <= 2 { year - 1 } else { year };
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let shifted_month = (month + 9) % 12;
        let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    /// Runs a system program with a fixed locale and time zone, so `ps` prints its dates in
    /// one format whatever the user's settings; whether it succeeded, and its output.
    fn run(program: &str, arguments: &[&str]) -> Result<(bool, String), ProcessError> {
        let mut child = Command::new(program)
            .args(arguments)
            .env("LC_ALL", "C")
            .env("TZ", "UTC0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| unreadable(format!("{program}: {error}")))?;
        let mut stdout = child.stdout.take().expect("stdout is piped");
        // The output is read on its own thread: a table larger than the pipe's buffer would
        // otherwise block `ps` before it exits, and the deadline below could not tell.
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut output = Vec::new();
            let read = stdout.read_to_end(&mut output).map(|_| output);
            let _ = sender.send(read);
        });
        let output = match receiver.recv_timeout(HOST_COMMAND_DEADLINE) {
            Ok(read) => read.map_err(|error| unreadable(format!("{program}: {error}")))?,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unreadable(format!(
                    "{program} answered nothing within {HOST_COMMAND_DEADLINE:?}"
                )));
            }
        };
        let status = child
            .wait()
            .map_err(|error| unreadable(format!("{program}: {error}")))?;
        let output = String::from_utf8(output)
            .map_err(|_| unreadable(format!("{program} printed something other than UTF-8")))?;
        Ok((status.success(), output))
    }

    fn unreadable(reason: String) -> ProcessError {
        ProcessError::Unreadable { reason }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use super::{ProcessError, ProcessIdentity};

    pub(super) fn ancestry(_pid: u32) -> Result<Vec<ProcessIdentity>, ProcessError> {
        Err(ProcessError::Unsupported)
    }

    pub(super) fn identity_of(_pid: u32) -> Result<ProcessIdentity, ProcessError> {
        Err(ProcessError::Unsupported)
    }
}
