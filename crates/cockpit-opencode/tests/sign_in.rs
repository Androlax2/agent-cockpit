//! Sign-in through the public provider contract, against isolated harness commands.
mod common;

use cockpit_opencode::OpenCode;
use cockpit_provider_api::{Provider, SignInStatus};

fn probe(body: &str) -> cockpit_provider_api::SignIn {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(
        dir.path(),
        "agent",
        &format!("[ \"$*\" = 'auth list' ] || exit 99\n{body}"),
    );
    OpenCode::new(program, None, None).sign_in()
}

#[test]
fn documented_sign_in_answers_are_reported_without_account_details() {
    let answer = probe(
        r#"printf '\033[90m┌  Credentials /private/path\033[0m\n│  Provider api\n└  1 credentials\n'"#,
    );
    assert!(!format!("{answer:?}").contains("private"));
    assert_eq!(answer.status, SignInStatus::SignedIn);
    let answer = probe(r#"printf '┌  Credentials /private/path\n└  0 credentials\n'"#);
    assert!(!format!("{answer:?}").contains("private"));
    let SignInStatus::NotSignedIn { next } = answer.status else {
        panic!("{answer:?}")
    };
    assert!(next.contains("opencode auth login"));
    let answer = probe(
        r#"printf '└  0 credentials\n┌  Environment\n│  Provider API_KEY\n└  1 environment variable\n'"#,
    );
    assert!(!format!("{answer:?}").contains("private"));
    assert_eq!(answer.status, SignInStatus::SignedIn);
}

#[test]
fn errors_and_unrecognized_output_are_unknown_and_never_echoed() {
    for body in [
        "echo 'private secret' >&2; exit 1",
        "echo 'Usage: private secret'",
        "exit 2",
        "printf '\\377'",
        "echo '└  no credentials'",
        "echo '└  1 credentials'; exit 1",
        "echo '└  0 credentials'; echo '└  nope environment variables'",
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
        let adapter = OpenCode::new(program, None, None);
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
    let answer = OpenCode::new(dir.path().join("missing"), None, None).sign_in();
    assert!(matches!(answer.status, SignInStatus::Unknown { .. }));
}

#[test]
fn only_a_measured_sign_in_version_is_tested() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(dir.path(), "agent", r#"echo '└  1 credentials'"#);
    let pinned = OpenCode::new(
        program.clone(),
        Some(cockpit_opencode::CONFORMANCE_VERSION.to_owned()),
        None,
    )
    .sign_in();
    assert_eq!(
        pinned.provenance,
        cockpit_provider_api::Provenance::Discovered
    );
    let unmeasured = OpenCode::new(program, Some("999.0.0".to_owned()), None).sign_in();
    assert_eq!(
        unmeasured.provenance,
        cockpit_provider_api::Provenance::Discovered
    );
}

#[test]
fn a_timeout_stops_descendants_even_after_the_wrapper_exits() {
    use std::process::Command;
    use std::time::{Duration, Instant};
    for ending in ["wait", "exit 0"] {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("descendant");
        let program = common::script(
            dir.path(),
            "agent",
            &format!("sleep 3 &\necho $! > '{}'\n{ending}", pid_file.display()),
        );
        let adapter = OpenCode::new(program, None, None);
        let answer = adapter.sign_in_within(Duration::from_millis(200));
        assert!(matches!(answer.status, SignInStatus::Unknown { .. }));
        let pid = std::fs::read_to_string(pid_file).unwrap();
        let running = || {
            let state = Command::new("/bin/ps")
                .args(["-o", "stat=", "-p", pid.trim()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&state.stdout);
            !state.trim().is_empty() && !state.trim_start().starts_with('Z')
        };
        let until = Instant::now() + Duration::from_millis(500);
        while running() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        let survived = running();
        // Clean up the fixture even when reproducing the regression.
        if survived {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", pid.trim()])
                .status();
        }
        assert!(
            !survived,
            "the {ending} wrapper left its descendant running"
        );
    }
}
