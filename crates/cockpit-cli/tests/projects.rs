//! Issue #28's acceptance: projects and checkouts through the `cockpit` command line, the real
//! daemon and a fake `git` that logs every host Git call.
mod common;

use common::{GITHUB_URL, TestHome, commit_all, real_git};

/// The checkout line `cockpit checkout list` prints for `id`, split on tabs.
fn checkout_line(home: &TestHome, id: &str) -> Vec<String> {
    home.cockpit_ok(&["checkout", "list"])
        .lines()
        .map(|line| line.split('\t').map(str::to_owned).collect::<Vec<_>>())
        .find(|fields| fields[0] == id)
        .unwrap_or_else(|| panic!("checkout {id} is not listed"))
}

/// The id field of the one checkout line in `output`.
fn id_of(output: &str) -> String {
    output.split('\t').next().unwrap().to_owned()
}

#[test]
fn an_added_project_is_listed_with_its_main_checkout_at_incarnation_one() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");

    let added = home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);

    assert_eq!(
        home.cockpit_ok(&["project", "list"]),
        format!("app\t{}\n", root.display())
    );
    assert_eq!(
        checkout_line(&home, &id_of(&added)),
        [
            "1",
            "app",
            "main",
            "1",
            "-",
            "untrusted",
            root.to_str().unwrap()
        ]
    );
}

#[test]
fn a_relative_project_path_is_resolved_by_the_cli_before_the_daemon_sees_it() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"))
        .current_dir(root.parent().unwrap())
        .env_clear()
        .env("HOME", home.home())
        .env("XDG_STATE_HOME", home.home().join("state"))
        .env("XDG_RUNTIME_DIR", home.home().join("run"))
        .args(["project", "add", "app"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        home.cockpit_ok(&["project", "list"]),
        format!("app\t{}\n", root.display())
    );
}

#[test]
fn every_host_git_call_runs_with_hooks_off_and_program_settings_reset() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]);

    let log = home.git_log();

    let safe_prefix: String = cockpit_settings()
        .iter()
        .map(|setting| format!("-c {setting} "))
        .collect();
    assert!(log.len() >= 4, "{log:?}");
    for call in &log {
        assert!(call.starts_with(&safe_prefix), "{call}");
    }
}

/// `cockpit-projects`' `SAFE_CONFIG`, restated so a change to it shows up here.
fn cockpit_settings() -> [&'static str; 17] {
    [
        "core.hooksPath=/dev/null",
        "core.fsmonitor=false",
        "core.sshCommand=ssh",
        "core.askPass=",
        "core.gitProxy=",
        "core.pager=cat",
        "diff.external=",
        "credential.helper=",
        "log.showSignature=false",
        "push.gpgSign=false",
        "gpg.program=gpg",
        "gpg.ssh.program=ssh-keygen",
        "gpg.x509.program=gpgsm",
        "protocol.ext.allow=never",
        "protocol.file.allow=never",
        "remote.origin.receivepack=git-receive-pack",
        "remote.origin.uploadpack=git-upload-pack",
    ]
}

#[test]
fn a_git_dir_in_the_daemons_environment_does_not_redirect_host_git() {
    let mut home = TestHome::new();
    home.daemon_env
        .push(("GIT_DIR".to_owned(), "/nonexistent".to_owned()));
    let _daemon = home.start();
    let root = home.project("app");

    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
}

#[test]
fn a_subdirectory_of_a_work_tree_is_refused_naming_the_top_level() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    std::fs::create_dir(root.join("src")).unwrap();

    let refused = home.cockpit_err(&["project", "add", root.join("src").to_str().unwrap()]);

    assert!(refused.contains("invalid-argument"), "{refused}");
    assert!(refused.contains(root.to_str().unwrap()), "{refused}");
}

#[test]
fn a_parallel_clone_is_a_dissociated_local_clone_on_its_branch_with_origin_on_github() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);

    let cloned = home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]);

    let clone = home.home().join("projects/.parallel/app/feature-x");
    assert_eq!(
        checkout_line(&home, &id_of(&cloned))[2..5],
        ["parallel:feature/x", "1", "-"]
    );
    assert_eq!(
        real_git(&clone, &["branch", "--show-current"]).trim(),
        "feature/x"
    );
    assert_eq!(
        real_git(&clone, &["remote", "get-url", "origin"]).trim(),
        GITHUB_URL
    );
    assert!(
        !clone.join(".git/objects/info/alternates").exists(),
        "the clone still borrows objects"
    );
    let clone_call = home
        .git_log()
        .into_iter()
        .find(|call| call.contains(" clone "))
        .unwrap();
    assert!(
        clone_call.contains(&format!(
            "clone --quiet --no-local --reference {root} --dissociate -- {root} {}",
            clone.display(),
            root = root.display()
        )),
        "{clone_call}"
    );
}

