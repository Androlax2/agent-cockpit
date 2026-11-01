//! Installed-harness sign-in conformance: isolated homes and placeholder credentials only.
//! Ignored unless COCKPIT_CODEX_PROGRAM names the exact pinned harness.
mod common;

use cockpit_codex::{Codex, SIGN_IN_CONFORMANCE_VERSION};
use cockpit_provider_api::{Provider, SignInStatus};
use std::path::{Path, PathBuf};

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[test]
#[ignore = "needs the installed pinned harness; uses only an isolated home"]
fn sign_in_in_an_isolated_home_follows_the_harness() {
    let installed = PathBuf::from(std::env::var_os("COCKPIT_CODEX_PROGRAM").expect("harness path"));
    let dir = tempfile::tempdir().unwrap();
    let home = quoted(dir.path());
    let program = quoted(&installed);
    let wrapper = |extra: &str| {
        common::script(
            dir.path(),
            "probe",
            &format!(
                "cd {home} || exit 1\nexec /usr/bin/env -i PATH=/usr/bin:/bin HOME={home} XDG_CONFIG_HOME={home}/config XDG_DATA_HOME={home}/data XDG_CACHE_HOME={home}/cache XDG_STATE_HOME={home}/state NO_COLOR=1 {extra} {program} \"$@\""
            ),
        )
    };
    // Force file storage inside the fixture; never read the host's keyring.
    std::fs::write(
        dir.path().join("config.toml"),
        "cli_auth_credentials_store = \"file\"\n",
    )
    .unwrap();
    let path = wrapper(&format!("CODEX_HOME={home}"));
    let adapter = Codex::discover(&path, None).unwrap();
    assert_eq!(adapter.version(), Some(SIGN_IN_CONFORMANCE_VERSION));
    assert!(matches!(
        adapter.sign_in().status,
        SignInStatus::NotSignedIn { .. }
    ));
    // The harness itself stores a dummy API key through its documented login interface.
    let mut child = std::process::Command::new(&path)
        .args(["login", "--with-api-key"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"sk-cockpit-conformance-placeholder\n")
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(adapter.sign_in().status, SignInStatus::SignedIn);
}
