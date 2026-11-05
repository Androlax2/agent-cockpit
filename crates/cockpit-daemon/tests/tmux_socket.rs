//! The daemon names its tmux server by path (#845): `tmux -L` takes `$TMUX_TMPDIR` only
//! when that directory exists and otherwise falls back to `/tmp`, where the owner's installed
//! Torlek runs its server. The daemon's `tmux` here is a stand-in that logs its arguments and
//! reports no server, so no test reaches a real one.

mod candidate;
mod common;

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use common::{BUILD_PROFILE_DIR, TestHome};

/// A home whose daemon finds only the logging stand-in for tmux on its `PATH`.
struct LoggedTmux {
    home: TestHome,
    bin: tempfile::TempDir,
}

impl LoggedTmux {
    fn new() -> LoggedTmux {
        let bin = tempfile::tempdir().expect("a directory for the stand-in");
        let program = bin.path().join("tmux");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$*\" >> '{log}'\n\
                 if [ \"$1\" = -V ]; then echo 'tmux 3.6'; exit 0; fi\n\
                 echo 'no server running on the stand-in' >&2\n\
                 exit 1\n",
                log = bin.path().join("calls").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut home = TestHome::new();
        home.path = Some(bin.path().as_os_str().to_owned());
        LoggedTmux { home, bin }
    }

    /// Every argument list the daemon ran tmux with, one line each, `-V` included.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.bin.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn server_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| call != "-V")
            .collect()
    }
}

fn user_dir(tmux_tmpdir: &Path) -> PathBuf {
    let uid = std::fs::metadata(tmux_tmpdir).unwrap().uid();
    tmux_tmpdir.join(format!("tmux-{uid}"))
}

#[test]
fn every_tmux_command_names_the_server_by_its_path_under_tmux_tmpdir() {
    let fixture = LoggedTmux::new();
    let _daemon = fixture.home.start();

    let socket = user_dir(&fixture.home.tmux_tmpdir).join(BUILD_PROFILE_DIR);
    let calls = fixture.server_calls();
    assert!(!calls.is_empty(), "the start queries its server");
    let expected = format!(" -S {} ", socket.display());
    let elsewhere: Vec<&String> = calls
        .iter()
        .filter(|call| !call.contains(&expected))
        .collect();
    assert!(
        elsewhere.is_empty(),
        "not on the home's socket: {elsewhere:#?}"
    );
}

#[test]
fn a_missing_tmux_user_directory_is_created_private() {
    let fixture = LoggedTmux::new();
    let _daemon = fixture.home.start();

    let metadata = std::fs::metadata(user_dir(&fixture.home.tmux_tmpdir)).unwrap();

    assert!(metadata.is_dir());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
}

#[test]
fn a_missing_tmux_tmpdir_is_refused_before_any_server_command() {
    let mut fixture = LoggedTmux::new();
    let missing = fixture.home.home().join("no-such-tmux-dir");
    fixture.home.tmux_tmpdir = missing.clone();

    let refused = fixture.home.run_to_exit();

    assert_eq!(refused.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("TMUX_TMPDIR") && stderr.contains(&missing.display().to_string()),
        "{stderr}"
    );
    assert_eq!(fixture.server_calls(), Vec::<String>::new());
    assert!(!missing.exists(), "the daemon does not create it");
}

/// tmux refuses only a `tmux-<uid>` directory other users can open; a group bit is its own.
#[test]
fn a_tmux_user_directory_with_group_bits_is_accepted_as_tmux_accepts_it() {
    let fixture = LoggedTmux::new();
    let user_dir = user_dir(&fixture.home.tmux_tmpdir);
    std::fs::create_dir(&user_dir).unwrap();
    std::fs::set_permissions(&user_dir, std::fs::Permissions::from_mode(0o750)).unwrap();

    let _daemon = fixture.home.start();

    assert!(
        !fixture.server_calls().is_empty(),
        "the start queries its server"
    );
}

#[test]
fn a_tmux_user_directory_other_users_can_open_is_refused() {
    let fixture = LoggedTmux::new();
    let user_dir = user_dir(&fixture.home.tmux_tmpdir);
    std::fs::create_dir(&user_dir).unwrap();
    std::fs::set_permissions(&user_dir, std::fs::Permissions::from_mode(0o705)).unwrap();

    let refused = fixture.home.run_to_exit();

    assert_eq!(refused.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains(&user_dir.display().to_string()) && stderr.contains("705"),
        "{stderr}"
    );
    assert_eq!(fixture.server_calls(), Vec::<String>::new());
}
