//! make handoff-install (tools/handoff-install.sh): checksums, a diff, a confirmation, then cleanup.

mod common;
use common::sandbox::*;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Handoff {
    sandbox: Sandbox,
    repo: PathBuf,
    group: PathBuf,
}

impl Handoff {
    fn new() -> Handoff {
        let sandbox = Sandbox::new();
        let repo = sandbox.root.join("repo");
        fs::create_dir_all(repo.join("config/claude")).unwrap();
        fs::write(repo.join("config/claude/hook.sh"), "old\n").unwrap();
        let group = repo.join(".handoff/claude");
        fs::create_dir_all(&group).unwrap();
        fs::write(group.join("hook.sh"), "new\n").unwrap();
        let handoff = Handoff { sandbox, repo, group };
        handoff.manifest("hook.sh\tconfig/claude/hook.sh\techo ran >> after.log\n");
        handoff.sums();
        handoff
    }

    fn manifest(&self, text: &str) {
        fs::write(self.group.join("MANIFEST"), text).unwrap();
    }

    fn sums(&self) {
        let output = Command::new("sha256sum").arg(self.group.join("hook.sh")).output().unwrap();
        let digest = stdout(&output).split_whitespace().next().unwrap().to_string();
        fs::write(self.group.join("SHA256SUMS"), format!("{digest}  hook.sh\n")).unwrap();
    }

    fn install(&self, answer: &str) -> Output {
        let script = repo().join("tools/handoff-install.sh");
        let handoff_repo = self.repo.display().to_string();
        self.sandbox.run_with_input("bash", &[script.to_str().unwrap()], &[("HANDOFF_REPO", &handoff_repo)], answer)
    }

    fn installed(&self) -> String {
        fs::read_to_string(self.repo.join("config/claude/hook.sh")).unwrap()
    }
}

#[test]
fn installs_on_yes_runs_the_follow_up_once_and_cleans_up() {
    let handoff = Handoff::new();
    let output = handoff.install("y\n");
    assert_eq!(output.status.code(), Some(0), "{}{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("+new"));
    assert!(stdout(&output).contains("then runs: echo ran >> after.log"));
    assert_eq!(handoff.installed(), "new\n");
    assert_eq!(fs::read_to_string(handoff.repo.join("after.log")).unwrap(), "ran\n");
    assert!(!handoff.group.exists());
}

#[test]
fn no_leaves_everything_as_it_was() {
    let handoff = Handoff::new();
    handoff.install("n\n");
    assert_eq!(handoff.installed(), "old\n");
    assert!(handoff.group.exists());
}

#[test]
fn a_checksum_mismatch_installs_nothing() {
    let handoff = Handoff::new();
    fs::write(handoff.group.join("hook.sh"), "tampered\n").unwrap();
    let output = handoff.install("y\n");
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("checksum mismatch"));
    assert_eq!(handoff.installed(), "old\n");
}

#[test]
fn a_destination_outside_the_repository_is_refused() {
    let handoff = Handoff::new();
    handoff.manifest("hook.sh\t../../.bashrc\n");
    let output = handoff.install("y\n");
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout(&output).contains("refusing"));
}

#[test]
fn an_installed_group_is_just_removed() {
    let handoff = Handoff::new();
    fs::write(handoff.repo.join("config/claude/hook.sh"), "new\n").unwrap();
    let output = handoff.install("");
    assert!(stdout(&output).contains("already installed"));
    assert!(!handoff.group.exists());
}