#[test]
fn a_parallel_clone_of_a_trusted_project_inherits_its_trust() {
    let home = TestHome::new();
    let root = home.project("app");
    home.trust(&root);
    let _daemon = home.start();
    let main = home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);

    let clone = home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]);

    assert_eq!(checkout_line(&home, &id_of(&main))[5], "trusted");
    assert_eq!(
        checkout_line(&home, &id_of(&clone))[5],
        format!("inherited:{}", root.display())
    );
}

#[test]
fn a_pull_request_number_beyond_storage_is_an_invalid_argument() {
    let home = TestHome::new();
    let root = home.project("app");
    let _daemon = home.start();
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);

    let refused = home.cockpit_err(&["checkout", "review", "app", &u64::MAX.to_string()]);

    assert!(refused.contains("invalid-argument"), "{refused}");
}

#[test]
fn a_review_clone_checks_out_the_pull_request_head_and_is_never_trusted() {
    let home = TestHome::new();
    let root = home.project("app");
    home.trust(&root);
    let contributor = home.home().join("contributor");
    real_git(
        &home.home(),
        &[
            "clone",
            "--quiet",
            home.github().to_str().unwrap(),
            contributor.to_str().unwrap(),
        ],
    );
    std::fs::write(contributor.join("CHANGE"), "proposed\n").unwrap();
    commit_all(&contributor, "proposed change");
    real_git(
        &contributor,
        &["push", "--quiet", "origin", "HEAD:refs/pull/7/head"],
    );
    let proposed = real_git(&contributor, &["rev-parse", "HEAD"]);
    let _daemon = home.start();
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);

    let review = home.cockpit_ok(&["checkout", "review", "app", "7"]);

    let clone = home.home().join("projects/.parallel/app/pr-7");
    assert_eq!(real_git(&clone, &["rev-parse", "HEAD"]), proposed);
    assert_eq!(
        checkout_line(&home, &id_of(&review))[2..6],
        ["review:#7", "1", "-", "never"]
    );
}

#[test]
fn a_parallel_clone_gets_cockpits_lefthook_runner_and_a_review_clone_none() {
    let home = TestHome::new();
    let root = home.project("app");
    std::fs::write(root.join("lefthook.yml"), "pre-commit:\n  commands: {}\n").unwrap();
    commit_all(&root, "lefthook");
    real_git(
        &root,
        &["push", "--quiet", home.github().to_str().unwrap(), "main"],
    );
    real_git(
        &root,
        &[
            "push",
            "--quiet",
            home.github().to_str().unwrap(),
            "main:refs/pull/3/head",
        ],
    );
    let _daemon = home.start();
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);

    home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]);
    home.cockpit_ok(&["checkout", "review", "app", "3"]);

    let clones = home.home().join("projects/.parallel/app");
    let runner = std::fs::read_to_string(clones.join("feature-x/.git/hooks/pre-commit")).unwrap();
    assert!(runner.contains("lefthook run pre-commit"), "{runner}");
    assert!(!clones.join("pr-3/.git/hooks/pre-commit").exists());
}

#[test]
fn a_failed_clone_leaves_no_directory_and_no_checkout() {
    let mut home = TestHome::new();
    home.failing_git_argument = Some("switch".to_owned());
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);

    home.cockpit_err(&["checkout", "clone", "app", "feature/x"]);

    assert!(
        !home
            .home()
            .join("projects/.parallel/app/feature-x")
            .exists()
    );
    assert_eq!(home.cockpit_ok(&["checkout", "list"]).lines().count(), 1);
}

#[test]
fn a_checkout_claimed_by_one_holder_is_refused_to_another_naming_the_occupant() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    let main = id_of(&home.cockpit_ok(&["project", "add", root.to_str().unwrap()]));
    home.cockpit_ok(&["checkout", "claim", &main, "session-a"]);

    let refused = home.cockpit_err(&["checkout", "claim", &main, "session-b"]);

    assert!(refused.contains("conflict"), "{refused}");
    assert!(refused.contains("session-a"), "{refused}");
}

