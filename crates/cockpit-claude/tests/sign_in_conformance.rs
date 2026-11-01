//! Installed-harness sign-in conformance: isolated homes and placeholder credentials only.
//! Ignored unless COCKPIT_CLAUDE_PROGRAM names the exact pinned harness.
#![cfg(target_os = "linux")] // Other platforms need an isolated OS credential store too.
mod common;

use cockpit_claude::{CONFORMANCE_VERSION, ClaudeCode};
use cockpit_provider_api::{Provider, SignInStatus};
use std::path::{Path, PathBuf};

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[test]
#[ignore = "needs the installed pinned harness; uses only an isolated home"]
fn sign_in_in_an_isolated_home_follows_the_harness() {
    let installed =
        PathBuf::from(std::env::var_os("COCKPIT_CLAUDE_PROGRAM").expect("harness path"));
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
    // CLAUDE_CONFIG_DIR isolates Claude's own settings and credential files.
    let environment = format!("CLAUDE_CONFIG_DIR={home}/claude");
    let path = wrapper(&environment);
    let adapter = ClaudeCode::discover(&path, None).unwrap();
    assert_eq!(adapter.version(), Some(CONFORMANCE_VERSION));
    assert!(matches!(
        adapter.sign_in().status,
        SignInStatus::NotSignedIn { .. }
    ));
    wrapper(&format!(
        "{environment} ANTHROPIC_API_KEY=sk-cockpit-conformance-placeholder"
    ));
    assert_eq!(adapter.sign_in().status, SignInStatus::SignedIn);
}
