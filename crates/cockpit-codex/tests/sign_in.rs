//! Sign-in through the public provider contract, against isolated harness commands.
mod common;

use cockpit_codex::Codex;
use cockpit_provider_api::{Provider, SignInStatus};

fn probe(body: &str) -> cockpit_provider_api::SignIn {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(
        dir.path(),
        "agent",
        &format!("[ \"$*\" = 'login status' ] || exit 99\n{body}"),
    );
    Codex::new(program, None, None).sign_in()
}

#[test]
fn documented_sign_in_answers_are_reported_without_account_details() {
    let answer = probe(
        "printf 'API key configured - sk-private\\nAuthentication is checked when you send a request.\\n' >&2",
    );
    assert_eq!(answer.status, SignInStatus::SignedIn);
    assert!(!format!("{answer:?}").contains("private"));
    let answer = probe(r#"echo 'Logged in using an API key - sk-private' >&2"#);
    assert!(!format!("{answer:?}").contains("private"));
    assert_eq!(answer.status, SignInStatus::SignedIn);
    let answer = probe(r#"printf 'WARNING: test warning\nNot logged in\n' >&2; exit 1"#);
    assert!(!format!("{answer:?}").contains("private"));
    let SignInStatus::NotSignedIn { next } = answer.status else {
        panic!("{answer:?}")
    };
    assert!(next.contains("codex login"));
}

#[test]
fn errors_and_unrecognized_output_are_unknown_and_never_echoed() {
    for body in [
        "echo 'private secret' >&2; exit 1",
        "echo 'Usage: private secret'",
        "exit 2",
        "printf '\\377'",
    ] {
        let answer = probe(body);
        let SignInStatus::Unknown { reason, next } = answer.status else {
            panic!("{answer:?} for {body}")
        };
        assert!(!reason.is_empty());
        assert!(!next.is_empty());
        assert!(!reason.contains("private"));
    }
}

#[test]
fn stalled_large_or_inherited_output_is_bounded() {
    use std::time::{Duration, Instant};
    for body in [
        "exec sleep 2",
        "head -c 65537 /dev/zero",
        "sleep 0.5 & exit 0",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let program = common::script(dir.path(), "agent", body);
        let adapter = Codex::new(program, None, None);
        let started = Instant::now();
        let answer = adapter.sign_in_within(Duration::from_millis(100));
        assert!(
            matches!(answer.status, SignInStatus::Unknown { .. }),
            "{answer:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

#[test]
fn a_missing_program_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let answer = Codex::new(dir.path().join("missing"), None, None).sign_in();
    assert!(matches!(answer.status, SignInStatus::Unknown { .. }));
}

#[test]
fn only_a_measured_sign_in_version_is_tested() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(dir.path(), "agent", r#"echo 'Logged in using ChatGPT' >&2"#);
    let pinned = Codex::new(
        program.clone(),
        Some(cockpit_codex::SIGN_IN_CONFORMANCE_VERSION.to_owned()),
        None,
    )
    .sign_in();
    assert_eq!(
        pinned.provenance,
        cockpit_provider_api::Provenance::Tested {
            version: cockpit_codex::SIGN_IN_CONFORMANCE_VERSION.to_owned()
        }
    );
    let unmeasured = Codex::new(program, Some("999.0.0".to_owned()), None).sign_in();
    assert_eq!(
        unmeasured.provenance,
        cockpit_provider_api::Provenance::Discovered
    );
}
