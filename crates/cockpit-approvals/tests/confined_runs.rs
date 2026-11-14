//! Confined runs (#1387): `Runner` on a real state database, with fake `systemd-run` and
//! `systemctl` that record what they are asked and run the command in place. What the real
//! manager enforces is proved in `cockpit-processes` and the daemon's ignored tests.
#![cfg(target_os = "linux")]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_approvals::{
    Approved, NoResult, Project, RunConfinement, RunSettings, Runner, approve_run,
};
use cockpit_processes::{LaunchContainment, Supervisor, SystemdTools};
use cockpit_turso::Database;
use common::{NOW, session};

const FAKE_SYSTEMD_RUN: &str = r#"#!/bin/sh
state=@STATE@
if [ "$1" = --version ]; then echo "systemd 262 (fake)"; exit 0; fi
printf '%s\n' "$@" > "$state/arguments"
unit=
while [ $# -gt 0 ]; do
  case $1 in
    --unit=*) unit=${1#--unit=} ;;
    --) shift; break ;;
  esac
  shift
done
printf '%s\n' "$$" > "$state/$unit"
exec "$@"
"#;

const FAKE_SYSTEMCTL: &str = r#"#!/bin/sh
state=@STATE@
case $2 in
  show-environment) exit 0 ;;
  kill) /bin/kill -s "${4#--signal=SIG}" -- "-$(cat "$state/$5")" 2>/dev/null; exit 0 ;;
  show)
    if [ -f "$state/$5" ] && /bin/kill -s 0 -- "-$(cat "$state/$5")" 2>/dev/null; then
      echo active
    else
      echo inactive
    fi ;;
esac
"#;

struct Confined {
    fixture: common::Fixture,
    fakes: tempfile::TempDir,
    /// The asking session's checkout, the one folder the run writes.
    checkout: PathBuf,
    /// A folder the run must not see.
    secrets: PathBuf,
}

impl Confined {
    async fn new() -> Confined {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(
            &dir.path().join("state.db"),
            &[
                cockpit_authorization::MIGRATIONS,
                cockpit_processes::MIGRATIONS,
                cockpit_approvals::MIGRATIONS,
            ],
        )
        .await
        .unwrap();
        let fixture = common::Fixture { dir, database };
        let checkout = fixture.cwd().join("checkout");
        let secrets = fixture.cwd().join("secrets");
        for folder in [&checkout, &secrets] {
            std::fs::create_dir(folder).unwrap();
        }
        let fakes = tempfile::tempdir().unwrap();
        for (name, script) in [
            ("systemd-run", FAKE_SYSTEMD_RUN),
            ("systemctl", FAKE_SYSTEMCTL),
        ] {
            let path = fakes.path().join(name);
            std::fs::write(
                &path,
                script.replace("@STATE@", &fakes.path().display().to_string()),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Confined {
            fixture,
            fakes,
            checkout,
            secrets,
        }
    }

    fn settings(&self) -> RunSettings {
        // `env` stands in for the session leader: the fake runs the command in place, so
        // the terminal is already the launcher's.
        let mut settings = RunSettings::new(
            PathBuf::from("/bin/sh"),
            PathBuf::from("/usr/bin/env"),
            Vec::new(),
        );
        settings.environment = vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("TMPDIR".into(), "/somewhere/else".into()),
        ];
        settings.confinement = RunConfinement::Required {
            hidden: vec![self.secrets.clone()],
            emptied: Vec::new(),
            reachable: Vec::new(),
        };
        settings
    }

    fn runner(&self, settings: RunSettings, containment: LaunchContainment) -> Runner {
        let tools = SystemdTools {
            systemd_run: self.fakes.path().join("systemd-run"),
            systemctl: self.fakes.path().join("systemctl"),
        };
        let supervisor = Supervisor::new(
            self.fixture.dir.path().join("executions"),
            containment,
            tools,
        );
        Runner::new(settings, supervisor)
    }

    /// `command`, asked from the checkout's project in `cwd` and approved.
    async fn approved(&self, command: &str, cwd: &Path) -> Approved {
        let asker = session("s1");
        self.fixture.grant(&asker).await;
        let mut ask = self.fixture.ask(command);
        ask.cwd = cwd.to_owned();
        ask.project = Some(Project {
            name: "demo".to_owned(),
            root: self.checkout.clone(),
        });
        let write = self.fixture.database.write().await.unwrap();
        let asked = cockpit_approvals::ask_run(&write, &asker, ask, NOW)
            .await
            .unwrap();
        let (approved, _) = approve_run(
            &write,
            None,
            asked.id,
            asked.revision,
            &asked.digest(),
            NOW + cockpit_approvals::APPROVAL_GRACE_SECONDS,
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        approved
    }

    fn arguments(&self) -> Vec<String> {
        std::fs::read_to_string(self.fakes.path().join("arguments"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn has_property(arguments: &[String], value: &str) -> bool {
    arguments
        .windows(2)
        .any(|pair| pair[0] == "-p" && pair[1] == value)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_run_writes_only_its_checkout() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let approved = confined.approved("echo ran", &confined.checkout).await;

    let ran = runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    assert_eq!(ran.result.exit_code, 0);
    assert!(ran.result.output.shown.contains("ran"), "{ran:?}");
    let arguments = confined.arguments();
    assert!(
        has_property(
            &arguments,
            &format!("BindPaths=\"{}\"", confined.checkout.display())
        ),
        "{arguments:?}"
    );
    assert!(
        has_property(
            &arguments,
            &format!("InaccessiblePaths=-\"{}\"", confined.secrets.display())
        ),
        "{arguments:?}"
    );
}

/// `/tmp` is the run's own (`PrivateTmp=`): the owner's `TMPDIR` would point outside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_confined_run_keeps_its_temporary_files_in_its_own_tmp() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let approved = confined
        .approved("echo \"tmp=$TMPDIR\"", &confined.checkout)
        .await;

    let ran = runner
        .run(&confined.fixture.database, &approved, &[])
        .await
        .unwrap();

    assert!(ran.result.output.shown.contains("tmp=/tmp"), "{ran:?}");
}

/// Readiness names a host without a systemd user manager; a run there is refused, never run
/// unconfined.
#[tokio::test(flavor = "multi_thread")]
async fn a_confined_run_is_refused_without_a_systemd_manager() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::ProcessGroup);
    let marker = confined.checkout.join("ran");
    let approved = confined
        .approved(&format!("touch '{}'", marker.display()), &confined.checkout)
        .await;

    let ended = runner.run(&confined.fixture.database, &approved, &[]).await;

    let Err(NoResult::NotStarted(reason)) = ended else {
        panic!("expected a run that never started, got {ended:?}");
    };
    assert!(reason.contains("confined"), "{reason}");
    assert!(!marker.exists(), "the command ran unconfined");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_confined_run_in_a_hidden_folder_is_not_started() {
    let confined = Confined::new().await;
    let runner = confined.runner(confined.settings(), LaunchContainment::SystemdScope);
    let approved = confined.approved("true", &confined.secrets).await;

    let ended = runner.run(&confined.fixture.database, &approved, &[]).await;

    let Err(NoResult::NotStarted(reason)) = ended else {
        panic!("expected a run that never started, got {ended:?}");
    };
    assert!(
        reason.contains(&confined.secrets.display().to_string()),
        "{reason}"
    );
    assert!(confined.arguments().is_empty(), "systemd-run was called");
}
