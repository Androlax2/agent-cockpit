//! Target pruning through the real owner CLI/socket, exclusively in fixture homes.
mod common;
use common::TestHome;

#[test]
fn storage_prune_non_candidate_does_not_claim_its_unmeasured_target_is_absent() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let listed = home.cockpit_ok(&["checkout", "list"]);
    let id = listed.split('\t').next().unwrap();
    std::fs::create_dir(root.join("target")).unwrap();
    std::fs::write(root.join("target/keep"), b"user output").unwrap();
    let result = home.cockpit(&["storage", "prune", id, "--dry-run"]);
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(!output.contains("target is absent"), "{output}");
    assert!(output.contains("size unavailable"), "{output}");
    assert!(
        !result.status.success(),
        "unmeasured size returned success: {output}"
    );
    assert_eq!(
        std::fs::read(root.join("target/keep")).unwrap(),
        b"user output"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn storage_prune_incomplete_apply_fails_but_its_status_remains_inspectable() {
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
    let created = home.cockpit_ok(&["checkout", "clone", "app", "feature"]);
    let id = created.split('\t').next().unwrap();
    let clone = home.home().join("projects/.parallel/app/feature");
    std::fs::create_dir(clone.join("target")).unwrap();
    std::fs::write(clone.join(".git/info/exclude"), "/target/\n").unwrap();
    std::fs::write(clone.join("target/output"), b"keep").unwrap();
    let prepared = home.cockpit_ok(&["storage", "prune", id]);
    let command = prepared
        .lines()
        .find(|line| line.starts_with("torlek storage prune --plan "))
        .unwrap();
    let args: Vec<_> = command.split_whitespace().skip(1).collect();
    let collision = clone.join(format!(".torlek-target-prune-{}-{}", args[7], args[3]));
    std::fs::create_dir(&collision).unwrap();
    std::fs::write(collision.join("keep"), b"not our quarantine").unwrap();
    let result = home.cockpit(&args);
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("needs-owner"), "{output}");
    assert!(
        !result.status.success(),
        "incomplete Apply returned success: {output}"
    );
    let status = home.cockpit_ok(&["storage", "prune", "status", args[3]]);
    assert!(status.contains("needs-owner"), "{status}");
    assert_eq!(std::fs::read(clone.join("target/output")).unwrap(), b"keep");
    assert_eq!(
        std::fs::read(collision.join("keep")).unwrap(),
        b"not our quarantine"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn storage_prune_confirm_carries_displayed_epoch_and_status_names_kept_recovery_paths() {
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
    let created = home.cockpit_ok(&["checkout", "clone", "app", "feature"]);
    let id = created.split('\t').next().unwrap();
    let clone = home.home().join("projects/.parallel/app/feature");
    std::fs::create_dir_all(clone.join("target/debug")).unwrap();
    std::fs::write(clone.join(".git/info/exclude"), "/target/\n").unwrap();
    std::fs::write(clone.join("target/debug/output"), b"12345678").unwrap();
    let output = home.cockpit_ok(&["storage", "prune", id]);
    assert!(output.contains("8 apparent bytes"), "{output}");
    assert!(output.contains("LOCAL ancestry"), "{output}");
    let command = output
        .lines()
        .find(|line| line.starts_with("torlek storage prune --plan "))
        .unwrap();
    let args: Vec<_> = command.split_whitespace().skip(1).collect();
    assert_eq!(args[6], "--epoch");
    let mut stale = args.clone();
    stale[7] = "old-database-epoch";
    let refused = home.cockpit(&stale);
    assert!(!refused.status.success());
    assert_eq!(
        std::fs::read(clone.join("target/debug/output")).unwrap(),
        b"12345678"
    );
    let result = home.cockpit_ok(&args);
    assert!(
        result.contains("complete; 8 removed apparent bytes, 0 absent bytes"),
        "{result}"
    );
    assert!(!clone.join("target").exists());
    assert!(clone.join("README").is_file());
    let status = home.cockpit_ok(&["storage", "prune", "status", args[3]]);
    assert!(
        status.contains(&format!("target: {}", clone.join("target").display())),
        "{status}"
    );
    assert!(status.contains("quarantine:"), "{status}");
    assert!(status.contains("epoch:"), "{status}");
}

#[test]
fn storage_prune_dry_run_reports_size_and_keeps_fixture_files() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let created = home.cockpit_ok(&["checkout", "clone", "app", "feature"]);
    let id = created.split('\t').next().unwrap();
    let clone = home.home().join("projects/.parallel/app/feature");
    std::fs::create_dir_all(clone.join("target/debug")).unwrap();
    std::fs::write(clone.join(".git/info/exclude"), "/target/\n").unwrap();
    std::fs::write(clone.join("target/a"), b"abc").unwrap();
    std::fs::write(clone.join("target/debug/b"), b"12345").unwrap();
    let result = home.cockpit(&["storage", "prune", id, "--dry-run"]);
    let output = String::from_utf8(result.stdout).unwrap();
    #[cfg(target_os = "linux")]
    {
        assert!(result.status.success(), "{output}");
        assert!(output.contains("8 apparent bytes"), "{output}");
    }
    #[cfg(not(target_os = "linux"))]
    {
        assert!(!result.status.success(), "{output}");
        assert!(output.contains("mount-boundary support"), "{output}");
    }
    assert!(output.contains("dry-run"), "{output}");
    assert_eq!(std::fs::read(clone.join("target/a")).unwrap(), b"abc");
    assert_eq!(
        std::fs::read(clone.join("target/debug/b")).unwrap(),
        b"12345"
    );
    assert!(clone.join(".git").is_dir());
    assert!(clone.join("README").is_file());
}
