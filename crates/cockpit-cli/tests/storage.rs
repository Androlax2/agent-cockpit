//! #406's first part through the CLI and real daemon, in a fixture home only.
mod common;
mod scripted;

use cockpit_protocol::{
    CloneStorageView, Command, Outcome, PruneVerdictView, Reply, TargetSizeView,
};
use common::TestHome;
use std::os::unix::fs::DirBuilderExt;

#[test]
fn storage_measures_only_registered_clone_targets_without_changing_them() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let created = home.cockpit_ok(&["checkout", "clone", "app", "feature"]);
    let id = created.split('\t').next().unwrap();
    home.cockpit_ok(&["checkout", "claim", id, "live-session"]);
    let clone = home.home().join("projects/.parallel/app/feature");
    std::fs::create_dir_all(clone.join("target/debug")).unwrap();
    std::fs::write(clone.join("target/a"), "abc").unwrap();
    std::fs::write(clone.join("target/debug/b"), "12345").unwrap();
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("target/not-a-clone"), [0; 100]).unwrap();
    let unknown = home
        .home()
        .join("projects/.parallel/app/unregistered/target");
    std::fs::create_dir_all(&unknown).unwrap();
    std::fs::write(unknown.join("not-registered"), [0; 100]).unwrap();

    let output = home.cockpit_ok(&["storage"]);

    assert!(output.contains("apparent bytes"), "{output}");
    assert!(
        output.contains(&format!("{id}\tapp\t1\t8\tcomplete\tlive-session\t")),
        "{output}"
    );
    assert!(!output.contains("unregistered"), "{output}");
    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with(&format!("{id}\t")))
            .count(),
        1
    );
    assert_eq!(
        std::fs::read(clone.join("target/debug/b")).unwrap(),
        b"12345"
    );
    assert!(
        home.cockpit_ok(&["checkout", "list"])
            .contains("live-session")
    );
}

#[test]
fn storage_does_not_accept_a_prune_argument() {
    let home = TestHome::new();
    let output = home.cockpit(&["storage", "prune"]);
    assert_eq!(output.status.code(), Some(2));
}

#[tokio::test]
async fn storage_preserves_measurement_status_and_exits_unsuccessfully_on_partial_reports() {
    for (measurement, truncated, status, row) in [
        (
            TargetSizeView::Complete { apparent_bytes: 0 },
            false,
            0,
            "\t0\tcomplete\t",
        ),
        (TargetSizeView::Missing, false, 0, "\t0\tmissing\t"),
        (
            TargetSizeView::Incomplete {
                apparent_bytes: 8,
                reason: "entry limit".to_owned(),
            },
            false,
            1,
            "\t8\tincomplete\t",
        ),
        (
            TargetSizeView::Unavailable {
                reason: "permission denied".to_owned(),
            },
            false,
            1,
            "\t-\tunavailable\t",
        ),
        (
            TargetSizeView::Complete { apparent_bytes: 8 },
            true,
            1,
            "\t8\tcomplete\t",
        ),
    ] {
        let home = TestHome::new();
        let socket = home.home().join("run/torlek-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(socket.parent().unwrap())
            .unwrap();
        let daemon = scripted::scripted_daemon(
            &socket,
            Outcome::Ok(Reply::Storage {
                clones: vec![CloneStorageView {
                    checkout: 2,
                    project: "app".to_owned(),
                    incarnation: 1,
                    target: "/registered/target".to_owned(),
                    holder: None,
                    measurement,
                    prune: Some(PruneVerdictView::Kept {
                        reason: "fixture".to_owned(),
                    }),
                }],
                truncated,
            }),
        );
        let output = tokio::task::spawn_blocking(move || home.cockpit(&["storage"]))
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(status),
            "{}",
            scripted::text(&output.stderr)
        );
        let stdout = scripted::text(&output.stdout);
        assert!(stdout.contains(row), "{stdout}");
        assert_eq!(
            stdout.contains("further registered clones omitted"),
            truncated,
            "{stdout}"
        );
        assert_eq!(daemon.await.unwrap(), Command::StorageReport);
    }
}