#[test]
fn checkout_occupancy_survives_a_daemon_crash() {
    let home = TestHome::new();
    let root = home.project("app");
    let main = {
        let daemon = home.start();
        let main = id_of(&home.cockpit_ok(&["project", "add", root.to_str().unwrap()]));
        home.cockpit_ok(&["checkout", "claim", &main, "session-a"]);
        daemon.kill();
        main
    };

    let _restarted = home.start();

    assert_eq!(checkout_line(&home, &main)[4], "session-a");
    let refused = home.cockpit_err(&["checkout", "claim", &main, "session-b"]);
    assert!(refused.contains("session-a"), "{refused}");
}

#[test]
fn a_released_checkout_can_be_claimed_by_another_holder() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    let main = id_of(&home.cockpit_ok(&["project", "add", root.to_str().unwrap()]));
    home.cockpit_ok(&["checkout", "claim", &main, "session-a"]);
    home.cockpit_ok(&["checkout", "release", &main, "session-a"]);

    home.cockpit_ok(&["checkout", "claim", &main, "session-b"]);

    assert_eq!(checkout_line(&home, &main)[4], "session-b");
}

#[test]
fn the_sweep_removes_a_clean_unoccupied_clone_and_records_it_removed() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));

    let swept = home.cockpit_ok(&["checkout", "sweep"]);

    let path = home.home().join("projects/.parallel/app/feature-x");
    assert_eq!(swept, format!("removed\t{clone}\t{}\n", path.display()));
    assert!(!path.exists());
    assert!(!home.cockpit_ok(&["checkout", "list"]).contains("feature-x"));
}

#[test]
fn a_dry_run_sweep_deletes_nothing() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));

    let swept = home.cockpit_ok(&["checkout", "sweep", "--dry-run"]);

    assert!(
        swept.starts_with(&format!("would remove\t{clone}\t")),
        "{swept}"
    );
    assert!(
        home.home()
            .join("projects/.parallel/app/feature-x")
            .exists()
    );
}

#[test]
fn the_sweep_keeps_a_clone_that_holds_work_or_a_holder_naming_the_reason() {
    for (case, expected_reason) in [
        ("occupied", "occupied by session-a"),
        ("uncommitted", "uncommitted changes"),
        ("unpushed", "unpushed commits"),
        ("committed on a detached head", "unpushed commits"),
        ("stashed", "uncommitted changes"),
    ] {
        let home = TestHome::new();
        let _daemon = home.start();
        let root = home.project("app");
        home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
        let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));
        let path = home.home().join("projects/.parallel/app/feature-x");
        match case {
            "occupied" => {
                home.cockpit_ok(&["checkout", "claim", &clone, "session-a"]);
            }
            "uncommitted" => std::fs::write(path.join("draft"), "work\n").unwrap(),
            "unpushed" => {
                std::fs::write(path.join("draft"), "work\n").unwrap();
                commit_all(&path, "local work");
            }
            "committed on a detached head" => {
                real_git(&path, &["switch", "--quiet", "--detach"]);
                std::fs::write(path.join("draft"), "work\n").unwrap();
                commit_all(&path, "local work");
            }
            _ => {
                std::fs::write(path.join("README"), "edited\n").unwrap();
                real_git(&path, &["stash", "--quiet"]);
            }
        }

        let swept = home.cockpit_ok(&["checkout", "sweep"]);

        assert_eq!(
            swept,
            format!("kept\t{clone}\t{}\t{expected_reason}\n", path.display()),
            "{case}"
        );
        assert!(path.exists(), "{case}");
    }
}

#[test]
fn a_push_sends_the_branch_to_its_github_origin_and_sets_the_upstream() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));
    let path = home.home().join("projects/.parallel/app/feature-x");
    std::fs::write(path.join("work"), "done\n").unwrap();
    commit_all(&path, "work");

    let pushed = home.cockpit_ok(&["checkout", "push", &clone]);

    assert_eq!(pushed, format!("pushed feature/x to {GITHUB_URL}\n"));
    assert_eq!(
        real_git(&home.github(), &["rev-parse", "refs/heads/feature/x"]),
        real_git(&path, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        real_git(&path, &["rev-parse", "--abbrev-ref", "@{upstream}"]).trim(),
        "origin/feature/x"
    );
}

