//! #1351: through the real daemon binary, what agents run in their sessions yields. In every
//! profile a session's processes run at nice 19 in the idle IO class; under `foreground` they
//! also get `SCHED_IDLE` and the lowest CPU weight, live, and lose those again on `normal`. A
//! ceiling this host could not take off again is never set, and `resources status` says so.
#![cfg(target_os = "linux")]

mod candidate;
mod real_daemon;

use cockpit_client::Connection;
use cockpit_protocol::{
    Command, EffectiveView, ProfileView, Reply, ResourceStatusView, SessionLimitsView,
};
use real_daemon::{GENEROUS, Home, new_session, wait_for_running};

async fn set_profile(client: &Connection, profile: ProfileView) -> ResourceStatusView {
    match client
        .request(Command::ResourceProfile { profile }, GENEROUS)
        .await
    {
        Ok(Reply::ResourceStatus { status }) => *status,
        other => panic!("resource-profile: {other:?}"),
    }
}

/// Starts a session and returns its id and its agent's process: the first one it ran.
async fn running_agent(home: &Home, client: &Connection) -> (String, i32) {
    let (root, _) = home.project(client, "app").await;
    let session = new_session(client, &root).await;
    let launches = home.wait_for_launches(&session.id, 1).await;
    wait_for_running(client, &session.id).await;
    (session.id, launches[0].pid)
}

/// Whether this host sets `control` on running sessions, as the status says.
fn sets(status: &ResourceStatusView, control: &str) -> bool {
    status
        .session_controls
        .iter()
        .find(|support| support.control == control)
        .unwrap_or_else(|| panic!("no session ceiling {control}: {status:#?}"))
        .supported
}

fn session<'a>(status: &'a ResourceStatusView, id: &str) -> &'a SessionLimitsView {
    status
        .sessions
        .iter()
        .find(|session| session.session == id)
        .unwrap_or_else(|| panic!("session {id} is not in the status: {status:#?}"))
}

/// Niceness and whether the policy is `SCHED_IDLE`, from `/proc/<pid>/stat`.
fn priority(pid: i32) -> (i32, bool) {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<&str> = stat
        .rsplit_once(") ")
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    (fields[19 - 3].parse().unwrap(), fields[41 - 3] == "5")
}

fn io_class(pid: i32) -> String {
    let output = std::process::Command::new("ionice")
        .args(["-p", &pid.to_string()])
        .output()
        .expect("ionice runs");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// Nice 19 and the idle IO class: what every session process has, in every profile.
fn assert_always_yields(status: &ResourceStatusView, pid: i32) {
    assert!(sets(status, "nice"), "{status:#?}");
    assert_eq!(priority(pid).0, 19, "process {pid}");
    if sets(status, "idle-io") {
        assert_eq!(io_class(pid), "idle", "process {pid}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_started_under_normal_runs_at_nice_19_in_the_idle_io_class() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;

    let (_, agent) = running_agent(&home, &client).await;

    let status = match client.request(Command::ResourceStatus, GENEROUS).await {
        Ok(Reply::ResourceStatus { status }) => *status,
        other => panic!("resource-status: {other:?}"),
    };
    assert_eq!(status.profile, ProfileView::Normal);
    assert_always_yields(&status, agent);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_sessions_agent_gets_the_hard_ceilings_under_foreground_and_loses_them_on_normal()
{
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (id, agent) = running_agent(&home, &client).await;
    let was_idle = priority(agent).1;

    let foreground = set_profile(&client, ProfileView::Foreground).await;

    assert_always_yields(&foreground, agent);
    assert_eq!(
        priority(agent).1,
        sets(&foreground, "idle-scheduling") || was_idle
    );
    for line in &session(&foreground, &id).limits {
        if line.supported && !line.control.starts_with("cpu-") {
            assert_eq!(
                line.effective,
                EffectiveView::Value {
                    value: line.requested.clone().unwrap()
                },
                "{line:#?}"
            );
        }
    }

    let normal = set_profile(&client, ProfileView::Normal).await;

    assert_always_yields(&normal, agent);
    if sets(&normal, "idle-scheduling") {
        assert!(!priority(agent).1, "the agent is still SCHED_IDLE");
    }
    let requested: Vec<&str> = session(&normal, &id)
        .limits
        .iter()
        .filter(|line| line.requested.is_some())
        .map(|line| line.control.as_str())
        .collect();
    assert_eq!(requested, ["nice", "idle-io"], "{normal:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_started_under_foreground_yields_from_its_first_process() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let test_is_idle = priority(std::process::id().try_into().unwrap()).1;
    let foreground = set_profile(&client, ProfileView::Foreground).await;

    let (_, agent) = running_agent(&home, &client).await;

    assert_always_yields(&foreground, agent);
    assert_eq!(
        priority(agent).1,
        sets(&foreground, "idle-scheduling") || test_is_idle
    );
}
