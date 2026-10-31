//! Issue #36: the control path's latency under load (report 03's targets, report 15's
//! claim), through the daemon binary. While every load below runs at once, a probe on its
//! own thread sends one control request every [`PROBE_INTERVAL`] on a held connection and
//! pings on a new connection every second. A latency counts from when its request was due,
//! so requests a stalled daemon delayed are counted as late, not skipped. Afterwards the
//! daemon is killed with its sessions running and started again: each is reattached and
//! none is launched again.
//!
//! The loads (workload decided in docs/decisions/2026-10-31-36-latency-workload.md):
//! - build: per build process, `head -c 64M /dev/urandom | gzip -1` into a file, looped,
//!   outside the daemon as builds run in agent terminals;
//! - log flood: a session whose agent writes to its terminal without pause, and a viewer
//!   that subscribed to the change feed and never reads it;
//! - stalled provider: a session whose agent ignores `SIGTERM`, restarted back to back, so
//!   each restart holds a bulk worker through the stop's grace;
//! - database contention: owner writers that add and delete items back to back, and a
//!   session started and ended back to back.
//!
//! The smoke test runs the same harness with light load in CI and asserts the harness, not
//! the targets. `recorded_latency_run` is the measured run of docs/gates/m1-latency.md;
//! it reports whether each target was met and fails only on a refused control request or
//! a broken restart.
#![cfg(target_os = "linux")]

mod real_daemon;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{
    Capability, Command, ErrorCode, FeedPosition, Hello, ProtocolError, Reply, Request, RequestId,
    SUPPORTED_VERSIONS, SessionStateView,
};
use real_daemon::{GENEROUS, Home, listed, new_session};
use tokio::io::AsyncWriteExt;

const PROBE_INTERVAL: Duration = Duration::from_millis(50);
const FRESH_CONNECTION_INTERVAL: Duration = Duration::from_secs(1);
/// A load request may wait out a stalled agent's stop: grace and kill wait, 5 s each.
const LOAD_DEADLINE: Duration = Duration::from_secs(60);
/// Report 03's targets.
const P95_TARGET: Duration = Duration::from_millis(100);
const P99_TARGET: Duration = Duration::from_millis(250);
const STALL: Duration = Duration::from_secs(1);

struct Workload {
    duration: Duration,
    build_processes: usize,
    writers: usize,
}

impl Workload {
    fn smoke() -> Workload {
        Workload {
            duration: Duration::from_secs(3),
            build_processes: 1,
            writers: 1,
        }
    }

