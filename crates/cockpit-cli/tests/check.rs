//! Issue #313 through `cockpit check` against the real daemon: the owner runs a project's
//! check, the receipt it yields passes the gate, and an edit to a file it covered turns the
//! gate's report for that check stale. A session's side (claims, the gate of its checkout) is
//! proven through the sockets in `cockpit-daemon/tests/verification.rs`.
mod common;

use std::path::PathBuf;

use common::{TestHome, commit_all, real_git};

const CHECKS: &str = "[[checks]]\nname = \"test\"\ncommand = \"echo testing; test -f src/lib.rs\"\n\
                      artifacts = [\"src/lib.rs\"]\n";

/// The real daemon with project `app` registered and trusted, declaring [`CHECKS`] in its
/// last commit.
fn with_checks() -> (TestHome, common::RunningDaemon, PathBuf) {
    let (home, daemon, root) = with_untrusted_checks();
    home.trust(&root);
    (home, daemon, root)
}

/// [`with_checks`] before the owner trusts the project.
fn with_untrusted_checks() -> (TestHome, common::RunningDaemon, PathBuf) {
    let home = TestHome::new();
    let daemon = home.start();
    let root = home.project("app");
    std::fs::create_dir_all(root.join(".torlek")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join(".torlek/config.toml"), CHECKS).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n").unwrap();
    commit_all(&root, "declare the checks");
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    (home, daemon, root)
}

fn gate(home: &TestHome) -> (String, bool) {
    let output = home.cockpit(&["check", "gate", "--project", "app"]);
    (
        String::from_utf8(output.stdout).unwrap(),
        output.status.success(),
    )
}

#[test]
fn a_check_the_owner_runs_prints_its_output_and_passes_the_gate() {
    let (home, _daemon, _root) = with_checks();

    let run = home.cockpit_ok(&["check", "run", "test", "--project", "app"]);

    assert!(
        run.starts_with("testing\ncheck test passed (exit 0): receipt 1 for run execution-"),
        "{run}"
    );
    let (report, passed) = gate(&home);
    assert!(passed, "{report}");
    assert!(report.starts_with("gate passed for app at "), "{report}");
    assert!(
        report.contains("\ntest\tchecked: passed\treceipt 1\n"),
        "{report}"
    );
}

#[test]
fn editing_a_covered_file_turns_the_checks_gate_report_stale() {
    let (home, _daemon, root) = with_checks();
    home.cockpit_ok(&["check", "run", "test", "--project", "app"]);

    std::fs::write(root.join("src/lib.rs"), "pub fn answer() -> u8 { 41 }\n").unwrap();

    let (report, passed) = gate(&home);
    assert!(!passed, "{report}");
    assert!(report.starts_with("gate open for app at "), "{report}");
    assert!(report.contains("uncommitted changes"), "{report}");
    let line = report
        .lines()
        .find(|line| line.starts_with("test\t"))
        .unwrap_or_else(|| panic!("no test line in {report}"));
    assert!(
        line.starts_with("test\tstale: source-changed (ran on "),
        "{line}"
    );
    assert!(
        line.ends_with("\tnext: run `test` again on the current result"),
        "{line}"
    );
}

#[test]
fn a_check_that_fails_exits_non_zero_with_its_receipt() {
    let (home, _daemon, root) = with_checks();
    std::fs::remove_file(root.join("src/lib.rs")).unwrap();
    std::fs::write(
        root.join(".torlek/config.toml"),
        CHECKS.replace("artifacts = [\"src/lib.rs\"]\n", ""),
    )
    .unwrap();

    let output = home.cockpit(&["check", "run", "test", "--project", "app"]);

    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("check test failed (exit 1)"), "{stdout}");
}

#[test]
fn a_gate_before_any_run_names_the_check_to_run() {
    let (home, _daemon, _root) = with_checks();

    let (report, passed) = gate(&home);

    assert!(!passed, "{report}");
    assert!(
        report.contains("\ntest\tmissing\tnext: run `test`\n"),
        "{report}"
    );
}

#[test]
fn the_gate_of_an_untrusted_project_names_the_trust_step_before_the_run() {
    let (home, _daemon, _root) = with_untrusted_checks();

    let (report, passed) = gate(&home);

    assert!(!passed, "{report}");
    assert!(
        report.contains(
            "\ntest\tmissing\tnext: trust the project under `projects.trusted` in the owner \
             configuration, then run `test`\n"
        ),
        "{report}"
    );
}

#[test]
fn the_gate_of_a_review_checkout_names_the_projects_own_checkout() {
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
    std::fs::create_dir_all(contributor.join(".torlek")).unwrap();
    std::fs::write(contributor.join(".torlek/config.toml"), CHECKS).unwrap();
    commit_all(&contributor, "declare the checks");
    real_git(
        &contributor,
        &["push", "--quiet", "origin", "HEAD:refs/pull/7/head"],
    );
    let _daemon = home.start();
    home.cockpit_ok(&["project", "add", root.to_str().unwrap()]);
    home.cockpit_ok(&["checkout", "review", "app", "7"]);
    let clone = home.home().join("projects/.parallel/app/pr-7");

    let output = home.cockpit(&[
        "check",
        "gate",
        "--project",
        "app",
        "--checkout",
        clone.to_str().unwrap(),
    ]);

    let report = String::from_utf8(output.stdout).unwrap();
    assert!(
        report.contains(
            "\ntest\tmissing\tnext: in the project's own checkout or a parallel clone, run \
             `test`: a review checkout's checks never run\n"
        ),
        "{report}{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn running_a_check_without_its_project_is_a_usage_error() {
    let (home, _daemon, _root) = with_checks();

    let error = home.cockpit_err(&["check", "run", "test"]);

    assert!(error.contains("--project is required"), "{error}");
}

#[test]
fn a_gate_takes_its_checkout_relative_to_the_working_directory() {
    let (home, _daemon, root) = with_checks();

    let output = home.cockpit_in(
        &root,
        &["check", "gate", "--project", "app", "--checkout", "."],
    );

    let report = String::from_utf8(output.stdout).unwrap();
    assert!(report.starts_with("gate open for app at "), "{report}");
}

#[test]
fn pending_help_and_empty_owner_listing_do_not_invent_a_result() {
    let (home, _daemon, _root) = with_checks();
    let output = home.cockpit_ok(&["check", "pending", "--project", "app"]);
    assert!(output.contains("current-database-epoch"));
    assert_eq!(output.lines().count(), 1, "only the header: {output}");
    let refused = home.cockpit(&[
        "check",
        "resolve-pending",
        "execution-old-1",
        "--project",
        "app",
        "--revision",
        "1",
    ]);
    assert!(!refused.status.success());
    assert!(
        String::from_utf8(refused.stderr)
            .unwrap()
            .contains("--epoch")
    );
}
