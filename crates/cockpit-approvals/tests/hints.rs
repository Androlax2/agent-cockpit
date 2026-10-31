//! What the owner is told before approving, beyond the text itself: a `timeout` that would
//! take the terminal from the command, and build files or scripts the session changed that
//! the command would run (#156).

use cockpit_approvals::{build_files_run, timeout_without_foreground};

#[test]
fn timeout_without_foreground_gets_the_hint() {
    for command in [
        "timeout 60 sudo pacman -Syu",
        "timeout -s KILL 5 make test",
        "cd x && /usr/bin/timeout 5 read -r line",
        "echo a; timeout --kill-after=3 10 ./run.sh",
    ] {
        assert!(timeout_without_foreground(command), "{command:?}");
    }
}

#[test]
fn timeout_with_foreground_or_no_timeout_gets_no_hint() {
    for command in [
        "timeout --foreground 60 sudo pacman -Syu",
        "timeout -k 3 --foreground 10 make",
        "timeout --fore 10 make",
        "make timeout-test",
        "echo timeout",
        "cargo test -- timeout",
    ] {
        assert!(!timeout_without_foreground(command), "{command:?}");
    }
}

fn changed(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| (*path).to_owned()).collect()
}

#[test]
fn each_build_tool_names_the_changed_files_it_runs() {
    for (command, changed_files, expected) in [
        (
            "make test",
            &["Makefile", "src/lib.rs"][..],
            &["Makefile"][..],
        ),
        (
            "make -C tools all",
            &["tools/rules.mk"],
            &["tools/rules.mk"],
        ),
        (
            "cargo build --release",
            &["crates/x/build.rs", "README.md"],
            &["crates/x/build.rs"],
        ),
        (
            "cargo test",
            &[".cargo/config.toml"],
            &[".cargo/config.toml"],
        ),
        (
            "pnpm run lint",
            &["web/package.json"],
            &["web/package.json"],
        ),
        ("npm install", &["package.json"], &["package.json"]),
        (
            "gh workflow run ci.yml",
            &[".github/workflows/ci.yml"],
            &[".github/workflows/ci.yml"],
        ),
        (
            "git commit -m x",
            &[".husky/pre-commit", ".pre-commit-config.yaml"],
            &[".husky/pre-commit", ".pre-commit-config.yaml"],
        ),
    ] {
        assert_eq!(
            build_files_run(command, &changed(changed_files)),
            changed(expected),
            "{command:?}"
        );
    }
}

#[test]
fn a_changed_script_the_command_names_is_listed() {
    let changed_files = changed(&["scripts/deploy.sh", "tools/check.py", "docs/a.md"]);
    assert_eq!(
        build_files_run(
            "bash scripts/deploy.sh && python3 ./tools/check.py",
            &changed_files
        ),
        changed(&["scripts/deploy.sh", "tools/check.py"])
    );
}

#[test]
fn a_command_that_runs_nothing_changed_gets_no_warning() {
    let changed_files = changed(&["Makefile", "build.rs", "package.json", "scripts/x.sh"]);
    for command in ["ls -la", "cat README.md", "sudo pacman -Syu"] {
        assert!(
            build_files_run(command, &changed_files).is_empty(),
            "{command:?}"
        );
    }
}
