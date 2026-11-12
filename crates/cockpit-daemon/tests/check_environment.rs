//! #352 through the real daemon binary: under `agents.sandbox = "required"` a declared check runs
//! through `torlek-sandbox` (#862, Q1). From inside it the check cannot read the owner's keys or
//! the daemon's state, cannot reach the owner's socket, has none of the daemon's other variables
//! (#862, Q2), writes nothing into the owner's caches and is refused a host its project was
//! never granted; its receipt records the sandbox that ran it. On a host without the sandbox
//! the check is recorded `unavailable` and never runs; under `"off"` it runs on the host. A
//! check that passes in the sandbox passes the gate (#352's acceptance).

mod candidate;
mod real_daemon;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::time::{Duration, Instant};

use cockpit_protocol::{CheckOutcomeView, CheckReceiptView, Command, GateOutcomeView, Reply};
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

const PROBE_CHECK: &str =
    "[[checks]]\nname = \"probe\"\ncommand = \"sh probe.sh\"\nenvironment = \"sandbox\"\n";

const CONFINED_CHECK: &str =
    "[[checks]]\nname = \"confined\"\ncommand = \"sh confined.sh\"\nenvironment = \"sandbox\"\n";

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

/// Runs `check` as the owner and waits for its receipt.
async fn run_check(home: &Home, check: &str) -> CheckReceiptView {
    let client = home.connect().await;
    let run = match client
        .request(
            Command::CheckRun {
                project: "app".to_owned(),
                check: check.to_owned(),
                checkout: None,
            },
            GENEROUS,
        )
        .await
    {
        Ok(Reply::CheckRunStarted { run }) => run,
        other => panic!("expected a started run, got {other:?}"),
    };
    let deadline = Instant::now() + GENEROUS * 3;
    loop {
        let reply = client
            .request(
                Command::CheckReceipt {
                    project: "app".to_owned(),
                    run: run.clone(),
                },
                GENEROUS,
            )
            .await;
        match reply {
            Ok(Reply::CheckReceipt {
                receipt: Some(receipt),
            }) => return *receipt,
            Ok(Reply::CheckReceipt { receipt: None }) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => panic!(
                "expected the receipt of {run}, got {other:?}; {}",
                home.stderr()
            ),
        }
    }
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

fn printed(receipt: &CheckReceiptView) -> Vec<&str> {
    receipt.output.lines().collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sandboxed_check_sees_nothing_of_the_owners_host_it_was_not_given() {
    let (home, _daemon) = started(REQUIRED, PROBE_CHECK, |home| {
        home.start_with(&[("GH_TOKEN", DAEMON_TOKEN)])
    })
    .await;

    let receipt = run_check(&home, "probe").await;

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

    let receipt = run_check(&home, "probe").await;

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

    let receipt = run_check(&home, "probe").await;

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
    let (home, _daemon) = started(REQUIRED, PROBE_CHECK, Home::start_without_bubblewrap).await;

    let receipt = run_check(&home, "probe").await;

    assert_eq!(
        receipt.outcome,
        CheckOutcomeView::Unavailable,
        "{receipt:?}"
    );
    assert_eq!(receipt.ran_environment, None);
    assert!(receipt.output.contains("sandbox"), "{receipt:?}");
    assert!(!marker(&home).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_runs_on_the_host_when_the_owner_turns_the_sandbox_off() {
    let (home, _daemon) = started("", PROBE_CHECK, Home::start).await;

    let receipt = run_check(&home, "probe").await;

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

    let receipt = run_check(&home, "confined").await;

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