    /// `COCKPIT_LATENCY_SECONDS` (1800 by default) of load with one build process per CPU.
    fn recorded() -> Workload {
        let seconds = std::env::var("COCKPIT_LATENCY_SECONDS").map_or(1800, |value| {
            value
                .parse()
                .expect("COCKPIT_LATENCY_SECONDS is a number of seconds")
        });
        Workload {
            duration: Duration::from_secs(seconds),
            build_processes: std::thread::available_parallelism().map_or(1, usize::from),
            writers: 2,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn control_requests_are_answered_while_every_load_runs_and_a_restart_reattaches() {
    run(Workload::smoke()).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "the recorded run of docs/gates/m1-latency.md: 30 minutes on every CPU"]
async fn recorded_latency_run() {
    run(Workload::recorded()).await;
}

async fn run(workload: Workload) {
    let home = Home::new();
    let daemon = home.start();
    let client = home.connect().await;
    let mut dirs = Vec::new();
    for name in ["flood", "stall", "churn", "tasks"] {
        dirs.push(home.project(&client, name).await);
    }
    let [
        (flood_dir, _),
        (stall_dir, _),
        (churn_dir, _),
        (_, tasks_project),
    ] = <[_; 4]>::try_from(dirs).unwrap();
    let flood = new_session(&client, &flood_dir).await;
    let stall = new_session(&client, &stall_dir).await;
    home.wait_for_launches(&flood.id, 1).await;
    home.wait_for_launches(&stall.id, 1).await;
    let _stalled_viewer = stalled_viewer(&home.socket_path()).await;

    let stop = Arc::new(AtomicBool::new(false));
    let memory = MemorySampler::start(daemon.pid(), Arc::clone(&stop));
    let builds = Builds::start(&home.path(), workload.build_processes);
    let probe = probe(home.socket_path(), workload.duration);
    let mut loads = vec![
        tokio::spawn(restart_back_to_back(
            home.connect().await,
            stall.id.clone(),
            Arc::clone(&stop),
        )),
        tokio::spawn(start_and_end_back_to_back(
            home.connect().await,
            churn_dir.clone(),
            Arc::clone(&stop),
        )),
    ];
    for _ in 0..workload.writers {
        loads.push(tokio::spawn(add_and_delete_back_to_back(
            home.connect().await,
            tasks_project.clone(),
            Arc::clone(&stop),
        )));
    }
    let samples = tokio::task::spawn_blocking(move || probe.join().expect("the probe ran"))
        .await
        .unwrap();
    stop.store(true, Ordering::SeqCst);
    let mut tallies = Vec::new();
    for load in loads {
        tallies.push(load.await.expect("a load task ran"));
    }
    drop(builds);
    let memory = memory.finish();

    // The stalled session's agent logged its start and one launch per answered restart.
    let stall_launches = 1 + usize::try_from(tallies[0].answered).unwrap();
    home.wait_for_launches(&stall.id, stall_launches).await;
    let restart = restart_with_sessions_running(&home, daemon, &[&flood.id, &stall.id]).await;

    let report = Report {
        workload: &workload,
        samples: &samples,
        tallies: &tallies,
        memory,
        restart: &restart,
    }
    .to_markdown();
    println!("{report}");
    if let Some(path) = std::env::var_os("COCKPIT_LATENCY_REPORT") {
        std::fs::write(&path, &report).expect("the report is writable");
    }

    let refused: Vec<_> = samples
        .iter()
        .filter(|sample| sample.error.is_some())
        .collect();
    assert!(refused.is_empty(), "control requests failed: {refused:?}");
    assert!(
        tallies.iter().all(|tally| tally.attempts() > 0),
        "every load ran: {tallies:?}"
    );
    assert!(
        restart.reattached,
        "every session kept its agent: {restart:?}"
    );
    assert_eq!(
        restart.launches_after, restart.launches_before,
        "no launch after the restart"
    );
}

#[derive(Debug)]
struct Sample {
    request: &'static str,
    latency: Duration,
    error: Option<String>,
}

/// The control requests the probe cycles through: what a client asks to show status.
fn probe_command(index: u32) -> (&'static str, Command) {
    match index % 5 {
        0 => ("ping", Command::Ping),
        1 => ("session-list", Command::SessionList { ended: false }),
        2 => ("agent-reports", Command::AgentReports),
        3 => ("approval-queue", Command::ApprovalQueue),
        _ => ("provider-list", Command::ProviderList),
    }
}

/// Runs the probe on its own thread and runtime, apart from the loads' tasks.
fn probe(socket: PathBuf, duration: Duration) -> std::thread::JoinHandle<Vec<Sample>> {
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (held, fresh) = tokio::join!(
                    probe_held_connection(&socket, duration),
                    probe_fresh_connections(&socket, duration)
                );
                held.into_iter().chain(fresh).collect()
            })
    })
}

async fn probe_held_connection(socket: &Path, duration: Duration) -> Vec<Sample> {
    let client = Connection::open(socket, GENEROUS)
        .await
        .expect("the probe connects");
    let start = tokio::time::Instant::now();
    let mut samples = Vec::new();
    for index in 0.. {
        let due = start + PROBE_INTERVAL * index;
        if due - start >= duration {
            break;
        }
        tokio::time::sleep_until(due).await;
        let (request, command) = probe_command(index);
        let answer = client.request(command, GENEROUS).await;
        samples.push(Sample {
            request,
            latency: due.elapsed(),
            error: answer.err().map(|error| error.to_string()),
        });
    }
    samples
}

