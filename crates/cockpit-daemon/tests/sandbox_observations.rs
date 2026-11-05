//! Owner-measured enforcement attaches only to a sandboxed launch; a report cannot grant
//! authority.
mod candidate;
mod real_daemon;

use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, ErrorCode};
use real_daemon::{GENEROUS, Home, new_session};

async fn record(
    home: &Home,
    session: &str,
    client: &str,
    observations: &[&str],
) -> std::process::Output {
    let mut arguments = vec!["session", "sandbox-record", session, "--client", client];
    for observation in observations {
        arguments.extend(["--observed", observation]);
    }
    let deadline = Instant::now() + GENEROUS;
    loop {
        let output = home.cockpit(&arguments, &[]).await;
        if !String::from_utf8_lossy(&output.stderr).starts_with("torlek: busy:")
            || Instant::now() >= deadline
        {
            return output;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn shown(home: &Home, session: &str) -> String {
    let output = home.cockpit(&["session", "sandbox", session], &[]).await;
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// No release produces a sandboxed launch now (#828), so an owner's measurement has no launch
/// to attach to: it is refused and the evidence stays as the launch recorded it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observations_of_an_unsandboxed_launch_are_refused_and_change_nothing() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "observed-enforcement").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;
    let before = shown(&home, &session.id).await;

    let output = record(&home, &session.id, "any-client", &["fixture: read denied"]).await;

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no sandboxed launch"),
        "{output:?}"
    );
    assert_eq!(shown(&home, &session.id).await, before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_or_oversized_observations_never_change_launch_evidence() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "invalid-observation").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;
    let before = shown(&home, &session.id).await;
    let long = "x".repeat(513);
    let many: Vec<String> = (0..17).map(|index| format!("probe {index}")).collect();
    let invalid = [
        vec![],
        vec![""],
        vec!["  "],
        vec!["line\nbreak"],
        vec!["escape\u{1b}"],
        vec![long.as_str()],
        vec!["duplicate", "duplicate"],
        many.iter().map(String::as_str).collect(),
    ];
    for observations in invalid {
        let output = record(&home, &session.id, "any-client", &observations).await;
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("sandbox observations"),
            "{output:?}"
        );
        assert_eq!(shown(&home, &session.id).await, before);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agent_socket_cannot_record_owner_observations() {
    let home = Home::new();
    let _daemon = home.start();
    let agent = Connection::open(&home.agent_socket_path(), GENEROUS)
        .await
        .unwrap();
    let command: Command = serde_json::from_value(serde_json::json!({
        "kind": "session-sandbox-record", "session": "s1", "client_identity": "not-authority",
        "observations": ["a claim is not authority"]
    }))
    .unwrap();
    let refused = agent.request(command, GENEROUS).await;
    assert!(
        matches!(&refused, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized),
        "{refused:?}"
    );
}
