//! Load the real tmux configuration on a private server, never the host's server.
mod common;
use common::sandbox::*;
use std::fs;
use std::process::Output;

struct Tmux {
    sandbox: Sandbox,
}

impl Tmux {
    fn new() -> Self {
        let sandbox = Sandbox::new();
        let theme = sandbox.home.join(".config/tmux");
        fs::create_dir_all(&theme).unwrap();
        fs::copy(repo().join("config/tmux/.config/tmux/tokyonight_night.tmux"), theme.join("tokyonight_night.tmux")).unwrap();
        let tmux = Self { sandbox };
        tmux.run(&["-f", "/dev/null", "new-session", "-d", "-s", "bootstrap", "sleep 60"]);
        tmux.run(&["source-file", repo().join("config/tmux/.config/tmux/tmux.conf").to_str().unwrap()]);
        tmux
    }

    fn run(&self, arguments: &[&str]) -> Output {
        let output = self.sandbox.command("/usr/bin/tmux", &[])
            .env_remove("TMUX")
            .args(["-S", self.sandbox.root.join("tmux.sock").to_str().unwrap()])
            .args(arguments).output().unwrap();
        assert!(output.status.success(), "{arguments:?}: {}", stderr(&output));
        output
    }
}

impl Drop for Tmux {
    fn drop(&mut self) {
        let _ = self.sandbox.command("/usr/bin/tmux", &[])
            .args(["-S", self.sandbox.root.join("tmux.sock").to_str().unwrap(), "kill-server"])
            .output();
    }
}

#[test]
fn only_opencode_and_cockpit_windows_allow_terminal_passthrough() {
    let tmux = Tmux::new();
    for (session, expected) in [
        ("claude-manager", "on"),
        ("claude-retry-opencode-123", "on"),
        ("ordinary", "off"),
        ("claude-run-123", "off"),
        ("claude-retry-123", "off"),
        ("not-claude-retry-opencode-123", "off"),
    ] {
        tmux.run(&["new-session", "-d", "-s", session, "sleep 60"]);
        let output = tmux.run(&["show-options", "-A", "-w", "-v", "-t", &format!("={session}:0"), "allow-passthrough"]);
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected, "{session}");
    }
}
