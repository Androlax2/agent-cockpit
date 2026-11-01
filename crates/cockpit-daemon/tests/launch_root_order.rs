//! Issue #316 at the agent socket: a launched agent runs only once its launch's process tree
//! (root) is recorded, so its very first request is identified, and the daemon holds no
//! request waiting for a root (#279's wait is gone).
#![cfg(target_os = "linux")]

mod common;
mod real_daemon;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_client::Connection;
use cockpit_daemon::{Instance, Limits, Options, Routes};
use cockpit_protocol::{AgentReport, Command, ErrorCode, Reply};
use real_daemon::{GENEROUS, Home, REPORT_PROJECT, REPORTED_LABEL, cockpit_program, new_session};

/// Agents launched at once, each of which reports as the first thing it does.
const AGENTS: usize = 3;

/// Far below the 5 s #279 held such a request for.
const AT_ONCE: Duration = Duration::from_secs(1);

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_first_report_lands_without_a_retry() {
    assert!(
        cockpit_program().is_file(),
        "{} is missing: run the workspace's tests (`make crates-test`), which build it",
        cockpit_program().display()
    );
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let mut launched = Vec::new();
    for index in 0..AGENTS {
        let (root, _) = home
            .project(&client, &format!("{REPORT_PROJECT}-{index}"))
            .await;
        launched.push(new_session(&client, &root).await.id);
    }

    let deadline = Instant::now() + GENEROUS;
    loop {
        let reports = match client.request(Command::AgentReports, GENEROUS).await {
            Ok(Reply::AgentReports { reports }) => reports,
            other => panic!("agent-reports answered {other:?}"),
        };
        let landed = launched.iter().all(|id| {
            reports.iter().any(|report| {
                &report.session == id && report.label.as_deref() == Some(REPORTED_LABEL)
            })
        });
        if landed {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "not every agent's single report landed; they printed {:?}",
            std::fs::read_to_string(home.report_log()).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_whose_launch_has_no_root_is_refused_at_once() {
    let home = tempfile::tempdir().unwrap();
    let kept = Arc::new(Mutex::new(None));
    let options = Options {
        runtime_dir: home.path().join("run/cockpit"),
        state_dir: home.path().join("state/cockpit"),
        limits: Limits::default(),
    };
    let daemon = Instance::acquire(options)
        .unwrap()
        .start(common::wiring_in(home.path(), Vec::new()), {
            let kept = Arc::clone(&kept);
            move |capabilities| {
                *kept.lock().unwrap() = Some(Arc::clone(&capabilities.database));
                Arc::new(Routes::new(
                    capabilities,
                    cockpit_config::Environment::default(),
                ))
            }
        })
        .await
        .unwrap();
    let database = kept.lock().unwrap().take().unwrap();
    let session = SessionId::new("s1").unwrap();
    let token = {
        let write = database.write().await.unwrap();
        let token = cockpit_agent_channel::mint(&write, &session).await.unwrap();
        cockpit_authorization::grant(
            &write,
            &session,
            Permission::ChangeState,
            &Scope::Session(session.clone()),
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        token
    };
    let agent = Connection::open(daemon.agent_socket_path(), common::GENEROUS)
        .await
        .unwrap();

    let started = Instant::now();
    let reply = agent
        .request(
            Command::AgentReport {
                launch_token: Some(token.as_str().to_owned()),
                report: AgentReport {
                    label: Some("a report from no known process".to_owned()),
                    ..AgentReport::default()
                },
            },
            common::GENEROUS,
        )
        .await;

    let refusal = common::refused(reply);
    assert!(
        started.elapsed() < AT_ONCE,
        "answered after {:?}",
        started.elapsed()
    );
    assert_eq!(refusal.code, ErrorCode::UnidentifiedPeer, "{refusal:?}");
}
