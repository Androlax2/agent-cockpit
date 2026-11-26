//! Select the verified release archive for CI fixtures, or the gate's build copied where a
//! second Unix user can run it (#628); ordinary tests use Cargo's build.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn program(name: &str) -> PathBuf {
    match std::env::var_os("COCKPIT_CANDIDATE_ROOT") {
        Some(root) => {
            let root = PathBuf::from(root);
            assert!(root.is_absolute(), "the candidate root must be absolute");
            root.join("bin").join(name)
        }
        None => Path::new(env!("CARGO_BIN_EXE_torlek-daemon")).with_file_name(name),
    }
}

pub fn configure(command: &mut Command) {
    if std::env::var_os("COCKPIT_CANDIDATE_ROOT").is_some() {
        // A release archive normally selects its channel; fixtures own the isolated dev home.
        command.env("TORLEK_PROFILE", "dev");
    }
}

pub fn assert_running_candidate(pid: u32) {
    #[cfg(target_os = "linux")]
    if std::env::var_os("COCKPIT_CANDIDATE_ROOT").is_some() {
        assert_eq!(
            std::fs::read_link(format!("/proc/{pid}/exe")).expect("the running daemon executable"),
            program("torlek-daemon")
                .canonicalize()
                .expect("the archived daemon"),
            "the fixture must exercise the archived daemon, not a fresh Cargo build"
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
}

/// Gives the owner's `config.toml` at `path` the owner's named opt-out `agents.confined_runs
/// = "off"` (#1387), unless it sets that key itself: a daemon in a test home reaches no
/// systemd user manager, so it would refuse every approved run. Tests of confined runs set
/// the key.
pub fn unconfine_runs(path: &Path) {
    add_agents_opt_out(path, "confined_runs", "confined_runs = \"off\"\n");
}

/// Gives the owner's `config.toml` at `path` the owner's rollback `agents.sandbox = "off"`
/// (#833), unless it sets that key itself: the fake agents a test home launches have no
/// sandbox profile, and a test host may lack bubblewrap. Tests of the sandbox, or of its
/// product default, set the key or keep no owner config.
pub fn unsandbox_agents(path: &Path) {
    add_agents_opt_out(path, "sandbox", "sandbox = \"off\"\n");
}

fn add_agents_opt_out(path: &Path, key: &str, opt_out: &str) {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => panic!("{}: {error}", path.display()),
    };
    let dotted = format!("agents.{key}");
    let sets_key = contents.lines().any(|line| {
        let line = line.trim_start();
        [key, dotted.as_str()].into_iter().any(|name| {
            line.strip_prefix(name)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
    });
    if sets_key {
        return;
    }
    let contents = match contents.find("[agents]\n") {
        Some(at) => {
            let (before, after) = contents.split_at(at + "[agents]\n".len());
            format!("{before}{opt_out}{after}")
        }
        None => format!("{contents}\n[agents]\n{opt_out}"),
    };
    std::fs::create_dir_all(path.parent().expect("a config file has a folder")).unwrap();
    std::fs::write(path, contents).unwrap();
}
