//! Owner-measured enforcement stays with the exact launch; a report cannot grant authority.
mod candidate;
mod real_daemon;

use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, ErrorCode, Reply, SandboxEnforcementView};
use real_daemon::{GENEROUS, Home, new_session};

async fn identity(client: &Connection, session: &str) -> String {
    let Reply::SessionSandbox {
        evidence: Some(evidence),
    } = client
        .request(
            Command::SessionSandbox {
                session: session.into(),
            },
            GENEROUS,
        )
        .await
        .unwrap()
    else {
        panic!("missing sandbox evidence");
    };
    let SandboxEnforcementView::Sandboxed {
        client_identity, ..
    } = evidence.enforcement
    else {
        panic!("expected a sandboxed launch");
    };
    client_identity
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_observations_are_persistent_idempotent_and_bound_to_one_launch() {
    if real_daemon::skip_without_pinned_engines() {
        return;
    }
    let home = Home::new();
    let daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "observed-enforcement").await;
    let session = new_session(&client, &root).await;
    home.wait_for_launches(&session.id, 1).await;
    let first = identity(&client, &session.id).await;
    let before = shown(&home, &session.id).await;
    let observations = [
        "fixture private key: read denied",
        "allowed fixture path: read/write succeeded",
    ];

    let output = record(&home, &session.id, &first, &observations).await;
    assert!(output.status.success(), "{output:?}");
    let after = shown(&home, &session.id).await;
    assert!(
        after.starts_with(&before),
        "existing evidence changed: {after}"
    );
    assert_eq!(after.matches("observed: owner probe: ").count(), 2);
    assert!(after.contains("observed: owner probe: fixture private key: read denied\n"));
    assert!(
        record(&home, &session.id, &first, &observations)
            .await
            .status
            .success()
    );
    assert_eq!(shown(&home, &session.id).await, after);
    let conflict = record(&home, &session.id, &first, &["replacement claim"]).await;
    assert_eq!(conflict.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("already recorded"));
    assert_eq!(shown(&home, &session.id).await, after);

    drop(client);
    daemon.kill();
    let _daemon = home.start();
    assert_eq!(shown(&home, &session.id).await, after);
    let client = home.connect().await;
    client
        .request(
            Command::SessionRestart {
                session: session.id.clone(),
            },
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    home.wait_for_launches(&session.id, 2).await;
    let second = identity(&client, &session.id).await;
    assert_ne!(first, second);
    assert!(!shown(&home, &session.id).await.contains("owner probe:"));
    let stale = record(&home, &session.id, &first, &observations).await;
    assert_eq!(stale.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("stale launch"));
    assert!(!shown(&home, &session.id).await.contains("owner probe:"));
    assert!(
        record(&home, &session.id, &second, &observations)
            .await
            .status
            .success()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_or_oversized_observations_never_change_launch_evidence() {
    if real_daemon::skip_without_pinned_engines() {
        return;
    }
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "invalid-observation").await;
    let session = new_session(&client, &root).await;
    let identity = identity(&client, &session.id).await;
    let before = shown(&home, &session.id).await;
    let long = "x".repeat(513);
    let invalid = [
        vec![],
        vec![""],
        vec!["  "],
        vec!["line\nbreak"],
        vec!["escape\u{1b}"],
        vec![long.as_str()],
        vec!["duplicate", "duplicate"],
    ];
    for observations in invalid {
        let output = record(&home, &session.id, &identity, &observations).await;
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("sandbox observations"),
            "{output:?}"
        );
        assert_eq!(shown(&home, &session.id).await, before);
    }
    let many: Vec<String> = (0..17).map(|index| format!("probe {index}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(
        record(&home, &session.id, &identity, &many)
            .await
            .status
            .code(),
        Some(1)
    );
    assert_eq!(shown(&home, &session.id).await, before);
    let boundary = "x".repeat(512);
    let valid: Vec<String> = (0..15)
        .map(|index| format!("probe {index}"))
        .chain([boundary])
        .collect();
    let valid: Vec<&str> = valid.iter().map(String::as_str).collect();
    assert!(
        record(&home, &session.id, &identity, &valid)
            .await
            .status
            .success()
    );
    assert_eq!(
        shown(&home, &session.id)
            .await
            .matches("observed: owner probe: ")
            .count(),
        16
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agent_socket_cannot_record_owner_observations() {
    if real_daemon::skip_without_pinned_engines() {
        return;
    }
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