#[test]
fn a_push_to_an_origin_outside_github_is_refused() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    real_git(
        &root,
        &["remote", "set-url", "origin", "https://example.com/app.git"],
    );
    let main = id_of(&home.cockpit_ok(&["project", "add", root.to_str().unwrap()]));

    let refused = home.cockpit_err(&["checkout", "push", &main]);

    assert!(refused.contains("refused"), "{refused}");
    assert!(refused.contains("https://example.com/app.git"), "{refused}");
    assert!(!home.git_log().iter().any(|call| call.contains(" push ")));
}

#[test]
fn a_push_redirected_by_the_checkouts_own_configuration_is_refused() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    real_git(
        &root,
        &[
            "config",
            "remote.origin.pushurl",
            "https://github.com/someone-else/app.git",
        ],
    );
    let main = id_of(&home.cockpit_ok(&["project", "add", root.to_str().unwrap()]));

    let refused = home.cockpit_err(&["checkout", "push", &main]);

    assert!(refused.contains("redirects pushes"), "{refused}");
    assert!(!home.git_log().iter().any(|call| call.contains(" push ")));
}

#[test]
fn a_push_that_would_rewrite_the_remote_branch_is_refused() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));
    let path = home.home().join("projects/.parallel/app/feature-x");
    std::fs::write(path.join("work"), "first\n").unwrap();
    commit_all(&path, "first");
    home.cockpit_ok(&["checkout", "push", &clone]);
    let published = real_git(&home.github(), &["rev-parse", "refs/heads/feature/x"]);
    real_git(&path, &["commit", "--quiet", "--amend", "-m", "rewritten"]);

    home.cockpit_err(&["checkout", "push", &clone]);

    assert_eq!(
        real_git(&home.github(), &["rev-parse", "refs/heads/feature/x"]),
        published
    );
    assert!(
        !home
            .git_log()
            .iter()
            .any(|call| call.contains("--force") || call.contains(" +refs/")),
        "{:?}",
        home.git_log()
    );
}

#[test]
fn a_project_verb_without_a_running_daemon_fails_naming_the_socket() {
    let home = TestHome::new();

    let refused = home.cockpit_err(&["project", "list"]);

    assert!(refused.contains("daemon.sock"), "{refused}");
}

#[test]
fn an_unknown_checkout_is_not_found() {
    let home = TestHome::new();
    let _daemon = home.start();

    let refused = home.cockpit_err(&["checkout", "claim", "42", "session-a"]);

    assert!(refused.contains("not-found"), "{refused}");
}

#[test]
fn a_clean_review_clone_is_swept() {
    let home = TestHome::new();
    let root = home.project("app");
    real_git(
        &root,
        &[
            "push",
            "--quiet",
            home.github().to_str().unwrap(),
            "main:refs/pull/7/head",
        ],
    );
    let _daemon = home.start();
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let review = id_of(&home.cockpit_ok(&["checkout", "review", "app", "7"]));

    let swept = home.cockpit_ok(&["checkout", "sweep"]);

    assert!(
        swept.starts_with(&format!("removed\t{review}\t")),
        "{swept}"
    );
}

#[test]
fn a_content_filter_a_clone_configures_does_not_run_on_host_git() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]);
    let path = home.home().join("projects/.parallel/app/feature-x");
    let marker = home.home().join("filter-ran");
    real_git(
        &path,
        &[
            "config",
            "filter.planted.clean",
            &format!("touch {}; cat", marker.display()),
        ],
    );
    std::fs::write(path.join(".git/info/attributes"), "* filter=planted\n").unwrap();
    // Same content, new modification time: status must look inside the file again.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(path.join("README"), "fixture\n").unwrap();

    home.cockpit_ok(&["checkout", "sweep"]);

    assert!(!marker.exists(), "the clone's filter ran on host Git");
}

#[test]
fn a_push_to_an_origin_with_a_second_url_is_refused() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    real_git(
        &root,
        &[
            "config",
            "--add",
            "remote.origin.url",
            "https://example.com/elsewhere.git",
        ],
    );
    let main = id_of(&home.cockpit_ok(&["project", "add", root.to_str().unwrap()]));

    let refused = home.cockpit_err(&["checkout", "push", &main]);

    assert!(refused.contains("refused"), "{refused}");
    assert!(!home.git_log().iter().any(|call| call.contains(" push ")));
}