fn scripted_row(checkout: i64, apparent_bytes: u64, prune: PruneVerdictView) -> CloneStorageView {
    CloneStorageView {
        checkout,
        project: "app".to_owned(),
        incarnation: 1,
        target: format!("/registered/{checkout}/target"),
        holder: None,
        measurement: TargetSizeView::Complete { apparent_bytes },
        prune: Some(prune),
    }
}

#[tokio::test]
async fn storage_shows_each_clone_prune_verdict_and_the_eligible_total() {
    let home = TestHome::new();
    let socket = home.home().join("run/torlek-dev/daemon.sock");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(socket.parent().unwrap())
        .unwrap();
    let daemon = scripted::scripted_daemon(
        &socket,
        Outcome::Ok(Reply::Storage {
            clones: vec![
                scripted_row(
                    2,
                    8,
                    PruneVerdictView::Eligible {
                        confirmation_required: true,
                    },
                ),
                scripted_row(
                    3,
                    16,
                    PruneVerdictView::Eligible {
                        confirmation_required: false,
                    },
                ),
                scripted_row(
                    4,
                    1000,
                    PruneVerdictView::Kept {
                        reason: "the clone has unpushed commits".to_owned(),
                    },
                ),
            ],
            truncated: false,
        }),
    );

    let output = tokio::task::spawn_blocking(move || home.cockpit(&["storage"]))
        .await
        .unwrap();

    let stdout = scripted::text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("\tholder\tprune\ttarget\treason"),
        "{stdout}"
    );
    assert!(
        stdout.contains("2\tapp\t1\t8\tcomplete\t-\teligible\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("3\tapp\t1\t16\tcomplete\t-\teligible-auto\t"),
        "{stdout}"
    );
    assert!(
        stdout.contains("4\tapp\t1\t1000\tcomplete\t-\tkept\t/registered/4/target\tthe clone has unpushed commits"),
        "{stdout}"
    );
    assert!(
        stdout.contains("eligible for prune: 2 of 3 clones, 24 apparent bytes"),
        "{stdout}"
    );
    assert_eq!(daemon.await.unwrap(), Command::StorageReport);
}

#[test]
#[cfg(target_os = "linux")]
fn storage_keeps_a_clone_with_unpushed_commits_and_counts_only_eligible_clones() {
    let home = TestHome::new();
    std::fs::create_dir_all(home.home().join("config/torlek-dev")).unwrap();
    std::fs::write(
        home.home().join("config/torlek-dev/config.toml"),
        "[storage]\ntarget_prune_base_ref = 'refs/heads/main'\n",
    )
    .unwrap();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let mut ids = Vec::new();
    for branch in ["merged", "local-work"] {
        let created = home.cockpit_ok(&["checkout", "clone", "app", branch]);
        ids.push(created.split('\t').next().unwrap().to_owned());
        let clone = home.home().join("projects/.parallel/app").join(branch);
        std::fs::create_dir(clone.join("target")).unwrap();
        std::fs::write(clone.join(".git/info/exclude"), "/target/\n").unwrap();
        std::fs::write(clone.join("target/output"), b"12345678").unwrap();
    }
    common::real_git(
        &home.home().join("projects/.parallel/app/local-work"),
        &["commit", "--quiet", "--allow-empty", "-m", "on no remote"],
    );

    let output = home.cockpit_ok(&["storage"]);

    let row = |id: &str| {
        output
            .lines()
            .find(|line| line.starts_with(&format!("{id}\t")))
            .unwrap_or_else(|| panic!("no row for {id}: {output}"))
            .to_owned()
    };
    assert!(row(&ids[0]).contains("\teligible\t"), "{output}");
    assert!(row(&ids[1]).contains("\tkept\t"), "{output}");
    assert!(
        row(&ids[1]).contains("\tthe clone has unpushed commits"),
        "{output}"
    );
    assert!(
        output.contains("eligible for prune: 1 of 2 clones, 8 apparent bytes"),
        "{output}"
    );
    assert_eq!(
        std::fs::read(
            home.home()
                .join("projects/.parallel/app/local-work/target/output")
        )
        .unwrap(),
        b"12345678"
    );
}