async fn probe_fresh_connections(socket: &Path, duration: Duration) -> Vec<Sample> {
    let start = tokio::time::Instant::now();
    let mut samples = Vec::new();
    for index in 0.. {
        let due = start + FRESH_CONNECTION_INTERVAL * index;
        if due - start >= duration {
            break;
        }
        tokio::time::sleep_until(due).await;
        let answer = match Connection::open(socket, GENEROUS).await {
            Ok(client) => client.request(Command::Ping, GENEROUS).await.map(drop),
            Err(error) => Err(error),
        };
        samples.push(Sample {
            request: "connect+ping",
            latency: due.elapsed(),
            error: answer.err().map(|error| error.to_string()),
        });
    }
    samples
}

/// What one load's requests got.
#[derive(Debug)]
struct Tally {
    load: &'static str,
    answered: u64,
    busy: u64,
    failed: u64,
    first_failure: Option<String>,
}

impl Tally {
    fn new(load: &'static str) -> Tally {
        Tally {
            load,
            answered: 0,
            busy: 0,
            failed: 0,
            first_failure: None,
        }
    }

    fn attempts(&self) -> u64 {
        self.answered + self.busy + self.failed
    }

    /// Counts `answer`; a client that was told `busy` waits a little before it asks again.
    async fn record(&mut self, answer: Result<Reply, ClientError>) -> Option<Reply> {
        match answer {
            Ok(reply) => {
                self.answered += 1;
                return Some(reply);
            }
            Err(ClientError::Daemon(ProtocolError {
                code: ErrorCode::Busy,
                ..
            })) => self.busy += 1,
            Err(error) => {
                self.failed += 1;
                self.first_failure.get_or_insert_with(|| error.to_string());
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        None
    }
}

async fn restart_back_to_back(client: Connection, session: String, stop: Arc<AtomicBool>) -> Tally {
    let mut tally = Tally::new("restart the stalled agent");
    while !stop.load(Ordering::SeqCst) {
        let command = Command::SessionRestart {
            session: session.clone(),
        };
        tally
            .record(client.request(command, LOAD_DEADLINE).await)
            .await;
    }
    tally
}

async fn start_and_end_back_to_back(
    client: Connection,
    dir: PathBuf,
    stop: Arc<AtomicBool>,
) -> Tally {
    let mut tally = Tally::new("start and end a session");
    while !stop.load(Ordering::SeqCst) {
        let command = Command::SessionNew {
            dir: dir.display().to_string(),
            agent: Some("claude".to_owned()),
            prompt: None,
            clone: None,
        };
        let started = tally
            .record(client.request(command, LOAD_DEADLINE).await)
            .await;
        if let Some(Reply::Session { session }) = started {
            let command = Command::SessionEnd {
                session: session.id,
            };
            tally
                .record(client.request(command, LOAD_DEADLINE).await)
                .await;
        }
    }
    tally
}

async fn add_and_delete_back_to_back(
    client: Connection,
    project: String,
    stop: Arc<AtomicBool>,
) -> Tally {
    let mut tally = Tally::new("add and delete an item");
    while !stop.load(Ordering::SeqCst) {
        let command = Command::TaskAdd {
            launch_token: None,
            project: Some(project.clone()),
            title: "latency fixture item".to_owned(),
            notes: "written to keep the database writer busy".to_owned(),
            item_kind: None,
            priority: None,
            after: Vec::new(),
        };
        let added = tally
            .record(client.request(command, LOAD_DEADLINE).await)
            .await;
        if let Some(Reply::Task { task }) = added {
            let command = Command::TaskDelete {
                launch_token: None,
                item: task.id,
            };
            tally
                .record(client.request(command, LOAD_DEADLINE).await)
                .await;
        }
    }
    tally
}

/// A connection that subscribed to the change feed and never reads what it is sent.
async fn stalled_viewer(socket: &Path) -> tokio::net::UnixStream {
    let mut stream = tokio::net::UnixStream::connect(socket).await.unwrap();
    let hello = Hello {
        protocol_versions: SUPPORTED_VERSIONS.to_vec(),
        capabilities: vec![Capability::ChangeFeed],
    };
    let subscribe = Request {
        id: RequestId(1),
        command: Command::Subscribe {
            from: FeedPosition::Snapshot,
        },
    };
    let frames = format!(
        "{}\n{}\n",
        serde_json::to_string(&hello).unwrap(),
        serde_json::to_string(&subscribe).unwrap()
    );
    stream.write_all(frames.as_bytes()).await.unwrap();
    stream
}

/// The build stand-ins, each the leader of its own process group so ending it ends its
/// pipeline too.
struct Builds {
    processes: Vec<Child>,
}

impl Builds {
    fn start(dir: &Path, count: usize) -> Builds {
        use std::os::unix::process::CommandExt;
        let processes = (0..count)
            .map(|index| {
                let output = dir.join(format!("build-{index}.gz"));
                Process::new("sh")
                    .arg("-c")
                    .arg(r#"while :; do head -c 64M /dev/urandom | gzip -1 > "$1"; done"#)
                    .arg("build")
                    .arg(output)
                    .process_group(0)
                    .spawn()
                    .expect("a build stand-in starts")
            })
            .collect();
        Builds { processes }
    }
}

impl Drop for Builds {
    fn drop(&mut self) {
        for process in &mut self.processes {
            let _ = Process::new("kill")
                .arg("-KILL")
                .arg(format!("-{}", process.id()))
                .status();
            let _ = process.wait();
        }
    }
}

/// The daemon's resident memory, sampled each second until the run stops.
struct MemorySampler {
    thread: std::thread::JoinHandle<Memory>,
}

#[derive(Debug, Clone, Copy)]
struct Memory {
    first_kib: u64,
    peak_kib: u64,
    last_kib: u64,
}

impl MemorySampler {
    fn start(pid: u32, stop: Arc<AtomicBool>) -> MemorySampler {
        let thread = std::thread::spawn(move || {
            let first = resident_kib(pid);
            let mut memory = Memory {
                first_kib: first,
                peak_kib: first,
                last_kib: first,
            };
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_secs(1));
                memory.last_kib = resident_kib(pid);
                memory.peak_kib = memory.peak_kib.max(memory.last_kib);
            }
            memory
        });
        MemorySampler { thread }
    }

    fn finish(self) -> Memory {
        self.thread.join().expect("the memory sampler ran")
    }
}

fn resident_kib(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap_or_else(|| panic!("the daemon's VmRSS is readable: {status:?}"))
}

#[derive(Debug)]
struct Restart {
    ready_after: Duration,
    reattached: bool,
    launches_before: usize,
    launches_after: usize,
}

/// Kills the daemon while `sessions` run and starts it again.
async fn restart_with_sessions_running(
    home: &Home,
    daemon: real_daemon::Daemon,
    sessions: &[&str],
) -> Restart {
    let client = home.connect().await;
    let mut launches_seen = Vec::new();
    for id in sessions {
        launches_seen.push(listed(&client, id).await.launch);
    }
    let launches_before = home.launches().len();
    drop(client);
    daemon.kill();
    let killed = Instant::now();
    let _daemon = home.start();
    let ready_after = killed.elapsed();
    let client = home.connect().await;
    let mut reattached = true;
    for (id, launch) in sessions.iter().zip(&launches_seen) {
        let session = listed(&client, id).await;
        reattached &= session.state == SessionStateView::Running && &session.launch == launch;
    }
    Restart {
        ready_after,
        reattached,
        launches_before,
        launches_after: home.launches().len(),
    }
}

struct Report<'a> {
    workload: &'a Workload,
    samples: &'a [Sample],
    tallies: &'a [Tally],
    memory: Memory,
    restart: &'a Restart,
}

