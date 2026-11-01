//! Installed-harness sign-in conformance: isolated homes and placeholder credentials only.
//! Ignored unless COCKPIT_OPENCODE_PROGRAM names the exact pinned harness.
mod common;

use cockpit_opencode::{CONFORMANCE_VERSION, OpenCode};
use cockpit_provider_api::{Provider, SignInStatus};
use std::path::{Path, PathBuf};

fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

#[test]
#[ignore = "needs the installed pinned harness; uses only an isolated home"]
fn sign_in_in_an_isolated_home_follows_the_harness() {
    let installed =
        PathBuf::from(std::env::var_os("COCKPIT_OPENCODE_PROGRAM").expect("harness path"));
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
    let path = wrapper("OPENCODE_DISABLE_MODELS_FETCH=true");
    let adapter = OpenCode::discover(&path, None).unwrap();
    assert_eq!(adapter.version(), Some(CONFORMANCE_VERSION));
    assert!(matches!(
        adapter.sign_in().status,
        SignInStatus::NotSignedIn { .. }
    ));
    wrapper(
        "OPENCODE_DISABLE_MODELS_FETCH=true ANTHROPIC_API_KEY=sk-cockpit-conformance-placeholder",
    );
    assert_eq!(adapter.sign_in().status, SignInStatus::SignedIn);
}
