//! #406's first part through the CLI and real daemon, in a fixture home only.
mod common;
mod scripted;

use cockpit_protocol::{CloneStorageView, Command, Outcome, Reply, TargetSizeView};
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