impl Report<'_> {
    fn to_markdown(&self) -> String {
        let mut text = String::new();
        let workload = self.workload;
        let _ = writeln!(
            text,
            "## Latency run\n\n- Load for {} s; a control request every {} ms on a held \
             connection, a new connection's ping every {} s\n- Build processes: {}; item \
             writers: {}\n",
            workload.duration.as_secs(),
            PROBE_INTERVAL.as_millis(),
            FRESH_CONNECTION_INTERVAL.as_secs(),
            workload.build_processes,
            workload.writers,
        );
        let _ = writeln!(
            text,
            "| Request | Count | Failed | p50 ms | p95 ms | p99 ms | Max ms |\n| --- | --- \
             | --- | --- | --- | --- | --- |"
        );
        let held: Vec<_> = self
            .samples
            .iter()
            .filter(|sample| sample.request != "connect+ping")
            .collect();
        self.row(&mut text, "all control (held connection)", &held);
        for request in [
            "ping",
            "session-list",
            "agent-reports",
            "approval-queue",
            "provider-list",
            "connect+ping",
        ] {
            let samples: Vec<_> = self
                .samples
                .iter()
                .filter(|sample| sample.request == request)
                .collect();
            self.row(&mut text, request, &samples);
        }
        let held_latencies = sorted_latencies(&held);
        let stalls = self
            .samples
            .iter()
            .filter(|sample| sample.latency > STALL)
            .count();
        let p95 = percentile(&held_latencies, 0.95);
        let p99 = percentile(&held_latencies, 0.99);
        let _ = writeln!(
            text,
            "\n### Targets (report 03)\n\n- p95 ≤ {} ms: {} ({})\n- p99 ≤ {} ms: {} ({})\n- \
             No answer later than {} s: {} ({stalls} later)\n",
            P95_TARGET.as_millis(),
            verdict(p95 <= P95_TARGET),
            milliseconds(p95),
            P99_TARGET.as_millis(),
            verdict(p99 <= P99_TARGET),
            milliseconds(p99),
            STALL.as_secs(),
            verdict(stalls == 0),
        );
        let _ = writeln!(
            text,
            "### Loads\n\n| Load | Answered | Busy | Failed | First failure |\n| --- | --- | \
             --- | --- | --- |"
        );
        for tally in self.tallies {
            let _ = writeln!(
                text,
                "| {} | {} | {} | {} | {} |",
                tally.load,
                tally.answered,
                tally.busy,
                tally.failed,
                tally.first_failure.as_deref().unwrap_or("none"),
            );
        }
        let memory = self.memory;
        let restart = self.restart;
        let _ = writeln!(
            text,
            "\n### Daemon\n\n- Resident memory: {} MiB at the start, {} MiB at the peak, {} \
             MiB at the end\n- Killed with its sessions running, then ready again after {} \
             ms; sessions reattached: {}; agent launches {} before, {} after",
            memory.first_kib / 1024,
            memory.peak_kib / 1024,
            memory.last_kib / 1024,
            restart.ready_after.as_millis(),
            if restart.reattached { "all" } else { "NOT ALL" },
            restart.launches_before,
            restart.launches_after,
        );
        text
    }

    fn row(&self, text: &mut String, name: &str, samples: &[&Sample]) {
        let latencies = sorted_latencies(samples);
        let failed = samples
            .iter()
            .filter(|sample| sample.error.is_some())
            .count();
        let _ = writeln!(
            text,
            "| {name} | {} | {failed} | {} | {} | {} | {} |",
            latencies.len(),
            milliseconds(percentile(&latencies, 0.50)),
            milliseconds(percentile(&latencies, 0.95)),
            milliseconds(percentile(&latencies, 0.99)),
            milliseconds(latencies.last().copied().unwrap_or_default()),
        );
    }
}

fn sorted_latencies(samples: &[&Sample]) -> Vec<Duration> {
    let mut latencies: Vec<_> = samples.iter().map(|sample| sample.latency).collect();
    latencies.sort_unstable();
    latencies
}

/// Nearest rank; zero for no samples.
fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn milliseconds(duration: Duration) -> String {
    format!("{:.1}", duration.as_secs_f64() * 1000.0)
}

fn verdict(met: bool) -> &'static str {
    if met { "met" } else { "missed" }
}
