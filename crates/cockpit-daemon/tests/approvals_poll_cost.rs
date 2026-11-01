//! Issue #164's measurement: what an agent's `--wait` polls and the owner's queue cost the
//! daemon, through its sockets; issue #348's: what the queue's per-session authorization
//! reads (#285) cost when many sessions ask, some of them ended. Ignored by default: run with
//! `cargo nextest run --release --manifest-path Cargo.toml -p cockpit-daemon
//! --test approvals_poll_cost --run-ignored only --no-capture`.
#![cfg(target_os = "linux")]

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cockpit_agent_channel::identity_of;
use cockpit_approvals::RunAsk;
use cockpit_authorization::{Permission, Scope, SessionId};
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

/// The newest change-feed id: its growth is the change rows written in between.
async fn change_rows(home: &TestHome) -> i64 {
    let database = home.open_state().await;
    let read = database.read().await.unwrap();
    read.revision().await.unwrap()
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

/// The asks one queue holds in every #348 layout: the same rows, spread over more sessions.
const ASKS_RUNS: usize = 40;
const ASKS_GUIDES: usize = 40;

fn unix_now() -> i64 {
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    i64::try_from(elapsed.as_secs()).unwrap()
}

/// Files [`ASKS_RUNS`] run requests and [`ASKS_GUIDES`] guides spread evenly over `sessions`
/// sessions, each granted `run-commands`, then revokes the grant of the first `ended` of
/// them, as ending a session does. Written while no daemon runs.
async fn asked_by_sessions(home: &TestHome, cwd: &PathBuf, sessions: usize, ended: usize) {
    let database = home.open_state().await;
    let write = database.write().await.unwrap();
    let now = unix_now();
    let guide = cockpit_approvals::parse_guide(GUIDE).unwrap();
    let mut grants = Vec::new();
    for index in 0..sessions {
        let session = SessionId::new(format!("s{index}")).unwrap();
        let scope = Scope::Session(session.clone());
        grants.push(
            cockpit_authorization::grant(&write, &session, Permission::RunCommands, &scope)
                .await
                .unwrap(),
        );
    }
    for ask in 0..ASKS_RUNS {
        let session = &grants[ask % sessions].holder;
        let run = RunAsk {
            command: "true".to_owned(),
            cwd: cwd.clone(),
            reason: "Measures the queue.".to_owned(),
            project: None,
        };
        cockpit_approvals::ask_run(&write, session, run, now)
            .await
            .unwrap();
    }
    for ask in 0..ASKS_GUIDES {
        let session = &grants[ask % sessions].holder;
        cockpit_approvals::ask_guide(&write, session, &guide, cwd.clone(), None, now)
            .await
            .unwrap();
    }
    for grant in &grants[..ended] {
        cockpit_authorization::revoke(&write, grant.id)
            .await
            .unwrap();
    }
    write.commit().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "a measurement, not a check: run it before and after a change and compare"]
async fn measure_the_owner_queue_with_many_sessions_asking() {
    for (sessions, ended) in [(1, 0), (10, 0), (10, 5), (40, 0), (40, 20)] {
        let home = TestHome::new();
        let cwd = std::fs::canonicalize(home.home()).unwrap();
        asked_by_sessions(&home, &cwd, sessions, ended).await;
        let daemon = home.start();
        let owner = home.connect().await;

        let Reply::ApprovalQueue { runs, guides } = owner
            .request(Command::ApprovalQueue, GENEROUS)
            .await
            .unwrap()
        else {
            panic!("expected the queue");
        };
        let queue = timed(QUEUE_READS, &owner, || Command::ApprovalQueue).await;
        daemon.kill();

        println!(
            "{sessions} sessions asking ({ended} ended), {ASKS_RUNS} runs + {ASKS_GUIDES} guides \
             x 5 steps: queue shows {} runs + {} guides in {queue:?}/read",
            runs.len(),
            guides.len()
        );
    }
}
