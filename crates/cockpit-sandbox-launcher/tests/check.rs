//! #862: a check prepared by `Sandbox::prepare_check` and run by `torlek-sandbox` with the
//! real bubblewrap on this host. It works in its checkout and reaches none of what a session's
//! agent reaches beside it: the agent socket folder, the bridge programs, Torlek's state, the
//! owner's keys. Needs `bwrap` on `PATH`, as CI installs it.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;

use cockpit_sandbox::{
    AgentSandbox, Backend, CheckRequest, HostPaths, Platform, Sandbox, ToolNeeds,
};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_torlek-sandbox");

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let root = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let fixture = Fixture { root };
        for folder in [
            "checkout",
            "owner/.ssh",
            "agent-sockets",
            "bridges",
            "state",
            "execution/tmp",
        ] {
            std::fs::create_dir_all(fixture.path(folder)).unwrap();
        }
        for (file, contents) in [
            ("owner/.ssh/id_ed25519", "private key\n"),
            ("agent-sockets/agent.sock", "socket stand-in\n"),
            ("bridges/torlek-hook", "bridge\n"),
            ("state/state.db", "state\n"),
        ] {
            std::fs::write(fixture.path(file), contents).unwrap();
        }
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().canonicalize().unwrap().join(relative)
    }

    fn sandbox(&self) -> Sandbox {
        let backend = Backend::probe(Path::new(LAUNCHER), std::env::var_os("PATH").as_deref());
        assert!(
            backend.missing_reason().is_none(),
            "{:?}",
            backend.missing_reason()
        );
        Sandbox::new(
            Platform::Linux,
            HostPaths {
                read_only: vec![self.path("bridges")],
                sockets: vec![self.path("agent-sockets")],
                denied: vec![self.path("state/state.db")],
                denied_write: Vec::new(),
                owner_uid: 1000,
                mount_sources: self.path("mounts"),
                homes: self.path("homes"),
            },
            backend,
        )
    }

    /// What the check `command` printed in the sandbox, ending `exit=<code>`.
    fn run_check(&self, command: &str) -> String {
        let request = CheckRequest {
            run: "r1".to_owned(),
            check: "fixture".to_owned(),
            shell: PathBuf::from("/bin/sh"),
            command: command.to_owned(),
            checkout: self.path("checkout"),
            execution_dir: self.path("execution"),
            temp_dir: self.path("execution/tmp"),
            environment: vec![
                ("HOME".into(), self.path("owner").into()),
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("TMPDIR".into(), self.path("execution/tmp").into()),
            ],
            tools: ToolNeeds::none(),
        };
        let prepared = self
            .sandbox()
            .prepare_check(AgentSandbox::Required, &request)
            .unwrap();
        prepared.staged.as_ref().unwrap().write().unwrap();
        let output = Command::new(&prepared.program)
            .args(&prepared.arguments)
            .env_clear()
            .envs(prepared.environment)
            .output()
            .unwrap();
        format!(
            "{}{}exit={}\n",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            output.status.code().unwrap_or(-1)
        )
    }
}

#[test]
fn a_sandboxed_check_writes_its_checkout_and_temp_folder() {
    let fixture = Fixture::new();

    let output = fixture.run_check("echo built > out.txt && echo cached > \"$TMPDIR/cache\"");

    assert!(output.ends_with("exit=0\n"), "{output}");
    assert_eq!(
        std::fs::read_to_string(fixture.path("checkout/out.txt")).unwrap(),
        "built\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.path("execution/tmp/cache")).unwrap(),
        "cached\n"
    );
}

#[test]
fn a_sandboxed_check_reaches_no_agent_socket_bridge_state_or_key() {
    let fixture = Fixture::new();
    let reached = [
        fixture.path("agent-sockets/agent.sock"),
        fixture.path("bridges/torlek-hook"),
        fixture.path("state/state.db"),
        fixture.path("owner/.ssh/id_ed25519"),
    ];
    let probes: Vec<String> = reached
        .iter()
        .map(|path| {
            format!(
                "cat '{}' 2>/dev/null && echo 'read {}'",
                path.display(),
                path.display()
            )
        })
        .collect();

    let output = fixture.run_check(&format!("{}; true", probes.join("; ")));

    assert!(output.ends_with("exit=0\n"), "{output}");
    assert!(!output.contains("read "), "{output}");
}
