//! Closing a process to its own user: no tracing, no reading its memory or descriptors through
//! `/proc` (#1374). sudo's askpass helper holds the owner's password, and the approved command
//! that started its sudo runs as the same user.

/// Makes this process non-dumpable, then refuses when a tracer is already attached.
///
/// A non-dumpable process cannot be attached to by its own user, and its `/proc/<pid>/fd`,
/// `mem` and `exe` read as permission denied to anyone without `CAP_SYS_PTRACE` (proc(5),
/// `PR_SET_DUMPABLE`). A tracer attached before the call stays attached, so `TracerPid` is
/// read after it: seen as zero then, it stays zero.
#[cfg(target_os = "linux")]
pub fn seal_against_tracing() -> Result<(), String> {
    use rustix::process::{DumpableBehavior, set_dumpable_behavior};
    set_dumpable_behavior(DumpableBehavior::NotDumpable)
        .map_err(|error| format!("it cannot close itself to tracing ({error})"))?;
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("its tracer cannot be read ({error})"))?;
    match tracer_of(&status) {
        Some(0) => Ok(()),
        Some(tracer) => Err(format!("process {tracer} traces it")),
        None => Err("its tracer cannot be read".to_owned()),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn seal_against_tracing() -> Result<(), String> {
    Err("it closes itself to tracing on Linux only".to_owned())
}

/// The pid tracing a process, from its `/proc/<pid>/status` (`TracerPid:`): 0 when none.
pub fn tracer_of(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("TracerPid:"))?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tracer_is_read_from_the_status_file() {
        let status = "Name:\ttorlek-daemon\nState:\tS (sleeping)\nTracerPid:\t4242\nUid:\t1000\n";

        assert_eq!(tracer_of(status), Some(4242));
        assert_eq!(tracer_of("TracerPid:\t0\n"), Some(0));
        assert_eq!(tracer_of("Name:\tsh\n"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_sealed_untraced_process_reads_no_tracer() {
        // Sealing the test process changes only who may read it; nextest runs each test alone.
        assert_eq!(seal_against_tracing(), Ok(()));
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        assert_eq!(tracer_of(&status), Some(0));
    }
}
