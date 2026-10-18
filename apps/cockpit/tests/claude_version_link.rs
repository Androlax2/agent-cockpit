//! claude-version-link: finishes Claude Code's self-updates that greywall keeps from completing.

mod common;
use common::sandbox::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::Output;

struct Versions {
    sandbox: Sandbox,
    versions: PathBuf,
    link: PathBuf,
}

impl Versions {
    fn new() -> Versions {
        let sandbox = Sandbox::new();
        let versions = sandbox.root.join("versions");
        fs::create_dir(&versions).unwrap();
        let link = sandbox.root.join("claude");
        Versions { sandbox, versions, link }
    }

    fn version(&self, name: &str, starts: bool) {
        let path = self.versions.join(name);
        fs::write(&path, if starts { "#!/bin/sh\nexit 0\n" } else { "#!/bin/sh\nexit 1\n" }).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn link_to(&self, name: &str) {
        symlink(self.versions.join(name), &self.link).unwrap();
    }

    fn run_link(&self) -> Output {
        let (versions, link) = (self.versions.display().to_string(), self.link.display().to_string());
        self.sandbox.run(bin().join("claude-version-link"), &[], &[("CLAUDE_VERSIONS_DIR", &versions), ("CLAUDE_LINK", &link)])
    }

    fn target(&self) -> String {
        fs::read_link(&self.link).unwrap().file_name().unwrap().to_string_lossy().into_owned()
    }
}

#[test]
fn moves_the_link_to_the_newest_version() {
    let versions = Versions::new();
    for name in ["2.1.300", "2.1.302", "2.1.99"] {
        versions.version(name, true);
    }
    versions.link_to("2.1.300");
    versions.run_link();
    assert_eq!(versions.target(), "2.1.302");
}

#[test]
fn skips_a_version_that_does_not_start() {
    let versions = Versions::new();
    versions.version("2.1.300", true);
    versions.version("2.1.301", true);
    versions.version("2.1.302", false);
    versions.version("2.1.303.partial", true);
    versions.link_to("2.1.300");
    versions.run_link();
    assert_eq!(versions.target(), "2.1.301");
}

#[test]
fn never_moves_backwards_from_a_pinned_newer_version() {
    let versions = Versions::new();
    versions.version("2.1.300", true);
    versions.version("2.1.305", false);
    versions.link_to("2.1.305");
    versions.run_link();
    assert_eq!(versions.target(), "2.1.305");
}

#[test]
fn creates_the_link_when_there_is_none() {
    let versions = Versions::new();
    versions.version("2.1.302", true);
    assert_eq!(versions.run_link().status.code(), Some(0));
    assert_eq!(versions.target(), "2.1.302");
}
