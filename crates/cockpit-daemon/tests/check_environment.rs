//! #352 through the real daemon binary: under `agents.sandbox = "required"` a declared check runs
//! through `torlek-sandbox` (#862, Q1). From inside it the check cannot read the owner's keys or
//! the daemon's state, cannot reach the owner's socket, has none of the daemon's other variables
//! (#862, Q2), writes nothing into the owner's caches and is refused a host its project was
//! never granted; its receipt records the sandbox that ran it. On a host without the sandbox
//! the check is recorded `unavailable` and never runs; under `"off"` it runs on the host. A
//! check that passes in the sandbox passes the gate (#352's acceptance). A host the check was
//! refused reaches the owner as a request whose subject is the check run.

mod candidate;
mod real_daemon;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::time::{Duration, Instant};

use cockpit_protocol::{
    CheckOutcomeView, CheckReceiptView, Command, GateOutcomeView, NetworkSubjectView, Reply,
};
use real_daemon::{GENEROUS, Home};

const REQUIRED: &str = "[agents]\nsandbox = \"required\"\n";
const DAEMON_TOKEN: &str = "the-daemons-own-token";

/// The probe: each line says what the check saw of the owner's host from where it ran.
const PROBE: &str = r#"attempt() { if cat "$2" >/dev/null 2>&1; then echo "$1=read"; else echo "$1=denied"; fi; }
attempt ssh_key '@HOME@/.ssh/id_ed25519'
attempt state_db '@STATE@/state.db'
if [ -e '@SOCKET@' ]; then echo socket=visible; else echo socket=hidden; fi
echo "gh_token=${GH_TOKEN:-unset}"
if touch '@HOME@/.cargo/written' 2>/dev/null; then echo cargo=written; else echo cargo=refused; fi
touch "$TMPDIR/scratch" && echo tmp=writable
curl -sS -o /dev/null --max-time 10 -w 'tunnel=%{http_connect}\n' https://example.com/ 2>/dev/null
touch '@MARKER@'
"#;

/// Passes only where the checkout is readable and the owner's key is not: in the sandbox.
const CONFINED: &str = "test -f confined.sh && ! cat '@HOME@/.ssh/id_ed25519' >/dev/null 2>&1\n";

/// Asks for a host the project was never granted; the proxy refuses it without a lookup.
const ASKING: &str = "curl -sS -o /dev/null --max-time 10 https://ungranted.example/\n";

const PROBE_CHECK: &str =
    "[[checks]]\nname = \"probe\"\ncommand = \"sh probe.sh\"\nenvironment = \"sandbox\"\n";

const CONFINED_CHECK: &str =
    "[[checks]]\nname = \"confined\"\ncommand = \"sh confined.sh\"\nenvironment = \"sandbox\"\n";

const ASKING_CHECK: &str =
    "[[checks]]\nname = \"asking\"\ncommand = \"sh asking.sh\"\nenvironment = \"sandbox\"\n";

/// The owner's home with a key and a cargo folder in it, and project `app` the owner trusts,
/// written before the daemon reads the configuration.
fn prepared(home: &Home, owner_settings: &str) -> PathBuf {
    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(home.path().join(".ssh/id_ed25519"), "owner's key\n").unwrap();
    std::fs::create_dir_all(home.path().join(".cargo")).unwrap();
    let root = home.path().join("projects").join("app");
    home.write_owner_config(&format!(
        "{owner_settings}\n[projects]\ntrusted = [{:?}]\n",
        root.display().to_string()
    ));
    root
}

/// Commits `declaration` as the project's checks into `root`, with the scripts they run.
fn declare(home: &Home, root: &Path, declaration: &str) {
    let utf8 = |path: PathBuf| path.to_str().expect("a UTF-8 path").to_owned();
    let filled = |script: &str| {
        script
            .replace("@HOME@", &utf8(home.path()))
            .replace("@STATE@", &utf8(home.state_dir()))
            .replace("@SOCKET@", &utf8(home.socket_path()))
            .replace("@MARKER@", &utf8(marker(home)))
    };
    std::fs::create_dir_all(root.join(".torlek")).unwrap();
    std::fs::write(root.join(".torlek/config.toml"), declaration).unwrap();
    std::fs::write(root.join("probe.sh"), filled(PROBE)).unwrap();
    std::fs::write(root.join("confined.sh"), filled(CONFINED)).unwrap();
    std::fs::write(root.join("asking.sh"), ASKING).unwrap();
    std::fs::write(root.join(".gitignore"), "ran\n").unwrap();
    for arguments in [
        &["add", "--all"][..],
        &["commit", "--quiet", "-m", "checks"][..],
    ] {
        let status = Process::new("git")
            .current_dir(root)
            .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?}");
    }
}

