//! Issue #164's measurement: what an agent's `--wait` polls and the owner's queue cost the
//! daemon, through its sockets. Ignored by default: run with
//! `cargo nextest run --release --manifest-path Cargo.toml -p cockpit-daemon
//! --test approvals_poll_cost --run-ignored only --no-capture`.
#![cfg(target_os = "linux")]

mod common;

use std::time::{Duration, Instant};

use cockpit_agent_channel::identity_of;
use cockpit_authorization::Permission;
use cockpit_client::Connection;
use cockpit_protocol::{Command, Reply};
use common::{GENEROUS, TestHome};

const POLLS: u32 = 500;
const QUEUE_READS: u32 = 200;
const GUIDES: usize = 20;
const GUIDE: &str = "# Connect the app\nOne\nTwo\nThree\nFour\nFive\n";

async fn timed(times: u32, agent: &Connection, command: impl Fn() -> Command) -> Duration {
    let started = Instant::now();
    for _ in 0..times {
        agent.request(command(), GENEROUS).await.unwrap();
    }
    started.elapsed() / times
}

async fn change_rows(home: &TestHome) -> i64 {
    let database = home.open_state().await;
    let read = database.read().await.unwrap();
    read.query("SELECT count(*) FROM turso_cdc", ())
        .await
        .unwrap()
        .next()
        .await
        .unwrap()
        .unwrap()
        .get(0)
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement, not a check: run it before and after a change and compare"]
async fn measure_status_polls_and_the_owner_queue() {
    let home = TestHome::new();
    let token = home
        .launch(
            "s1",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::RunCommands),
        )
        .await;
    let cwd = std::fs::canonicalize(home.home()).unwrap();
    let cwd = cwd.to_str().unwrap().to_owned();
    let daemon = home.start();
    let agent = home.connect_agent().await;

    let Reply::RunRequest { request } = agent
        .request(
            Command::AskRun {
                launch_token: Some(token.as_str().to_owned()),
                command: "true".to_owned(),
                cwd: cwd.clone(),
                reason: "Measures the poll cost.".to_owned(),
            },
            GENEROUS,
        )
        .await
        .unwrap()
    else {
        panic!("expected a run request");
    };
    let mut guides = Vec::new();
    for _ in 0..GUIDES {
        let Reply::Guide { guide } = agent
            .request(
                Command::AskGuide {
                    launch_token: None,
                    guide: GUIDE.to_owned(),
                    cwd: cwd.clone(),
                },
                GENEROUS,
            )
            .await
            .unwrap()
        else {
            panic!("expected a guide");
        };
        guides.push(guide.id);
    }
    daemon.kill();
    let rows_before = change_rows(&home).await;
    let daemon = home.start();
    let agent = home.connect_agent().await;
    let owner = home.connect().await;

    let run_poll = timed(POLLS, &agent, || Command::AskRunStatus {
        launch_token: None,
        request: request.id,
    })
    .await;
    let guide_poll = timed(POLLS, &agent, || Command::AskGuideStatus {
        launch_token: None,
        guide: guides[0],
    })
    .await;
    let queue = timed(QUEUE_READS, &owner, || Command::ApprovalQueue).await;
    daemon.kill();
    let rows_written = change_rows(&home).await - rows_before;

    println!(
        "ask run status: {run_poll:?}/poll; ask guide status: {guide_poll:?}/poll; \
         approval queue ({GUIDES} guides x 5 steps): {queue:?}/read; \
         change rows written by {} polls: {rows_written}",
        2 * POLLS
    );
}
