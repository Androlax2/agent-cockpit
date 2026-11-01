//! `cockpit resources` driven as a process against a scripted daemon socket. The rules are
//! proven in `cockpit-daemon/tests/resources.rs`; these prove each verb's request, its
//! output and its exit status.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    Command, ControlSupportView, DemandView, EffectiveView, LimitView, ObservedView, Outcome,
    ProfileView, Reply, ReservationView, ResourceStatusView, VerdictView,
};
use scripted::{scripted_daemon, text};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    fn daemon_socket(&self) -> PathBuf {
        let path = self.runtime_dir().join("cockpit-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    async fn cockpit(&self, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.runtime_dir());
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn running_build() -> ReservationView {
    ReservationView {
        id: 3,
        label: "make test".to_owned(),
        workload: "build".to_owned(),
        state: "running".to_owned(),
        execution: Some(12),
        reserved: DemandView {
            cpu_millis: 2000,
            memory_mib: 1024,
        },
        required: vec![],
        checkpoint_requested: false,
        limits: vec![
            LimitView {
                control: "nice".to_owned(),
                requested: Some("19".to_owned()),
                supported: true,
                effective: EffectiveView::Value {
                    value: "19".to_owned(),
                },
            },
            LimitView {
                control: "cpu-quota".to_owned(),
                requested: Some("200%".to_owned()),
                supported: false,
                effective: EffectiveView::Unknown {
                    why: "a process group has no cgroup".to_owned(),
                },
            },
        ],
        observed: ObservedView::default(),
    }
}

#[tokio::test]
async fn resources_status_prints_the_host_each_control_and_each_reservations_limits() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::ResourceStatus {
            status: Box::new(ResourceStatusView {
                platform: "macos".to_owned(),
                containment: "process-group".to_owned(),
                profile: ProfileView::Foreground,
                queue_paused: false,
                budget: DemandView {
                    cpu_millis: 8000,
                    memory_mib: 16384,
                },
                in_use: DemandView {
                    cpu_millis: 2000,
                    memory_mib: 1024,
                },
                foreground_heavy_percent: 25,
                controls: vec![ControlSupportView {
                    control: "idle-scheduling".to_owned(),
                    supported: false,
                    detail: "macOS has no SCHED_IDLE; nice is its subset".to_owned(),
                }],
                reservations: vec![running_build()],
            }),
        }),
    );

    let output = home.cockpit(&["resources", "status"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "platform macos\tcontainment process-group\tprofile foreground\tqueue running\n\
         budget 8000 millicores 16384 MiB\tin use 2000 millicores 1024 MiB\tforeground heavy share 25%\n\
         control idle-scheduling\tno\tmacOS has no SCHED_IDLE; nice is its subset\n\
         reservation 3\tbuild\trunning\texecution 12\treserved 2000 millicores 1024 MiB\tmake test\n\
         \x20 nice\trequested 19\tsupported yes\teffective 19\n\
         \x20 cpu-quota\trequested 200%\tsupported no\teffective unknown (a process group has no cgroup)\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::ResourceStatus);
}

#[tokio::test]
async fn resources_check_sends_the_demand_and_required_controls_and_exits_1_when_held() {
    let home = Home::new();
    let reason = "cpu-quota is required but cannot be enforced here: macOS has no cgroups";
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::ResourceVerdict {
            verdict: VerdictView::Held {
                reason: reason.to_owned(),
            },
        }),
    );

    let output = home
        .cockpit(&[
            "resources",
            "check",
            "build",
            "--cpu",
            "2000",
            "--memory",
            "1024",
            "--require",
            "cpu-quota",
        ])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(text(&output.stdout), format!("held: {reason}\n"));
    assert_eq!(
        daemon.await.unwrap(),
        Command::ResourceCheck {
            workload: "build".to_owned(),
            cpu_millis: 2000,
            memory_mib: 1024,
            required: vec!["cpu-quota".to_owned()],
        }
    );
}

#[tokio::test]
async fn resources_check_prints_fits_and_exits_0_when_the_work_would_start() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::ResourceVerdict {
            verdict: VerdictView::Fits,
        }),
    );

    let output = home
        .cockpit(&[
            "resources",
            "check",
            "agent-turn",
            "--cpu",
            "500",
            "--memory",
            "256",
        ])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "fits\n");
}

#[tokio::test]
async fn resources_check_without_a_demand_is_a_usage_error() {
    let home = Home::new();

    let output = home
        .cockpit(&["resources", "check", "build", "--cpu", "500"])
        .await;

    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).starts_with("cockpit: check needs --memory MIB\n"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn each_settings_verb_sends_its_command() {
    for (arguments, command) in [
        (
            ["profile", "foreground"].as_slice(),
            Command::ResourceProfile {
                profile: ProfileView::Foreground,
            },
        ),
        (
            ["profile", "normal"].as_slice(),
            Command::ResourceProfile {
                profile: ProfileView::Normal,
            },
        ),
        (
            ["pause-queue"].as_slice(),
            Command::ResourceQueue { paused: true },
        ),
        (
            ["resume-queue"].as_slice(),
            Command::ResourceQueue { paused: false },
        ),
    ] {
        let home = Home::new();
        let daemon = scripted_daemon(
            &home.daemon_socket(),
            Outcome::Ok(Reply::ResourceStatus {
                status: Box::new(ResourceStatusView {
                    platform: "linux".to_owned(),
                    containment: "systemd-scope".to_owned(),
                    profile: ProfileView::Normal,
                    queue_paused: false,
                    budget: DemandView {
                        cpu_millis: 1000,
                        memory_mib: 1024,
                    },
                    in_use: DemandView {
                        cpu_millis: 0,
                        memory_mib: 0,
                    },
                    foreground_heavy_percent: 25,
                    controls: vec![],
                    reservations: vec![],
                }),
            }),
        );
        let mut words = vec!["resources"];
        words.extend(arguments);

        let output = home.cockpit(&words).await;

        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
        assert_eq!(daemon.await.unwrap(), command, "{arguments:?}");
    }
}

#[tokio::test]
async fn resources_checkpoint_prints_the_reservation_marked() {
    let home = Home::new();
    let mut marked = running_build();
    marked.checkpoint_requested = true;
    marked.limits.clear();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Reservation {
            reservation: Box::new(marked),
        }),
    );

    let output = home.cockpit(&["resources", "checkpoint", "3"]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "reservation 3\tbuild\trunning (checkpoint requested)\texecution 12\treserved 2000 millicores 1024 MiB\tmake test\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::ResourceCheckpoint { reservation: 3 }
    );
}

#[tokio::test]
async fn resources_stop_exits_1_when_the_execution_was_not_observed_ended() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::ResourceStopped {
            reservation: 3,
            observed_ended: false,
        }),
    );

    let output = home.cockpit(&["resources", "stop", "3"]).await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stdout),
        "reservation 3: stop requested, but its execution was not observed ended; it keeps its reservation\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::ResourceStop { reservation: 3 }
    );
}
