//! #433: the native sandbox enforced on a real host, through the real daemon. The owner's
//! probe runs inside a sandboxed agent: it cannot read a denied path, is refused a host the
//! session does not allow, reaches one it does, and records what it saw on the launch's
//! evidence (`observed: owner probe: ...`). Four launches in a row of one session each do.
//!
//! Ignored by default because it reaches the internet: CI's Linux and WSL 2 jobs run it on
//! their disposable runners with `--run-ignored only`.

mod candidate;
mod real_daemon;

use std::path::Path;
use std::time::{Duration, Instant};

use real_daemon::{COMMANDED_PROJECT, GENEROUS, Home, new_session};

const REQUIRED: &str = "[agents]\nsandbox = \"required\"\n";
/// The claude adapter allows its own API host: what a real agent needs first.
const ALLOWED_HOST: &str = "api.anthropic.com";
/// A real host no adapter, registry or grant allows.
const UNKNOWN_HOST: &str = "example.com";
const LAUNCHES: usize = 4;

/// `curl` through the session proxy the agent's `HTTPS_PROXY` names: the proxy's answer to
/// the tunnel, then the server's, `000` when there was none.
fn fetch_line(host: &str) -> String {
    format!(
        "curl -sS -o /dev/null --max-time 20 \
         -w 'tunnel=%{{http_connect}} status=%{{http_code}}\\n' https://{host}/"
    )
}

/// What `line` printed in the sandboxed agent, ending `exit=<code>`.
async fn run_in_agent(home: &Home, root: &Path, line: &str) -> String {
    let number = home.command_agent(root, line);
    home.agent_output(root, number).await
}

fn printed_value<'a>(printed: &'a str, key: &str) -> &'a str {
    printed
        .split_whitespace()
        .find_map(|word| word.strip_prefix(key))
        .unwrap_or_else(|| panic!("no {key} in {printed}"))
}

/// The launch identity `session sandbox` shows for `session` once it is other than
/// `previous`: the launch now running, which owner observations attach to.
async fn identity_of_a_new_launch(home: &Home, session: &str, previous: Option<&str>) -> String {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let output = home.cockpit(&["session", "sandbox", session], &[]).await;
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();
        let identity = printed
            .lines()
            .find_map(|line| line.strip_prefix("launch identity: "))
            .map(str::to_owned);
        if let Some(identity) = identity.filter(|identity| Some(identity.as_str()) != previous) {
            return identity;
        }
        assert!(
            Instant::now() < deadline,
            "no new sandboxed launch of {session}: {output:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Runs the owner's probes in the agent, asserting each, and returns what they observed.
async fn probe(home: &Home, root: &Path) -> Vec<String> {
    let mut observed = Vec::new();
    for (label, path) in [
        ("~/.ssh/id_ed25519", home.path().join(".ssh/id_ed25519")),
        ("the state database", home.state_dir().join("state.db")),
    ] {
        let printed = run_in_agent(home, root, &format!("cat '{}'", path.display())).await;
        assert!(!printed.contains("owner's key"), "{label}: {printed}");
        assert!(!printed.ends_with("exit=0\n"), "{label}: {printed}");
        let reason = printed
            .lines()
            .next()
            .and_then(|line| line.rsplit(": ").next());
        observed.push(format!(
            "read {label}: refused ({})",
            reason.unwrap_or("no message")
        ));
    }

    let refused = run_in_agent(home, root, &fetch_line(UNKNOWN_HOST)).await;
    assert_eq!(printed_value(&refused, "tunnel="), "403", "{refused}");
    assert_eq!(printed_value(&refused, "status="), "000", "{refused}");
    observed.push(format!(
        "https://{UNKNOWN_HOST}/: the session proxy answered 403, no tunnel"
    ));

    let reached = run_in_agent(home, root, &fetch_line(ALLOWED_HOST)).await;
    assert_eq!(printed_value(&reached, "tunnel="), "200", "{reached}");
    let status = printed_value(&reached, "status=");
    assert_ne!(status, "000", "{reached}");
    observed.push(format!(
        "https://{ALLOWED_HOST}/: tunnel 200, the server answered HTTP {status}"
    ));
    observed
}

/// Records `observed` as the owner's probe of the launch named `identity`, retrying while the
/// daemon answers busy, and returns the evidence it now shows.
async fn record(home: &Home, session: &str, identity: &str, observed: &[String]) -> String {
    let mut arguments = vec!["session", "sandbox-record", session, "--client", identity];
    for observation in observed {
        arguments.extend(["--observed", observation]);
    }
    let deadline = Instant::now() + GENEROUS;
    loop {
        let output = home.cockpit(&arguments, &[]).await;
        let busy = String::from_utf8_lossy(&output.stderr).starts_with("torlek: busy:");
        if !busy || Instant::now() >= deadline {
            assert!(output.status.success(), "{output:?}");
            return String::from_utf8(output.stdout).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The owner's home with a key the agent must not read, and `required` set.
fn prepare(home: &Home) {
    let ssh = home.path().join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    std::fs::write(ssh.join("id_ed25519"), "owner's key\n").unwrap();
    home.write_owner_config(REQUIRED);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live: reaches the internet; CI's Linux and WSL 2 jobs run it (#433)"]
async fn a_sandboxed_launch_records_what_the_owners_probe_observed() {
    let home = Home::new();
    prepare(&home);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    let session = new_session(&client, &root).await.id;
    let identity = identity_of_a_new_launch(&home, &session, None).await;

    let observed = probe(&home, &root).await;
    let evidence = record(&home, &session, &identity, &observed).await;

    println!("{evidence}");
    assert!(
        evidence.starts_with("sandbox: enforced by torlek-sandbox on "),
        "{evidence}"
    );
    for observation in &observed {
        assert!(
            evidence.contains(&format!("\nobserved: owner probe: {observation}\n")),
            "{observation}: {evidence}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live: reaches the internet; CI's Linux and WSL 2 jobs run it (#433)"]
async fn four_launches_in_a_row_of_one_session_each_reach_the_allowed_host() {
    let home = Home::new();
    prepare(&home);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, COMMANDED_PROJECT).await;
    let session = new_session(&client, &root).await.id;
    let mut identities: Vec<String> = Vec::new();

    for launch in 1..=LAUNCHES {
        if launch > 1 {
            // The next launch's agent numbers its lines from 0 again.
            std::fs::remove_dir_all(root.join(".agent")).unwrap();
            let restarted = home.cockpit(&["session", "restart", &session], &[]).await;
            assert!(restarted.status.success(), "{restarted:?}");
        }
        let previous = identities.last().map(String::as_str);
        let identity = identity_of_a_new_launch(&home, &session, previous).await;

        let observed = probe(&home, &root).await;
        let evidence = record(&home, &session, &identity, &observed).await;

        println!("launch {launch} of {LAUNCHES}:\n{evidence}");
        assert!(
            evidence.contains(&format!(
                "\nobserved: owner probe: https://{ALLOWED_HOST}/: tunnel 200"
            )),
            "launch {launch}: {evidence}"
        );
        identities.push(identity);
    }

    let mut distinct = identities.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), LAUNCHES, "{identities:?}");
}
