//! The readiness check and the command lines, through a fake `tmux` that prints a chosen
//! version and records every argument list it receives.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_tmux::{
    Error, PaneState, Server, ServerSpec, SessionLaunch, SessionName, Socket, config,
};

/// The history limit these specs ask for; the fake reads none of it.
const HISTORY_LIMIT: u32 = 10_000;

struct FakeTmux {
    directory: tempfile::TempDir,
}

impl FakeTmux {
    /// A fake whose `-V` prints `version` and whose other commands print `%7`.
    fn printing_version(version: &str) -> FakeTmux {
        FakeTmux::printing(version, "%7")
    }

    /// A fake whose `-V` prints `version` and whose other commands print `output`.
    fn printing(version: &str, output: &str) -> FakeTmux {
        FakeTmux::printing_server(version, version.trim_start_matches("tmux "), output)
    }

    fn printing_server(version: &str, server_version: &str, output: &str) -> FakeTmux {
        Self::with_server_response(version, &format!("echo '{server_version}'"), output)
    }

    fn with_server_response(version: &str, response: &str, output: &str) -> FakeTmux {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let log = directory.path().join("calls");
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = -V ]; then echo '{version}'; exit 0; fi\n\
             for argument in \"$@\"; do printf '%s\\n' \"$argument\"; done >> '{log}'\n\
             echo >> '{log}'\n\
             case \"$*\" in\n\
               *' display-message -p #{{version}}') {response} ;;\n\
               *) echo '{output}' ;;\n\
             esac\n",
            log = log.display()
        );
        let binary = directory.path().join("tmux");
        fs::write(&binary, script).expect("the fake is writable");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).expect("chmod");
        FakeTmux { directory }
    }

    fn spec(&self, socket: Socket) -> ServerSpec {
        ServerSpec {
            tmux: self.directory.path().join("tmux"),
            socket,
            config_path: self.config_path(),
            history_limit: HISTORY_LIMIT,
        }
    }

    fn config_path(&self) -> PathBuf {
        self.directory.path().join("tmux.conf")
    }

    /// [`Server::open`], then the recorded calls forgotten: the tests below check what a
    /// command runs, not the config [`Server::open`] applies.
    fn open(&self) -> Server {
        let server = Server::open(self.spec(Socket::cockpit())).expect("opens");
        fs::remove_file(self.directory.path().join("calls")).expect("open ran tmux");
        server
    }

    /// Each recorded call's arguments.
    fn calls(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.directory.path().join("calls"))
            .unwrap_or_default()
            .split("\n\n")
            .filter(|call| !call.is_empty())
            .map(|call| call.lines().map(str::to_string).collect())
            .collect()
    }
}

#[test]
fn a_tmux_before_stable_3_6_is_refused_before_configuring_the_server() {
    for release in ["3.4", "3.1c", "3.2", "3.3a", "3.5a", "next-3.6", "3.6-rc2"] {
        let fake = FakeTmux::printing_version(&format!("tmux {release}"));

        let error = Server::open(fake.spec(Socket::cockpit())).expect_err(release);

        assert!(
            matches!(error, Error::TmuxTooOld { ref found, .. } if found == release),
            "{release}: {error:?}"
        );
        assert!(
            error.to_string().contains("needs tmux 3.6 or newer"),
            "{error}"
        );
        assert!(
            !fake.config_path().exists(),
            "{release}: config was written"
        );
        assert!(fake.calls().is_empty(), "{release}: server commands ran");
    }
}

#[test]
fn stable_3_6_and_newer_release_spellings_are_accepted() {
    for printed in [
        "tmux 3.6",
        "tmux 3.6a",
        "tmux 3.7-rc2",
        "tmux next-3.7",
        "tmux 4.0",
    ] {
        let fake = FakeTmux::printing_version(printed);

        let opened = Server::open(fake.spec(Socket::cockpit()));

        assert!(opened.is_ok(), "{printed}: {opened:?}");
    }
}

#[test]
fn a_version_whose_features_cannot_be_known_is_refused() {
    for printed in ["tmux master", "tmux openbsd-7.5", "screen 4.9", ""] {
        let fake = FakeTmux::printing_version(printed);

        let error = Server::open(fake.spec(Socket::cockpit())).expect_err(printed);

        assert!(
            matches!(error, Error::UnrecognizedVersion { .. }),
            "{printed}: {error:?}"
        );
    }
}

