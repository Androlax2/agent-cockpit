//! An agent's Git signs nothing (#694): a commit made under a launch's environment carries no
//! signature, though the owner's and the checkout's configuration ask for one. Real Git, with a
//! fake signing program that signs anything it is asked to, so a commit it signed would show.

mod common;

use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use cockpit_sandbox::{AgentSandbox, Platform, Sandbox};
use common::{launch_request, missing_backend, native_backend, no_host_paths};

/// A repository whose own configuration signs every commit with `fake-gpg`, which answers as
/// GnuPG does when it signed.
struct SigningRepository {
    _scratch: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
}

impl SigningRepository {
    fn new() -> SigningRepository {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("app");
        let home = scratch.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let signer = scratch.path().join("fake-gpg");
        std::fs::write(
            &signer,
            "#!/bin/sh\ncat > /dev/null\necho '[GNUPG:] SIG_CREATED D 1 8 00 1 X' >&2\n\
             printf -- '-----BEGIN PGP SIGNATURE-----\\nfake\\n-----END PGP SIGNATURE-----\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&signer, std::fs::Permissions::from_mode(0o700)).unwrap();
        let repository = SigningRepository {
            _scratch: scratch,
            root,
            home,
        };
        repository.git(&[], &["init", "--quiet", repository.root.to_str().unwrap()]);
        for (key, value) in [
            ("user.name", "Owner"),
            ("user.email", "owner@example.com"),
            ("user.signingKey", "OWNERKEY"),
            ("commit.gpgSign", "true"),
            ("gpg.program", signer.to_str().unwrap()),
        ] {
            repository.git(
                &[],
                &[
                    "-C",
                    repository.root.to_str().unwrap(),
                    "config",
                    key,
                    value,
                ],
            );
        }
        repository
    }

    /// Runs Git with `environment` alone, but for this repository's home and the host's `PATH`.
    fn git(&self, environment: &[(OsString, OsString)], arguments: &[&str]) {
        let status = Command::new("git")
            .env_clear()
            .envs(environment.iter().cloned())
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?}");
    }

    /// Commits under `environment` and returns the raw commit.
    fn commit(&self, environment: &[(OsString, OsString)]) -> String {
        let root = self.root.to_str().unwrap();
        self.git(
            environment,
            &[
                "-C",
                root,
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "work",
            ],
        );
        let output = Command::new("git")
            .args(["-C", root, "cat-file", "commit", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap()
    }
}

fn is_signed(raw_commit: &str) -> bool {
    raw_commit
        .lines()
        .take_while(|line| !line.is_empty())
        .any(|line| line.starts_with("gpgsig"))
}

/// Without the launch's settings, the repository signs: the tests below would see a signature.
#[test]
fn the_fixture_signs_a_commit_made_outside_a_launch() {
    let repository = SigningRepository::new();

    assert!(is_signed(&repository.commit(&[])));
}

#[test]
fn an_unsandboxed_agents_commit_is_unsigned_though_its_checkout_signs() {
    let repository = SigningRepository::new();
    let request = launch_request("s1", repository.root.to_str().unwrap());
    let prepared = Sandbox::new(Platform::Linux, no_host_paths(), missing_backend())
        .prepare(
            AgentSandbox::Off,
            &request,
            PathBuf::from("/usr/bin/claude"),
            Vec::new(),
        )
        .unwrap();

    assert!(!is_signed(&repository.commit(&prepared.environment)));
}

#[test]
fn a_sandboxed_agents_commit_is_unsigned_though_its_checkout_signs() {
    let repository = SigningRepository::new();
    let request = launch_request("s1", repository.root.to_str().unwrap());
    let prepared = Sandbox::new(Platform::Linux, no_host_paths(), native_backend())
        .prepare(
            AgentSandbox::Required,
            &request,
            PathBuf::from("/usr/bin/claude"),
            Vec::new(),
        )
        .unwrap();
    // The session's own Git configuration file is written at launch; this test has none.
    let environment: Vec<(OsString, OsString)> = prepared
        .environment
        .into_iter()
        .filter(|(name, _)| name != "GIT_CONFIG_GLOBAL")
        .collect();

    assert!(!is_signed(&repository.commit(&environment)));
}

#[test]
fn an_unsandboxed_agents_commit_is_unsigned_though_git_c_asks_to_sign() {
    let repository = SigningRepository::new();
    let mut request = launch_request("s1", repository.root.to_str().unwrap());
    request.environment.push((
        "GIT_CONFIG_PARAMETERS".into(),
        "'commit.gpgsign'='true'".into(),
    ));
    let prepared = Sandbox::new(Platform::Linux, no_host_paths(), missing_backend())
        .prepare(
            AgentSandbox::Off,
            &request,
            PathBuf::from("/usr/bin/claude"),
            Vec::new(),
        )
        .unwrap();

    assert!(!is_signed(&repository.commit(&prepared.environment)));
}

#[test]
fn an_unsandboxed_launch_keeps_the_owners_command_scope_git_settings() {
    let mut request = launch_request("s1", "/home/owner/app");
    request.environment.extend([
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), "core.editor".into()),
        ("GIT_CONFIG_VALUE_0".into(), "vi".into()),
    ]);

    let prepared = Sandbox::new(Platform::Linux, no_host_paths(), missing_backend())
        .prepare(
            AgentSandbox::Off,
            &request,
            PathBuf::from("/usr/bin/claude"),
            Vec::new(),
        )
        .unwrap();

    let value = |name: &str| {
        prepared
            .environment
            .iter()
            .rev()
            .find(|(variable, _)| variable == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(value("GIT_CONFIG_KEY_0"), Some("core.editor".into()));
    assert_eq!(value("GIT_CONFIG_KEY_1"), Some("commit.gpgSign".into()));
    assert_eq!(value("GIT_CONFIG_COUNT"), Some("5".into()));
}