/// The file the probe touches last, outside the checkout: present only if it ran to the end
/// with the owner's home writable, which it is on the host and never in the sandbox.
fn marker(home: &Home) -> PathBuf {
    home.path().join("ran")
}

/// A started daemon under `owner_settings` with project `app` declaring `declaration`.
async fn started(
    owner_settings: &str,
    declaration: &str,
    start: impl FnOnce(&Home) -> real_daemon::Daemon,
) -> (Home, real_daemon::Daemon) {
    let home = Home::new();
    let root = prepared(&home, owner_settings);
    let daemon = start(&home);
    let client = home.connect().await;
    let (registered, _) = home.project(&client, "app").await;
    assert_eq!(registered, root);
    declare(&home, &root, declaration);
    (home, daemon)
}

/// What the supervisor recorded of `run`'s execution, read once the daemon stopped.
async fn execution_of(home: &Home, run: &str) -> cockpit_processes::Execution {
    let execution = run
        .rsplit('-')
        .next()
        .and_then(|number| number.parse().ok())
        .expect("a run names its execution");
    let database = cockpit_turso::Database::open(
        &home.state_dir().join("state.db"),
        &cockpit_daemon::MIGRATION_SETS,
    )
    .await
    .unwrap();
    cockpit_processes::execution(&database, cockpit_processes::ExecutionId(execution))
        .await
        .unwrap()
}

