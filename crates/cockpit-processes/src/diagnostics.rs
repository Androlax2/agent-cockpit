//! Failure evidence only: none of these observations may release an execution.

use std::time::Duration;

use crate::{Containment, Execution, Phase};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnconfirmedReason {
    Unidentified,
    KillWaitElapsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalPresence {
    Identified,
    Unidentified,
}

/// What the signal operation answered, not proof that any process exited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalOutcome {
    Sent,
    /// The process-group signal returned ESRCH.
    NoSuchGroup,
    /// No syscall was made because the group id was not a positive pid.
    InvalidGroup,
    CommandAccepted,
    /// The systemd kill command failed, but the scope was observed inactive.
    ScopeInactive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminationDiagnostics {
    /// Identity/phase at identification, before signalling.
    pub execution: Execution,
    pub reason: UnconfirmedReason,
    pub final_phase: Phase,
    pub final_presence: FinalPresence,
    pub term: Option<SignalOutcome>,
    pub kill: Option<SignalOutcome>,
    pub elapsed: Duration,
    pub snapshot: ProcessSnapshot,
}

/// Numeric kernel metadata only. In particular, comm and command lines are excluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessState {
    pub pid: i32,
    pub parent: i32,
    pub group: i32,
    pub session: i32,
    pub state: char,
    pub start_ticks: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessRead {
    Present(ProcessState),
    Absent,
    Unreadable { errno: Option<i32> },
    Malformed,
    TooLarge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessObservation {
    pub pid: i32,
    pub read: ProcessRead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotStatus {
    /// Traversal finished; records can still be unreadable or race process exit/reuse.
    Sampled,
    Busy,
    Deadline,
    EntryLimit,
    MemberLimit,
    UnsupportedPlatform,
    UnsupportedContainment,
    NoProcessGroup,
    ReadError {
        errno: Option<i32>,
    },
    WorkerFailed,
}

/// A bounded, non-atomic sample, never a liveness or identity decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub status: SnapshotStatus,
    pub leader: Option<ProcessObservation>,
    pub members: Vec<ProcessState>,
    pub parents: Vec<ProcessObservation>,
    pub entries_examined: usize,
    pub unreadable_entries: usize,
    pub vanished_entries: usize,
}

impl ProcessSnapshot {
    fn unavailable(status: SnapshotStatus) -> Self {
        Self {
            status,
            leader: None,
            members: Vec::new(),
            parents: Vec::new(),
            entries_examined: 0,
            unreadable_entries: 0,
            vanished_entries: 0,
        }
    }
}

pub(crate) async fn snapshot(containment: &Containment, group: Option<i32>) -> ProcessSnapshot {
    if !matches!(containment, Containment::ProcessGroup) {
        return ProcessSnapshot::unavailable(SnapshotStatus::UnsupportedContainment);
    }
    let Some(group) = group else {
        return ProcessSnapshot::unavailable(SnapshotStatus::NoProcessGroup);
    };
    #[cfg(target_os = "linux")]
    {
        static SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
        collect(&SLOTS, Duration::from_millis(100), move |deadline| {
            scan(
                std::path::Path::new("/proc"),
                group,
                Limits {
                    entries: 4096,
                    members: 32,
                    stat_bytes: 4096,
                },
                deadline,
            )
        })
        .await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = group;
        ProcessSnapshot::unavailable(SnapshotStatus::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
async fn collect(
    slots: &'static tokio::sync::Semaphore,
    budget: Duration,
    work: impl FnOnce(std::time::Instant) -> ProcessSnapshot + Send + 'static,
) -> ProcessSnapshot {
    let Ok(permit) = slots.try_acquire() else {
        return ProcessSnapshot::unavailable(SnapshotStatus::Busy);
    };
    let deadline = std::time::Instant::now() + budget;
    let worker = tokio::task::spawn_blocking(move || {
        // A dropped/timed-out caller cannot admit another worker while this one is blocked.
        let _permit = permit;
        work(deadline)
    });
    match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), worker).await {
        Ok(Ok(sample)) => sample,
        Ok(Err(_)) => ProcessSnapshot::unavailable(SnapshotStatus::WorkerFailed),
        Err(_) => ProcessSnapshot::unavailable(SnapshotStatus::Deadline),
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
struct Limits {
    entries: usize,
    members: usize,
    stat_bytes: usize,
}

#[cfg(target_os = "linux")]
fn scan(
    root: &std::path::Path,
    group: i32,
    limits: Limits,
    deadline: std::time::Instant,
) -> ProcessSnapshot {
    let mut sample = ProcessSnapshot::unavailable(SnapshotStatus::Sampled);
    if std::time::Instant::now() >= deadline {
        sample.status = SnapshotStatus::Deadline;
        return sample;
    }
    let leader = read_process(root, group, limits.stat_bytes);
    if let ProcessRead::Present(state) = &leader
        && state.group == group
    {
        sample.members.push(state.clone());
    }
    sample.leader = Some(ProcessObservation {
        pid: group,
        read: leader,
    });
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            sample.status = SnapshotStatus::ReadError {
                errno: error.raw_os_error(),
            };
            return sample;
        }
    };
    for entry in entries {
        if std::time::Instant::now() >= deadline {
            sample.status = SnapshotStatus::Deadline;
            break;
        }
        if sample.entries_examined == limits.entries {
            sample.status = SnapshotStatus::EntryLimit;
            break;
        }
        sample.entries_examined += 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                sample.status = SnapshotStatus::ReadError {
                    errno: error.raw_os_error(),
                };
                break;
            }
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
            .filter(|pid| *pid > 0)
        else {
            continue;
        };
        if pid == group {
            continue;
        }
        match read_process(root, pid, limits.stat_bytes) {
            ProcessRead::Present(state) if state.group == group => {
                if sample.members.len() == limits.members {
                    sample.status = SnapshotStatus::MemberLimit;
                    break;
                }
                sample.members.push(state);
            }
            ProcessRead::Present(_) => {}
            ProcessRead::Absent => sample.vanished_entries += 1,
            _ => sample.unreadable_entries += 1,
        }
    }
    for member in &sample.members {
        if std::time::Instant::now() >= deadline {
            sample.status = SnapshotStatus::Deadline;
            break;
        }
        if member.parent <= 0
            || sample
                .parents
                .iter()
                .any(|parent| parent.pid == member.parent)
        {
            continue;
        }
        sample.parents.push(ProcessObservation {
            pid: member.parent,
            read: read_process(root, member.parent, limits.stat_bytes),
        });
    }
    sample
}

#[cfg(target_os = "linux")]
fn read_process(root: &std::path::Path, pid: i32, max_bytes: usize) -> ProcessRead {
    use std::io::Read;
    let mut bytes = Vec::new();
    let read = std::fs::File::open(root.join(format!("{pid}/stat")))
        .and_then(|file| file.take((max_bytes + 1) as u64).read_to_end(&mut bytes));
    if let Err(error) = read {
        return if error.kind() == std::io::ErrorKind::NotFound
            || error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error())
        {
            ProcessRead::Absent
        } else {
            ProcessRead::Unreadable {
                errno: error.raw_os_error(),
            }
        };
    }
    if bytes.len() > max_bytes {
        return ProcessRead::TooLarge;
    }
    match parse_stat(&bytes) {
        Some(state) if state.pid == pid => ProcessRead::Present(state),
        _ => ProcessRead::Malformed,
    }
}

#[cfg(target_os = "linux")]
fn parse_stat(bytes: &[u8]) -> Option<ProcessState> {
    let first_space = bytes.iter().position(|byte| *byte == b' ')?;
    let pid: i32 = std::str::from_utf8(&bytes[..first_space])
        .ok()?
        .parse()
        .ok()?;
    // comm can contain whitespace, parentheses and non-UTF8; never decode or retain it.
    let end_command = bytes.windows(2).rposition(|pair| pair == b") ")?;
    if bytes.get(first_space + 1) != Some(&b'(') || end_command <= first_space {
        return None;
    }
    let mut fields = std::str::from_utf8(&bytes[end_command + 2..])
        .ok()?
        .split_whitespace();
    let state = fields.next()?;
    if state.len() != 1 || !b"RSDZTtXxKWPI".contains(&state.as_bytes()[0]) {
        return None;
    }
    let parent: i32 = fields.next()?.parse().ok()?;
    let group: i32 = fields.next()?.parse().ok()?;
    let session: i32 = fields.next()?.parse().ok()?;
    let start_ticks = fields.nth(22 - 7)?.parse().ok()?;
    if pid <= 0 || parent < 0 || group < 0 || session < 0 {
        return None;
    }
    Some(ProcessState {
        pid,
        parent,
        group,
        session,
        state: char::from(state.as_bytes()[0]),
        start_ticks,
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