#[test]
fn a_missing_tmux_is_refused_naming_the_binary() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let spec = ServerSpec {
        tmux: directory.path().join("no-tmux-here"),
        socket: Socket::cockpit(),
        config_path: directory.path().join("tmux.conf"),
        history_limit: HISTORY_LIMIT,
    };

    let error = Server::open(spec).expect_err("there is no binary");

    assert!(matches!(error, Error::TmuxNotRunnable { .. }), "{error:?}");
    assert!(error.to_string().contains("no-tmux-here"), "{error}");
}

#[test]
fn upgrading_the_binary_does_not_accept_an_unsupported_running_server() {
    for release in ["3.4", "3.5a", "next-3.6", "3.6-rc2", "master", ""] {
        let fake = FakeTmux::printing_server("tmux 3.8", release, "%7");

        let error = Server::open(fake.spec(Socket::cockpit())).expect_err(release);

        assert!(error.to_string().contains("running tmux server"), "{error}");
        assert!(!fake.config_path().exists(), "{release}");
        let calls = fake.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(&calls[0][4..], ["display-message", "-p", "#{version}"]);
    }
}

#[test]
fn opening_without_a_live_server_still_writes_the_next_servers_config() {
    for message in [
        "no server running on /fixture",
        "error connecting to /fixture (No such file or directory)",
        "error connecting to /fixture (Connection refused)",
        "server exited unexpectedly",
        "lost server",
    ] {
        let fake = FakeTmux::with_server_response(
            "tmux 3.6",
            &format!("echo '{message}' >&2; exit 1"),
            "%7",
        );

        Server::open(fake.spec(Socket::cockpit())).expect(message);

        assert_eq!(
            fs::read_to_string(fake.config_path()).expect("config"),
            config(HISTORY_LIMIT)
        );
    }
}

#[test]
fn a_failed_server_version_query_is_not_treated_as_an_absent_server() {
    for message in [
        "error connecting to /fixture (Permission denied)",
        "error connecting to /fixture (Not a directory)",
        "permission denied",
    ] {
        for existing in [None, Some("existing server configuration\n")] {
            let fake = FakeTmux::with_server_response(
                "tmux 3.6",
                &format!("echo '{message}' >&2; exit 1"),
                "%7",
            );
            if let Some(contents) = existing {
                fs::write(fake.config_path(), contents).expect("existing config");
            }

            let error = Server::open(fake.spec(Socket::cockpit())).expect_err(message);

            assert!(matches!(error, Error::CommandFailed { .. }), "{error:?}");
            assert!(error.to_string().contains(message), "{error}");
            if let Some(contents) = existing {
                assert_eq!(fs::read_to_string(fake.config_path()).unwrap(), contents);
            } else {
                assert!(!fake.config_path().exists());
            }
            assert_eq!(fake.calls().len(), 1);
        }
    }
}

#[test]
fn opening_writes_cockpits_own_config_private_to_the_user() {
    let fake = FakeTmux::printing_version("tmux 3.6");

    Server::open(fake.spec(Socket::cockpit())).expect("3.6 is supported");

    assert_eq!(
        fs::read_to_string(fake.config_path()).expect("written"),
        config(HISTORY_LIMIT)
    );
    let mode = fs::metadata(fake.config_path())
        .expect("exists")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn opening_applies_the_config_to_a_running_server() {
    let fake = FakeTmux::printing_version("tmux 3.6");

    Server::open(fake.spec(Socket::cockpit())).expect("opens");

    let config = fake.config_path().display().to_string();
    assert_eq!(
        fake.calls(),
        vec![
            vec![
                "-f",
                config.as_str(),
                "-L",
                "torlek",
                "display-message",
                "-p",
                "#{version}",
            ],
            vec![
                "-f",
                config.as_str(),
                "-L",
                "torlek",
                "source-file",
                config.as_str()
            ]
        ]
    );
}

fn launch(name: &str, working_dir: &Path) -> SessionLaunch {
    SessionLaunch {
        name: SessionName::new(name).expect("a valid name"),
        working_dir: working_dir.to_path_buf(),
        command: vec!["claude".into(), "--resume".into()],
        environment: vec![("TORLEK_SESSION".to_string(), "s1".into())],
        columns: 120,
        rows: 40,
    }
}

#[test]
fn every_command_uses_the_private_cockpit_server_and_its_config() {
    let fake = FakeTmux::printing_version("tmux 3.6");
    let server = fake.open();

    server
        .new_session(&launch("agent-1", fake.directory.path()))
        .expect("the fake prints a pane id");

    let calls = fake.calls();
    let config = fake.config_path().display().to_string();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0][..4], ["-f", config.as_str(), "-L", "torlek"]);
}

