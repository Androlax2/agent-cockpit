//! Fixtures shared by the admission tests: a scratch state database, a process-group
//! supervisor, real sleepers, and a fake `systemctl` over a fake cgroup tree.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cockpit_processes::{
    Escalation, Execution, LaunchContainment, LaunchSpec, Supervisor, SystemdTools, Termination,
};
use cockpit_resource_admission::{
    AdmissionRequest, Backend, Demand, HostTools, Limits, Policy, Workload,
};
use cockpit_turso::Database;

pub const QUICK_ESCALATION: Escalation = Escalation {
    grace: Duration::from_millis(500),
    kill_wait: Duration::from_secs(10),
};

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
            &[
                cockpit_processes::MIGRATIONS,
                cockpit_resource_admission::MIGRATIONS,
            ],
        )
        .await
        .expect("the state database opens")
    }

    pub fn supervisor(&self) -> Supervisor {
        Supervisor::new(
            self.path().join("executions"),
            LaunchContainment::ProcessGroup,
            SystemdTools::from_path(),
        )
    }

    /// `spec` running as a process group, confirmed started.
    pub async fn launch(&self, database: &Database, spec: &LaunchSpec) -> Execution {
        self.supervisor()
            .launch(database, spec)
            .await
            .expect("the execution starts")
    }

    pub async fn stop(&self, database: &Database, execution: &Execution) {
        let stopped = self
            .supervisor()
            .terminate(database, execution.id, QUICK_ESCALATION)
            .await
            .expect("termination runs");
        assert!(
            matches!(stopped, Termination::Confirmed { .. }),
            "{stopped:?}"
        );
    }

    pub fn sleeper(&self) -> LaunchSpec {
        LaunchSpec {
            program: PathBuf::from("/bin/sleep"),
            arguments: vec![OsString::from("300")],
            working_dir: self.path().to_path_buf(),
            environment: vec![],
        }
    }
}

/// A budget of 8 CPUs and 16 GiB, the default foreground share (25 %).
pub fn policy() -> Policy {
    Policy::for_budget(Demand {
        cpu_millis: 8000,
        memory_mib: 16 * 1024,
    })
}

pub fn request(workload: Workload, cpu_millis: u64, memory_mib: u64) -> AdmissionRequest {
    AdmissionRequest {
        label: format!("{} for the test", workload.as_str()),
        workload,
        demand: Demand {
            cpu_millis,
            memory_mib,
        },
        limits: Limits::default(),
        required: vec![],
    }
}

/// This host's tools with process-group executions: what a daemon without a user manager
/// enforces, which is what the sandbox and CI have.
pub async fn process_group_backend() -> Backend {
    Backend::detect(
        HostTools::from_host(&SystemdTools::from_path()),
        LaunchContainment::ProcessGroup,
    )
    .await
    .expect("the host probe runs")
}

/// A fake `systemctl` and a fake cgroup tree. The manager's control group is
/// `/user.slice/user@1000.service`; every unit's is `<manager>/app.slice/<unit>`. Each call
/// is appended to `calls`.
pub struct FakeHost {
    pub dir: tempfile::TempDir,
}

pub const MANAGER_GROUP: &str = "/user.slice/user@1000.service";

impl FakeHost {
    pub fn new(controllers: &str) -> FakeHost {
        let dir = tempfile::tempdir().expect("a temporary host");
        let host = FakeHost { dir };
        let systemctl = host.dir.path().join("systemctl");
        std::fs::write(
            &systemctl,
            format!(
                r#"#!/bin/sh
echo "$*" >> '{calls}'
[ -e '{fail}' ] && {{ echo 'Failed to connect to bus' >&2; exit 1; }}
case "$*" in
  "--user show --property=ControlGroup --value") echo '{MANAGER_GROUP}' ;;
  "--user show --property=ControlGroup --value "*) echo "{MANAGER_GROUP}/app.slice/$5" ;;
esac
exit 0
"#,
                calls = host.calls_file().display(),
                fail = host.dir.path().join("fail").display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let manager = host.manager_dir();
        std::fs::create_dir_all(&manager).unwrap();
        std::fs::write(manager.join("cgroup.controllers"), controllers).unwrap();
        std::fs::write(manager.join("cgroup.freeze"), "0\n").unwrap();
        std::fs::create_dir_all(host.dir.path().join("proc/sys/kernel")).unwrap();
        std::fs::write(
            host.dir.path().join("proc/sys/kernel/osrelease"),
            "6.12.0-arch1-1\n",
        )
        .unwrap();
        std::fs::write(
            host.dir.path().join("proc/meminfo"),
            "MemTotal:       32000000 kB\n",
        )
        .unwrap();
        host
    }

    /// The manager does not answer, as without a user bus.
    pub fn fail_systemctl(&self) {
        std::fs::write(self.dir.path().join("fail"), "").unwrap();
    }

    pub fn tools(&self) -> HostTools {
        HostTools {
            systemctl: self.dir.path().join("systemctl"),
            cgroup_root: self.dir.path().join("cgroup"),
            proc_root: self.dir.path().join("proc"),
            chrt: Some(PathBuf::from("/usr/bin/chrt")),
            nice: Some(PathBuf::from("/usr/bin/nice")),
        }
    }

    pub async fn scope_backend(&self) -> Backend {
        Backend::detect(self.tools(), LaunchContainment::SystemdScope)
            .await
            .expect("the fake host probes")
    }

    pub fn manager_dir(&self) -> PathBuf {
        self.dir
            .path()
            .join("cgroup")
            .join(MANAGER_GROUP.trim_start_matches('/'))
    }

    /// The scope's cgroup directory, holding `files`.
    pub fn scope(&self, unit: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = self.manager_dir().join("app.slice").join(unit);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, content) in files {
            std::fs::write(dir.join(name), content).unwrap();
        }
        dir
    }

    fn calls_file(&self) -> PathBuf {
        self.dir.path().join("calls")
    }

    pub fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.calls_file())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}
