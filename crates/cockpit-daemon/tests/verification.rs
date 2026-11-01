//! Issue #313 through the daemon binary's sockets: a check the owner runs yields a receipt the
//! gate counts; an agent can neither run a check nor create a passing receipt, and its word
//! that a check passed stays a claim. This test process plays the agent; its working directory
//! says which registered project it works in, as a launched session's does.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cockpit_agent_channel::{LaunchToken, identity_of};
use cockpit_authorization::Permission;
use cockpit_client::ClientError;
use cockpit_protocol::{
    CheckOutcomeView, CheckReceiptView, CheckStatusView, Command, ErrorCode, GateOutcomeView,
    GateView, Reply, StaleReasonView,
};
use common::{GENEROUS, TestHome, refused};

/// The working directory is the process's: tests that move it take turns.
static WORKING_DIRECTORY: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const CHECKS: &str = "[[checks]]\nname = \"test\"\ncommand = \"echo testing; test -f ok\"\n\
                      artifacts = [\"ok\"]\n\n\
                      [[checks]]\nname = \"lint\"\ncommand = \"echo linted\"\n\n\
                      [[checks]]\nname = \"boxed\"\ncommand = \"true\"\nenvironment = \"sandbox\"\n";

/// Fields drop in order: the daemon stops before its home is deleted.
struct Verification {
    _daemon: common::RunningDaemon,
    home: TestHome,
    root: PathBuf,
    token: LaunchToken,
    _cwd: tokio::sync::MutexGuard<'static, ()>,
}

/// A started daemon whose owner registered and trusts project `app`, a committed repository
/// declaring [`CHECKS`], with session `s0123456789ab` launched from this process and working
/// in it.
async fn started() -> Verification {
    started_with(true).await
}

async fn started_with(is_trusted: bool) -> Verification {
    let cwd = WORKING_DIRECTORY.lock().await;
    let home = TestHome::new();
    let config = home.home().join("config").join(common::BUILD_PROFILE_DIR);
    std::fs::create_dir_all(&config).unwrap();
    let root = committed_repository(home.home(), "app");
    if is_trusted {
        std::fs::write(
            config.join("config.toml"),
            format!("[projects]\ntrusted = [{:?}]\n", root.display().to_string()),
        )
        .unwrap();
    }
    let database = home.open_state().await;
    cockpit_projects::register_project(&database, &cockpit_projects::Git::from_path(), &root, None)
        .await
        .unwrap();
    drop(database);
    let token = home
        .launch(
            "s0123456789ab",
            &identity_of(std::process::id()).unwrap(),
            Some(Permission::ChangeState),
        )
        .await;
    std::env::set_current_dir(&root).unwrap();
    Verification {
        _daemon: home.start(),
        home,
        root,
        token,
        _cwd: cwd,
    }
}

fn git(dir: &Path, arguments: &[&str]) {
    let status = std::process::Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(arguments)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {arguments:?}");
}

fn committed_repository(home: &Path, name: &str) -> PathBuf {
    let root = home.join("projects").join(name);
    std::fs::create_dir_all(root.join(".cockpit")).unwrap();
    std::fs::write(root.join(".cockpit/config.toml"), CHECKS).unwrap();
    std::fs::write(root.join("ok"), "present\n").unwrap();
    git(&root, &["init", "--quiet"]);
    git(&root, &["add", "--all"]);
    git(&root, &["commit", "--quiet", "-m", "initial"]);
    root.canonicalize().unwrap()
}

impl Verification {
    fn token(&self) -> Option<String> {
        Some(self.token.as_str().to_owned())
    }

    async fn as_agent(&self, command: Command) -> Result<Reply, ClientError> {
        self.home
            .connect_agent()
            .await
            .request(command, GENEROUS)
            .await
    }

    async fn as_owner(&self, command: Command) -> Result<Reply, ClientError> {
        self.home.connect().await.request(command, GENEROUS).await
    }

    fn owner_run(&self, check: &str) -> Command {
        Command::CheckRun {
            project: "app".to_owned(),
            check: check.to_owned(),
            checkout: None,
        }
    }

    /// Runs `check` as the owner and waits for its receipt.
    async fn run(&self, check: &str) -> CheckReceiptView {
        let run = match self.as_owner(self.owner_run(check)).await {
            Ok(Reply::CheckRunStarted { run }) => run,
            other => panic!("expected a started run, got {other:?}"),
        };
        let deadline = Instant::now() + GENEROUS;
        loop {
            let reply = self
                .as_owner(Command::CheckReceipt {
                    project: "app".to_owned(),
                    run: run.clone(),
                })
                .await;
            match reply {
                Ok(Reply::CheckReceipt {
                    receipt: Some(receipt),
                }) => return *receipt,
                Ok(Reply::CheckReceipt { receipt: None }) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                other => panic!("expected the receipt of {run}, got {other:?}"),
            }
        }
    }

    async fn owner_gate(&self) -> GateView {
        match self
            .as_owner(Command::CheckGate {
                launch_token: None,
                project: Some("app".to_owned()),
                checkout: None,
            })
            .await
        {
            Ok(Reply::CheckGate { gate }) => *gate,
            other => panic!("expected a gate, got {other:?}"),
        }
    }

    fn status<'gate>(gate: &'gate GateView, check: &str) -> &'gate CheckStatusView {
        &gate
            .checks
            .iter()
            .find(|entry| entry.name == check)
            .unwrap_or_else(|| panic!("{check} is in the gate: {gate:?}"))
            .status
    }
}