#[test]
fn a_new_session_runs_its_command_without_tmux_variables_with_its_environment() {
    let fake = FakeTmux::printing_version("tmux 3.6");
    let server = fake.open();

    server
        .new_session(&launch("agent-1", fake.directory.path()))
        .expect("the fake prints a pane id");

    let call = &fake.calls()[0];
    let after_options = call
        .iter()
        .position(|argument| argument == "--")
        .expect("--");
    assert_eq!(
        call[after_options + 1..],
        [
            "/bin/sh",
            "-c",
            "unset TMUX TMUX_PANE; exec \"$@\"",
            "sh",
            "claude",
            "--resume"
        ]
    );
    assert!(
        call.windows(2)
            .any(|pair| pair == ["-e", "TORLEK_SESSION=s1"]),
        "{call:?}"
    );
    let working_dir = fake.directory.path().display().to_string();
    assert!(
        call.windows(2)
            .any(|pair| pair == ["-c", working_dir.as_str()]),
        "{call:?}"
    );
}

#[test]
fn sessions_are_targeted_by_exact_name_never_by_prefix() {
    let fake = FakeTmux::printing_version("tmux 3.6");
    let server = fake.open();
    let session = SessionName::new("agent-1").expect("a valid name");

    let _ = server.close_session(&session);

    let call = &fake.calls()[0];
    assert!(
        call.windows(2).any(|pair| pair == ["-t", "=agent-1:"]),
        "{call:?}"
    );
}

#[test]
fn a_session_name_tmux_would_read_as_a_target_is_refused() {
    for name in [
        "",
        "=agent",
        "agent:1",
        "agent.1",
        "-agent",
        "agent 1",
        "agént",
        &"a".repeat(65),
    ] {
        let refused = SessionName::new(name);

        assert!(
            matches!(refused, Err(Error::InvalidArgument { .. })),
            "{name:?}: {refused:?}"
        );
    }
}

#[test]
fn an_environment_name_that_is_not_a_shell_name_is_refused() {
    let fake = FakeTmux::printing_version("tmux 3.6");
    let server = fake.open();
    let mut bad_launch = launch("agent-1", fake.directory.path());
    bad_launch.environment = vec![("A=B".to_string(), "x".into())];

    let error = server.new_session(&bad_launch).expect_err("A=B is no name");

    assert!(matches!(error, Error::InvalidArgument { .. }), "{error:?}");
    assert_eq!(fake.calls(), Vec::<Vec<String>>::new());
}

#[test]
fn a_session_in_a_missing_directory_is_refused_before_tmux_runs() {
    let fake = FakeTmux::printing_version("tmux 3.6");
    let server = fake.open();

    let error = server
        .new_session(&launch(
            "agent-1",
            &fake.directory.path().join("removed-worktree"),
        ))
        .expect_err("the directory does not exist");

    assert!(matches!(error, Error::InvalidArgument { .. }), "{error:?}");
    assert_eq!(fake.calls(), Vec::<Vec<String>>::new());
}

#[test]
fn an_exited_pane_has_no_pid_even_where_tmux_still_prints_the_old_one() {
    // Some tmux releases (CI's Ubuntu tmux; not 3.8) still print the ended process's pid.
    let fake = FakeTmux::printing("tmux 3.6", "%3 4242 1 7");
    let server = fake.open();

    let panes = server
        .panes(&SessionName::new("agent-1").expect("valid"))
        .expect("lists");

    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].pid, None);
    assert_eq!(panes[0].state, PaneState::Exited { status: Some(7) });
}

#[test]
fn the_attach_command_names_the_private_server_and_the_exact_session() {
    let fake = FakeTmux::printing_version("tmux 3.6");
    let server = fake.open();

    let attach = server.attach_command(&SessionName::new("cockpit").unwrap());

    let tmux = fake.directory.path().join("tmux");
    assert_eq!(
        attach,
        [
            tmux.as_os_str(),
            "-L".as_ref(),
            "torlek".as_ref(),
            "attach-session".as_ref(),
            "-t".as_ref(),
            "=cockpit:".as_ref(),
        ]
    );
    assert!(fake.calls().is_empty(), "it runs nothing");
}