fn printed(receipt: &CheckReceiptView) -> Vec<&str> {
    receipt.output.lines().collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_check_sees_nothing_of_the_owners_host_it_was_not_given() {
    let (home, _daemon) = started(REQUIRED, PROBE_CHECK, |home| {
        home.start_with(&[("GH_TOKEN", DAEMON_TOKEN)])
    })
    .await;

    let receipt = home.run_check("app", "probe").await;

    let lines = printed(&receipt);
    for expected in [
        "ssh_key=denied",
        "state_db=denied",
        "socket=hidden",
        "gh_token=unset",
        "cargo=refused",
        "tmp=writable",
        "tunnel=403",
    ] {
        assert!(lines.contains(&expected), "{expected} in {receipt:?}");
    }
    assert!(!home.path().join(".cargo/written").exists());
    assert!(!marker(&home).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_checks_receipt_names_the_sandbox_that_ran_it() {
    let (home, _daemon) = started(REQUIRED, PROBE_CHECK, Home::start).await;

    let receipt = home.run_check("app", "probe").await;

    assert_eq!(
        receipt.ran_environment.as_deref(),
        Some("sandbox"),
        "{receipt:?}"
    );
    let isolation = receipt
        .ran_isolation
        .expect("a sandboxed run records its isolation");
    assert!(isolation.contains("bubblewrap"), "{isolation}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_run_leaves_no_folder_behind() {
    let (home, _daemon) = started(REQUIRED, PROBE_CHECK, Home::start).await;

    let receipt = home.run_check("app", "probe").await;

    assert_eq!(
        receipt.ran_environment.as_deref(),
        Some("sandbox"),
        "{receipt:?}"
    );
    // Removed once the receipt is sealed: the run's end, not its receipt, removes it.
    let leftover = home
        .state_dir()
        .join("tmp")
        .join(format!("check-{}", receipt.run));
    let deadline = Instant::now() + GENEROUS;
    while leftover.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!leftover.exists(), "{} remains", leftover.display());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_on_a_host_without_the_sandbox_is_recorded_unavailable_and_never_runs() {
    let (home, daemon) = started(REQUIRED, PROBE_CHECK, Home::start_without_bubblewrap).await;

    let receipt = home.run_check("app", "probe").await;

    assert_eq!(
        receipt.outcome,
        CheckOutcomeView::Unavailable,
        "{receipt:?}"
    );
    assert_eq!(receipt.ran_environment, None);
    assert!(receipt.output.contains("sandbox"), "{receipt:?}");
    assert!(!marker(&home).exists());
    drop(daemon);
    let execution = execution_of(&home, &receipt.run).await;
    assert_eq!(
        execution.phase,
        cockpit_processes::Phase::Ended(cockpit_processes::EndReason::NeverStarted)
    );
    assert_eq!(execution.process_group, None, "{execution:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_runs_on_the_host_when_the_owner_turns_the_sandbox_off() {
    let (home, _daemon) = started("", PROBE_CHECK, Home::start).await;

    let receipt = home.run_check("app", "probe").await;

    assert_eq!(
        receipt.ran_environment.as_deref(),
        Some("native"),
        "{receipt:?}"
    );
    assert!(printed(&receipt).contains(&"ssh_key=read"), "{receipt:?}");
    assert!(marker(&home).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_that_passes_in_the_sandbox_passes_the_gate() {
    let (home, _daemon) = started(REQUIRED, CONFINED_CHECK, Home::start).await;

    let receipt = home.run_check("app", "confined").await;

    assert_eq!(receipt.outcome, CheckOutcomeView::Passed, "{receipt:?}");
    assert_eq!(
        receipt.ran_environment.as_deref(),
        Some("sandbox"),
        "{receipt:?}"
    );
    let gate = match home
        .connect()
        .await
        .request(
            Command::CheckGate {
                launch_token: None,
                project: Some("app".to_owned()),
                checkout: None,
            },
            GENEROUS,
        )
        .await
    {
        Ok(Reply::CheckGate { gate }) => *gate,
        other => panic!("expected a gate, got {other:?}"),
    };
    assert_eq!(gate.outcome, GateOutcomeView::Passed, "{gate:?}");
    assert_eq!(gate.checks[0].receipt, Some(receipt.id), "{gate:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_a_sandboxed_check_was_refused_asks_the_owner_for_the_check_run() {
    let (home, _daemon) = started(REQUIRED, ASKING_CHECK, Home::start).await;

    let receipt = home.run_check("app", "asking").await;

    assert_eq!(receipt.outcome, CheckOutcomeView::Failed, "{receipt:?}");
    let requests = network_requests(&home).await;
    let [request] = requests.as_slice() else {
        panic!("expected one request, got {requests:?}; {}", home.stderr());
    };
    assert_eq!(
        request.subject,
        NetworkSubjectView::Check {
            run: receipt.run.clone(),
            check: "asking".to_owned(),
        }
    );
    assert_eq!(request.project, "app");
    assert_eq!(request.target, "ungranted.example:443");
}

/// #352: the run ends before the owner can decide, so its output says what to wait for.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_check_run_names_the_request_the_owner_decides() {
    let (home, _daemon) = started(REQUIRED, ASKING_CHECK, Home::start).await;

    let receipt = home.run_check("app", "asking").await;

    let [request] = network_requests(&home).await.try_into().unwrap();
    let named = format!(
        "torlek: the sandbox refused ungranted.example:443: network request {} waits for the \
         owner to allow it always or deny it",
        request.id
    );
    assert!(receipt.output.contains(&named), "{}", receipt.output);
}

/// #352: one card per check and host; a rerun while it asks joins it.
#[tokio::test(flavor = "multi_thread")]
async fn a_rerun_of_a_refused_check_adds_no_second_request() {
    let (home, _daemon) = started(REQUIRED, ASKING_CHECK, Home::start).await;
    home.run_check("app", "asking").await;

    let rerun = home.run_check("app", "asking").await;

    let requests = network_requests(&home).await;
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        rerun
            .output
            .contains(&format!("network request {} waits", requests[0].id)),
        "{}",
        rerun.output
    );
}

/// #352: a denial holds for the check's next runs, which name it.
#[tokio::test(flavor = "multi_thread")]
async fn a_rerun_after_the_owner_denied_the_host_names_the_denial() {
    let (home, _daemon) = started(REQUIRED, ASKING_CHECK, Home::start).await;
    home.run_check("app", "asking").await;
    let [request] = network_requests(&home).await.try_into().unwrap();
    let denied = home
        .connect()
        .await
        .request(
            Command::NetworkDeny {
                request: request.id,
                through: None,
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(denied, Ok(Reply::NetworkRequest { .. })),
        "{denied:?}"
    );

    let rerun = home.run_check("app", "asking").await;

    assert_eq!(rerun.outcome, CheckOutcomeView::Failed, "{rerun:?}");
    let named = format!(
        "torlek: the sandbox refused ungranted.example:443: the owner denied it (network \
         request {})",
        request.id
    );
    assert!(rerun.output.contains(&named), "{}", rerun.output);
    assert_eq!(network_requests(&home).await, []);
}

/// What `NetworkList` shows the owner.
async fn network_requests(home: &Home) -> Vec<cockpit_protocol::NetworkRequestView> {
    match home
        .connect()
        .await
        .request(Command::NetworkList, GENEROUS)
        .await
    {
        Ok(Reply::NetworkRequests { requests }) => requests,
        other => panic!("expected the network requests, got {other:?}"),
    }
}