#[tokio::test]
async fn a_check_the_owner_runs_yields_a_receipt_the_gate_counts() {
    let verification = started().await;

    let receipt = verification.run("test").await;

    assert_eq!(receipt.outcome, CheckOutcomeView::Passed);
    assert_eq!(receipt.exit_code, Some(0));
    assert_eq!(receipt.output, "testing\n");
    assert_eq!(receipt.dirty, None);
    assert_eq!(receipt.artifacts.len(), 1);
    let gate = verification.owner_gate().await;
    assert_eq!(
        Verification::status(&gate, "test"),
        &CheckStatusView::Checked {
            outcome: CheckOutcomeView::Passed
        }
    );
    assert_eq!(gate.checks[0].receipt, Some(receipt.id));
}

#[tokio::test]
async fn a_check_that_exits_non_zero_is_recorded_failed_with_its_output() {
    let verification = started().await;
    std::fs::remove_file(verification.root.join("ok")).unwrap();
    std::fs::write(
        verification.root.join(".cockpit/config.toml"),
        CHECKS.replace("artifacts = [\"ok\"]\n", ""),
    )
    .unwrap();

    let receipt = verification.run("test").await;

    assert_eq!(receipt.outcome, CheckOutcomeView::Failed);
    assert_eq!(receipt.exit_code, Some(1));
    assert_eq!(receipt.output, "testing\n");
    assert!(receipt.dirty.is_some(), "{receipt:?}");
}

#[tokio::test]
async fn a_change_after_the_run_makes_its_receipt_stale() {
    let verification = started().await;
    verification.run("test").await;

    std::fs::write(verification.root.join("ok"), "changed\n").unwrap();

    let gate = verification.owner_gate().await;
    assert!(
        matches!(
            Verification::status(&gate, "test"),
            CheckStatusView::Stale {
                reason: StaleReasonView::SourceChanged { .. }
            }
        ),
        "{gate:?}"
    );
    assert_eq!(gate.outcome, GateOutcomeView::Open);
}

#[tokio::test]
async fn an_agent_cannot_run_a_check_or_read_a_receipt() {
    let verification = started().await;

    let run = refused(verification.as_agent(verification.owner_run("test")).await);
    let receipt = refused(
        verification
            .as_agent(Command::CheckReceipt {
                project: "app".to_owned(),
                run: "execution-1".to_owned(),
            })
            .await,
    );

    assert_eq!(run.code, ErrorCode::NotAuthorized);
    assert_eq!(receipt.code, ErrorCode::NotAuthorized);
}

#[tokio::test]
async fn an_agents_claim_is_recorded_and_never_counts_as_a_pass() {
    let verification = started().await;

    let claimed = verification
        .as_agent(Command::CheckClaim {
            launch_token: verification.token(),
            check: "test".to_owned(),
            statement: "cargo test passed for me".to_owned(),
        })
        .await;

    match claimed {
        Ok(Reply::CheckClaimed { claim }) => {
            assert_eq!(claim.claimant, "s0123456789ab");
            assert_eq!(claim.statement, "cargo test passed for me");
        }
        other => panic!("expected a claim, got {other:?}"),
    }
    let gate = match verification
        .as_agent(Command::CheckGate {
            launch_token: verification.token(),
            project: None,
            checkout: None,
        })
        .await
    {
        Ok(Reply::CheckGate { gate }) => *gate,
        other => panic!("expected the agent's gate, got {other:?}"),
    };
    assert_eq!(gate.project, "app");
    assert_eq!(
        Verification::status(&gate, "test"),
        &CheckStatusView::Unverified
    );
    assert_eq!(gate.outcome, GateOutcomeView::Open);
}

#[tokio::test]
async fn the_owner_runs_a_check_rather_than_claiming_it() {
    let verification = started().await;

    let error = refused(
        verification
            .as_owner(Command::CheckClaim {
                launch_token: None,
                check: "test".to_owned(),
                statement: "trust me".to_owned(),
            })
            .await,
    );

    assert_eq!(error.code, ErrorCode::NotAuthorized);
}

#[tokio::test]
async fn an_agent_names_no_other_checkout_for_its_gate() {
    let verification = started().await;

    let error = refused(
        verification
            .as_agent(Command::CheckGate {
                launch_token: verification.token(),
                project: Some("other".to_owned()),
                checkout: None,
            })
            .await,
    );

    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[tokio::test]
async fn a_check_the_project_does_not_declare_is_not_found() {
    let verification = started().await;

    let error = refused(
        verification
            .as_owner(verification.owner_run("deploy"))
            .await,
    );

    assert_eq!(error.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn a_check_in_an_environment_the_daemon_cannot_provide_is_refused_before_it_runs() {
    let verification = started().await;

    let error = refused(verification.as_owner(verification.owner_run("boxed")).await);

    assert_eq!(error.code, ErrorCode::Refused);
    assert!(error.message.contains("sandbox"), "{error:?}");
}

#[tokio::test]
async fn a_check_in_a_checkout_the_owner_does_not_trust_is_refused_before_it_runs() {
    let verification = started_with(false).await;

    let error = refused(verification.as_owner(verification.owner_run("test")).await);

    assert_eq!(error.code, ErrorCode::NotAuthorized);
    assert!(error.message.contains("not trusted"), "{error:?}");
}
