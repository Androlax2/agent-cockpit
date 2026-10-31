//! Fixtures shared by the processes tests: a scratch state database, a supervisor over a
//! temporary executions directory, and fake `systemd-run` / `systemctl` programs.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cockpit_processes::{
    Escalation, Execution, ExecutionId, LaunchContainment, LaunchSpec, Phase, Supervisor,
    SystemdTools,
};
use cockpit_turso::Database;

/// Long enough never to fire in a passing test, short enough to fail a hung one quickly.
pub const GENEROUS: Duration = Duration::from_secs(10);

pub const QUICK_ESCALATION: Escalation = Escalation {
    grace: Duration::from_millis(500),
    kill_wait: GENEROUS,
};

/// One test's home: the state database file and the executions directory live in `root`.
pub struct Home {
    pub root: tempfile::TempDir,
}

impl Home {
    pub fn new() -> Home {
        Home {
            root: tempfile::tempdir().expect("a temporary home"),
        }
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }

    pub async fn open_database(&self) -> Database {
        Database::open(
            &self.path().join("state.db"),
            &[cockpit_processes::MIGRATIONS],
        )
        .await
        .expect("the state database opens")
    }

    pub fn executions_dir(&self) -> PathBuf {
        self.path().join("executions")
    }

    pub fn process_group_supervisor(&self) -> Supervisor {
        Supervisor::new(
            self.executions_dir(),
            LaunchContainment::ProcessGroup,
            SystemdTools::from_path(),
        )
    }

    pub fn scope_supervisor(&self, fakes: &FakeSystemd) -> Supervisor {
        Supervisor::new(
            self.executions_dir(),
            LaunchContainment::SystemdScope,
            fakes.tools(),
        )
    }
}

/// `/bin/sh -c <script>` in the home directory, with only `PATH` set.
pub fn shell(home: &Home, script: &str) -> LaunchSpec {
    LaunchSpec {
        program: PathBuf::from("/bin/sh"),
        arguments: vec![OsString::from("-c"), OsString::from(script)],
        working_dir: home.path().to_path_buf(),
        environment: vec![(
            OsString::from("PATH"),
            OsString::from("/usr/local/bin:/usr/bin:/bin"),
        )],
    }
}

/// Observes `id` until it ended or `GENEROUS` passed.
pub async fn wait_until_ended(
    supervisor: &Supervisor,
    database: &Database,
    id: ExecutionId,
) -> Execution {
    let deadline = tokio::time::Instant::now() + GENEROUS;
    loop {
        let execution = supervisor.observe(database, id).await.unwrap();
        if !execution.phase.is_occupying() || tokio::time::Instant::now() >= deadline {
            return execution;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Polls `condition` until it holds or `GENEROUS` passed; returns whether it held.
pub async fn eventually(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + GENEROUS;
    while tokio::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    condition()
}

/// Whether a process with this pid exists (zombies included).
pub fn process_exists(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists() || ps_lists(pid)
}

fn ps_lists(pid: i32) -> bool {
    cfg!(target_os = "macos")
        && std::process::Command::new("/bin/ps")
            .args(["-p", &pid.to_string()])
            .output()
            .is_ok_and(|output| output.status.success())
}

/// `kill -KILL -<group>`, as an external actor would.
pub fn kill_group(process_group: i32) {
    let _ = std::process::Command::new("/bin/kill")
        .args(["-s", "KILL", "--", &format!("-{process_group}")])
        .status();
}

/// Reads a pid a test program wrote to `path`, waiting for it.
pub async fn read_pid(path: &Path) -> i32 {
    let mut pid = None;
    assert!(
        eventually(|| {
            pid = std::fs::read_to_string(path)
                .ok()
                .and_then(|text| text.trim().parse().ok());
            pid.is_some()
        })
        .await,
        "{} was never written",
        path.display()
    );
    pid.unwrap()
}

/// Rewrites one column of an execution's row, as a crash or a reboot would leave it.
pub async fn set_column(database: &Database, id: ExecutionId, assignment: &str) {
    let write = database.write().await.unwrap();
    write
        .execute(
            &format!("UPDATE process_executions SET {assignment} WHERE id = ?1"),
            [id.0],
        )
        .await
        .unwrap();
    write.commit().await.unwrap();
}

pub fn assert_running(execution: &Execution) {
    assert_eq!(execution.phase, Phase::Running, "{execution:?}");
}

/// Fake `systemd-run` and `systemctl` sharing a state directory. `systemd-run` records the
/// unit's process group, optionally fails or waits, then execs the command like `--scope`
/// does. `systemctl` logs its arguments and answers `kill` and `show` from that record.
pub struct FakeSystemd {
    pub dir: tempfile::TempDir,
}

const FAKE_SYSTEMD_RUN: &str = r#"#!/bin/sh
state=@STATE@
unit=
while [ $# -gt 0 ]; do
  case $1 in
    --unit=*) unit=${1#--unit=} ;;
    --) shift; break ;;
  esac
  shift
done
echo "systemd-run $unit" >> "$state/calls"
if [ -f "$state/fail" ]; then
  echo "Failed to connect to bus: No such file or directory" >&2
  exit 1
fi
printf '%s\n' "$$" > "$state/$unit"
if [ -f "$state/delay" ]; then sleep "$(cat "$state/delay")"; fi
exec "$@"
"#;

const FAKE_SYSTEMCTL: &str = r#"#!/bin/sh
state=@STATE@
echo "systemctl $*" >> "$state/calls"
case $2 in
  show-environment)
    if [ -f "$state/no-manager" ]; then exit 1; fi ;;
  kill)
    signal=${4#--signal=SIG}
    /bin/kill -s "$signal" -- "-$(cat "$state/$5")" 2>/dev/null
    exit 0 ;;
  show)
    if [ -f "$state/$5" ] && /bin/kill -s 0 -- "-$(cat "$state/$5")" 2>/dev/null; then
      echo active
    else
      echo inactive
    fi ;;
esac
"#;

impl FakeSystemd {
    pub fn new() -> FakeSystemd {
        let dir = tempfile::tempdir().expect("a directory for the fakes");
        for (name, script) in [
            ("systemd-run", FAKE_SYSTEMD_RUN),
            ("systemctl", FAKE_SYSTEMCTL),
        ] {
            let path = dir.path().join(name);
            let script = script.replace("@STATE@", &dir.path().display().to_string());
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        FakeSystemd { dir }
    }

    pub fn tools(&self) -> SystemdTools {
        SystemdTools {
            systemd_run: self.dir.path().join("systemd-run"),
            systemctl: self.dir.path().join("systemctl"),
        }
    }

    /// Makes the next `systemd-run` sleep `seconds` before it starts the program.
    pub fn delay_launch(&self, seconds: u32) {
        std::fs::write(self.dir.path().join("delay"), seconds.to_string()).unwrap();
    }

    pub fn fail_launch(&self) {
        std::fs::write(self.dir.path().join("fail"), "").unwrap();
    }

    pub fn without_manager(&self) {
        std::fs::write(self.dir.path().join("no-manager"), "").unwrap();
    }

    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}