fn output_row(retired: u64, failures: &[&str]) -> cockpit_protocol::OutputRetentionView {
    cockpit_protocol::OutputRetentionView {
        project: "app".to_owned(),
        outputs: 3,
        apparent_bytes: 1800,
        unmeasured: 0,
        protected: 1,
        selected: 2,
        selected_bytes: 1200,
        retired,
        retired_bytes: retired * 600,
        failures: failures
            .iter()
            .map(|failure| (*failure).to_owned())
            .collect(),
    }
}

/// Runs `torlek ARGUMENTS` against a daemon that answers `reply`; the command it sent, its
/// exit code and stdout.
async fn against_scripted(
    arguments: &'static [&'static str],
    reply: Reply,
) -> (Command, Option<i32>, String) {
    let home = TestHome::new();
    let socket = home.home().join("run/torlek-dev/daemon.sock");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(socket.parent().unwrap())
        .unwrap();
    let daemon = scripted::scripted_daemon(&socket, Outcome::Ok(reply));
    let output = tokio::task::spawn_blocking(move || home.cockpit(arguments))
        .await
        .unwrap();
    (
        daemon.await.unwrap(),
        output.status.code(),
        scripted::text(&output.stdout),
    )
}

#[tokio::test]
async fn storage_outputs_reports_what_retention_selects_without_retiring_it() {
    let (sent, code, stdout) = against_scripted(
        &["storage", "outputs"],
        Reply::StorageOutputs {
            projects: vec![output_row(0, &[])],
            applied: false,
            truncated: false,
        },
    )
    .await;

    assert_eq!(sent, Command::StorageOutputs { apply: false });
    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains("app\t3\t1800\t0\t1\t2\t1200\t0\t0"),
        "{stdout}"
    );
    assert!(stdout.contains("torlek storage outputs prune"), "{stdout}");
}

#[tokio::test]
async fn storage_outputs_prune_retires_and_reports_the_bytes_removed() {
    let (sent, code, stdout) = against_scripted(
        &["storage", "outputs", "prune"],
        Reply::StorageOutputs {
            projects: vec![output_row(2, &[])],
            applied: true,
            truncated: false,
        },
    )
    .await;

    assert_eq!(sent, Command::StorageOutputs { apply: true });
    assert_eq!(code, Some(0), "{stdout}");
    assert!(
        stdout.contains("app\t3\t1800\t0\t1\t2\t1200\t2\t1200"),
        "{stdout}"
    );
    assert!(
        stdout.contains("retired 2 outputs, 1200 apparent bytes"),
        "{stdout}"
    );
}

#[tokio::test]
async fn storage_outputs_prune_fails_when_it_kept_a_selected_output() {
    let (_, code, stdout) = against_scripted(
        &["storage", "outputs", "prune"],
        Reply::StorageOutputs {
            projects: vec![output_row(1, &["execution 9: kept: being read"])],
            applied: true,
            truncated: false,
        },
    )
    .await;

    assert_eq!(code, Some(1), "{stdout}");
    assert!(
        stdout.contains("app: execution 9: kept: being read"),
        "{stdout}"
    );
}

#[tokio::test]
async fn storage_outputs_fails_when_the_pass_left_outputs_out() {
    let (_, code, stdout) = against_scripted(
        &["storage", "outputs"],
        Reply::StorageOutputs {
            projects: vec![output_row(0, &[])],
            applied: false,
            truncated: true,
        },
    )
    .await;

    assert_eq!(code, Some(1), "{stdout}");
    assert!(stdout.contains("incomplete"), "{stdout}");
}