#[test]
fn a_clone_deleted_and_cloned_again_at_its_path_is_refused_as_replaced() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));
    let path = home.home().join("projects/.parallel/app/feature-x");
    std::fs::remove_dir_all(&path).unwrap();
    real_git(
        &home.home(),
        &[
            "clone",
            "--quiet",
            root.to_str().unwrap(),
            path.to_str().unwrap(),
        ],
    );

    let refused = home.cockpit_err(&["checkout", "claim", &clone, "session-a"]);

    assert!(refused.contains("stale-revision"), "{refused}");
}

#[test]
fn a_half_deleted_clone_replaced_before_the_next_sweep_is_kept() {
    use std::os::unix::fs::PermissionsExt;
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));
    let path = home.home().join("projects/.parallel/app/feature-x");
    let locked = path.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("file"), "ignored\n").unwrap();
    std::fs::write(path.join(".git/info/exclude"), "locked/\n").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let interrupted = home.cockpit_ok(&["checkout", "sweep"]);
    assert!(
        interrupted.starts_with(&format!("kept\t{clone}\t")),
        "{interrupted}"
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir_all(&path).unwrap();
    real_git(
        &home.home(),
        &[
            "clone",
            "--quiet",
            root.to_str().unwrap(),
            path.to_str().unwrap(),
        ],
    );

    let swept = home.cockpit_ok(&["checkout", "sweep"]);

    assert!(swept.starts_with(&format!("kept\t{clone}\t")), "{swept}");
    assert!(path.join(".git").exists());
}

#[test]
fn a_clone_at_a_path_a_live_checkout_still_holds_is_refused_and_leaves_no_directory() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]);
    let path = home.home().join("projects/.parallel/app/feature-x");
    std::fs::remove_dir_all(&path).unwrap();

    let refused = home.cockpit_err(&["checkout", "clone", "app", "feature/x"]);

    assert!(refused.contains("conflict"), "{refused}");
    assert!(!path.exists());
}

#[test]
fn a_claim_with_an_unreadable_owner_config_is_refused_before_it_is_recorded() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    let main = id_of(&home.cockpit_ok(&["project", "add", root.to_str().unwrap()]));
    let config = home.home().join("config/cockpit/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[projects\n").unwrap();

    home.cockpit_err(&["checkout", "claim", &main, "session-a"]);

    std::fs::remove_file(&config).unwrap();
    assert_eq!(checkout_line(&home, &main)[4], "-");
}

#[test]
fn a_signature_program_a_clone_configures_does_not_run_during_the_sweep() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]);
    let path = home.home().join("projects/.parallel/app/feature-x");
    let marker = home.home().join("gpg-ran");
    let program = home.home().join("planted-gpg");
    std::fs::write(&program, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
    std::fs::set_permissions(
        &program,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    real_git(&path, &["config", "log.showSignature", "true"]);
    real_git(&path, &["config", "gpg.program", program.to_str().unwrap()]);
    let tree = real_git(&path, &["rev-parse", "HEAD^{tree}"]);
    let parent = real_git(&path, &["rev-parse", "HEAD"]);
    let commit = format!(
        "tree {}\nparent {}\nauthor a <a@b> 0 +0000\ncommitter a <a@b> 0 +0000\n\
         gpgsig -----BEGIN PGP SIGNATURE-----\n \n -----END PGP SIGNATURE-----\n\nsigned\n",
        tree.trim(),
        parent.trim()
    );
    let object = home.home().join("signed-commit");
    std::fs::write(&object, commit).unwrap();
    let signed = real_git(
        &path,
        &[
            "hash-object",
            "-t",
            "commit",
            "-w",
            object.to_str().unwrap(),
        ],
    );
    real_git(&path, &["reset", "--quiet", "--soft", signed.trim()]);

    home.cockpit_ok(&["checkout", "sweep"]);

    assert!(!marker.exists(), "the clone's signature program ran");
}

#[test]
fn the_sweep_keeps_a_clone_with_submodules() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));
    let path = home.home().join("projects/.parallel/app/feature-x");
    std::fs::write(path.join(".gitmodules"), "").unwrap();
    commit_all(&path, "submodules");
    real_git(
        &path,
        &[
            "push",
            "--quiet",
            home.github().to_str().unwrap(),
            "HEAD:refs/heads/feature/x",
        ],
    );
    real_git(
        &path,
        &[
            "fetch",
            "--quiet",
            home.github().to_str().unwrap(),
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );

    let swept = home.cockpit_ok(&["checkout", "sweep"]);

    assert!(swept.starts_with(&format!("kept\t{clone}\t")), "{swept}");
    assert!(swept.contains("submodules"), "{swept}");
}

