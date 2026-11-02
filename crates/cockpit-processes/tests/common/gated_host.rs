//! A test-owned host that never reaps until asked, including adopted group descendants.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{self, Pid, Signal, WaitId, WaitIdOptions, WaitOptions};

static SUBREAPER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

pub struct GatedHost {
    pub leader: Pid,
    _leader_handle: std::process::Child,
    previous_subreaper: Option<Pid>,
    _permit: tokio::sync::SemaphorePermit<'static>,
    emptied: bool,
}

impl GatedHost {
    pub async fn start(command: &[std::ffi::OsString], directory: &std::path::Path) -> Self {
        let permit = SUBREAPER.acquire().await.unwrap();
        let previous_subreaper = process::child_subreaper().unwrap();
        process::set_child_subreaper(Some(Pid::INIT)).unwrap();
        let child = Command::new(&command[0])
            .args(&command[1..])
            .current_dir(directory)
            .stdin(Stdio::null())
            .process_group(0)
            .spawn();
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                process::set_child_subreaper(previous_subreaper).unwrap();
                panic!("host spawn: {error}");
            }
        };
        // Retain the host handle; this fixture owns every wait by group/pid, not Child::wait.
        Self {
            leader: Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap(),
            _leader_handle: child,
            previous_subreaper,
            _permit: permit,
            emptied: false,
        }
    }

    pub async fn exited_without_reaping(&self, pid: Pid) -> process::WaitIdStatus {
        let deadline = tokio::time::Instant::now() + super::GENEROUS;
        loop {
            if let Some(status) = process::waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
            )
            .unwrap()
            {
                return status;
            }
            assert!(tokio::time::Instant::now() < deadline, "child did not exit");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub fn reap(&mut self, pid: Pid) {
        assert!(
            process::waitpid(Some(pid), WaitOptions::NOHANG)
                .unwrap()
                .is_some()
        );
        self.emptied = matches!(
            process::test_kill_process_group(self.leader),
            Err(Errno::SRCH)
        );
    }

    pub fn cleanup(&mut self) {
        if self.emptied {
            return;
        }
        match process::kill_process_group(self.leader, Signal::KILL) {
            Ok(()) | Err(Errno::SRCH) => {}
            Err(error) => panic!("fixture group cleanup: {error}"),
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match process::waitpgid(self.leader, WaitOptions::NOHANG) {
                Ok(Some(_)) => continue,
                Err(Errno::CHILD) => {
                    self.emptied = true;
                    break;
                }
                Ok(None) => {}
                Err(Errno::INTR) => continue,
                Err(error) => panic!("fixture group reap: {error}"),
            }
            assert!(Instant::now() < deadline, "fixture group cleanup timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for GatedHost {
    fn drop(&mut self) {
        self.cleanup();
        process::set_child_subreaper(self.previous_subreaper).expect("restore fixture subreaper");
    }
}
