//! make system (tools/system-install.sh): every file reviewed, nothing setuid.

mod common;
use common::sandbox::*;

use std::fs;
use std::os::unix::fs::PermissionsExt;

struct Install {
    sandbox: Sandbox,
}

impl Install {
    fn new() -> Install {
        let sandbox = Sandbox::new();
        fs::create_dir_all(sandbox.root.join("source/etc")).unwrap();
        fs::create_dir_all(sandbox.root.join("target")).unwrap();
        Install { sandbox }
    }

    fn file(&self, name: &str, content: &str, mode: u32) {
        let path = self.sandbox.root.join("source/etc").join(name);
        fs::write(&path, content).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn run(&self, answers: &str) -> String {
        let script = repo().join("tools/system-install.sh");
        let source = self.sandbox.root.join("source").display().to_string();
        let target = self.sandbox.root.join("target").display().to_string();
        stdout(&self.sandbox.run_with_input("bash", &[script.to_str().unwrap()],
                                            &[("SYSTEM_SOURCE", &source), ("SYSTEM_ROOT", &target), ("SUDO", "")], answers))
    }

    fn installed(&self, name: &str) -> bool {
        self.sandbox.root.join("target/etc").join(name).exists()
    }
}

#[test]
fn a_changed_file_shows_its_diff_and_waits_for_yes() {
    let install = Install::new();
    install.file("app.conf", "setting=1\n", 0o644);
    let shown = install.run("\n");
    assert!(shown.contains("+setting=1"), "{shown}");
    assert!(!install.installed("app.conf"));
    install.run("y\n");
    assert!(install.installed("app.conf"));
}

#[test]
fn a_setuid_file_is_refused() {
    let install = Install::new();
    install.file("tool", "#!/bin/sh\n", 0o4755);
    let shown = install.run("y\n");
    assert!(shown.contains("refused"), "{shown}");
    assert!(!install.installed("tool"));
}

#[test]
fn control_characters_in_a_diff_are_made_visible() {
    let install = Install::new();
    install.file("sneaky.conf", "ok=1\n\x1b[8mhidden=1\n", 0o644);
    let shown = install.run("\n");
    assert!(!shown.contains('\x1b'), "{shown:?}");
    assert!(shown.contains("^["), "{shown}");
}