#[test]
fn a_push_rewritten_off_github_by_the_owners_configuration_is_refused() {
    let home = TestHome::new();
    std::fs::write(
        home.home().join(".gitconfig"),
        "[url \"https://example.com/\"]\n\tinsteadOf = https://github.com/\n",
    )
    .unwrap();
    let _daemon = home.start();
    let root = home.project("app");
    let main = id_of(&home.cockpit_ok(&["project", "add", root.to_str().unwrap()]));

    let refused = home.cockpit_err(&["checkout", "push", &main]);

    assert!(refused.contains("https://example.com/"), "{refused}");
    assert!(!home.git_log().iter().any(|call| call.contains(" push ")));
}

#[test]
fn a_push_through_a_proxy_the_checkout_configures_is_refused() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    real_git(
        &root,
        &["config", "http.proxy", "http://attacker.example:8080"],
    );
    let main = id_of(&home.cockpit_ok(&["project", "add", root.to_str().unwrap()]));

    let refused = home.cockpit_err(&["checkout", "push", &main]);

    assert!(refused.contains("redirects pushes"), "{refused}");
    assert!(!home.git_log().iter().any(|call| call.contains(" push ")));
}

#[test]
fn a_dry_run_keeps_an_occupied_clone_whose_directory_is_gone() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));
    home.cockpit_ok(&["checkout", "claim", &clone, "session-a"]);
    std::fs::remove_dir_all(home.home().join("projects/.parallel/app/feature-x")).unwrap();

    let swept = home.cockpit_ok(&["checkout", "sweep", "--dry-run"]);

    assert!(swept.starts_with(&format!("kept\t{clone}\t")), "{swept}");
    assert!(swept.contains("occupied by session-a"), "{swept}");
}

#[test]
fn the_next_sweep_finishes_an_interrupted_deletion() {
    use std::os::unix::fs::PermissionsExt;
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));
    let path = home.home().join("projects/.parallel/app/feature-x");
    let locked = path.join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("file"), "ignored\n").unwrap();
    std::fs::write(path.join(".git/info/exclude"), "locked/\n").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    home.cockpit_ok(&["checkout", "sweep"]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    let swept = home.cockpit_ok(&["checkout", "sweep"]);

    assert_eq!(swept, format!("removed\t{clone}\t{}\n", path.display()));
    assert!(!path.exists());
}

#[test]
fn the_sweep_keeps_a_clone_holding_a_committed_nested_repository() {
    let home = TestHome::new();
    let _daemon = home.start();
    let root = home.project("app");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    let clone = id_of(&home.cockpit_ok(&["checkout", "clone", "app", "feature/x"]));
    let path = home.home().join("projects/.parallel/app/feature-x");
    let nested = path.join("vendor");
    real_git(
        &path,
        &[
            "clone",
            "--quiet",
            root.to_str().unwrap(),
            nested.to_str().unwrap(),
        ],
    );
    real_git(&path, &["add", "vendor"]);
    real_git(&path, &["commit", "--quiet", "-m", "embed"]);
    real_git(
        &path,
        &[
            "push",
            "--quiet",
            home.github().to_str().unwrap(),
            "HEAD:refs/heads/feature/x",
        ],
    );
    real_git(
        &path,
        &[
            "fetch",
            "--quiet",
            home.github().to_str().unwrap(),
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    std::fs::write(nested.join("draft"), "work\n").unwrap();

    let swept = home.cockpit_ok(&["checkout", "sweep"]);

    assert!(swept.starts_with(&format!("kept\t{clone}\t")), "{swept}");
    assert!(nested.join("draft").exists());
}

#[test]
fn a_push_url_rewritten_off_github_by_the_owners_configuration_is_refused() {
    let home = TestHome::new();
    std::fs::write(
        home.home().join(".gitconfig"),
        "[url \"https://example.com/\"]\n\tpushInsteadOf = https://github.com/\n",
    )
    .unwrap();
    let _daemon = home.start();
    let root = home.project("app");
    let main = id_of(&home.cockpit_ok(&["project", "add", root.to_str().unwrap()]));

    let refused = home.cockpit_err(&["checkout", "push", &main]);

    assert!(refused.contains("https://example.com/"), "{refused}");
    assert!(!home.git_log().iter().any(|call| call.contains(" push ")));
}
