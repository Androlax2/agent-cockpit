//! #754: a project whose owner keeps a GitHub token gives it to its sessions as `GH_TOKEN`;
//! another project's sessions get no token.
//! Through the real daemon binary and a stand-in Claude Code that logs its GitHub variables.

mod candidate;
mod real_daemon;

use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_protocol::{Command, Reply};
use real_daemon::{
    GENEROUS, Home, ONBOARD_PROJECT, new_session, session_new_command, wait_for_running,
};

const TOKEN: &str = "github_pat_11FIXTURE0abcdefghijklmnopqrstuvwxyz0123456789";
/// What the daemon itself holds: no session may inherit it.
const DAEMON_TOKEN: &str = "ghp_daemonOwnTokenNoSessionInherits0123";

/// Starts a session in project `project` and waits for its agent.
async fn launched_session(home: &Home, client: &Connection, project: &str) -> String {
    let (root, _) = home.project(client, project).await;
    let session = new_session(client, &root).await;
    home.wait_for_launches(&session.id, 1).await;
    wait_for_running(client, &session.id).await;
    session.id
}

/// The `GH_TOKEN` and `GITHUB_TOKEN` session `id`'s agent logged at its start.
async fn logged_github_variables(home: &Home, id: &str) -> (String, String) {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let log = std::fs::read_to_string(home.github_log()).unwrap_or_default();
        let logged = log.lines().find_map(|line| {
            let mut words = line.split(' ');
            (words.next() == Some(id)).then(|| {
                (
                    words.next().unwrap_or_default().to_owned(),
                    words.next().unwrap_or_default().to_owned(),
                )
            })
        });
        if let Some(logged) = logged {
            return logged;
        }
        assert!(
            Instant::now() < deadline,
            "{id}'s agent logged nothing: {log}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_with_a_token_gives_it_to_its_sessions_and_another_projects_get_none() {
    let home = Home::new();
    home.write_github_token("first", &format!("{TOKEN}\n"), 0o600);
    let _daemon = home.start_with(&[("GH_TOKEN", DAEMON_TOKEN), ("GITHUB_TOKEN", DAEMON_TOKEN)]);
    let client = home.connect().await;
    let first = launched_session(&home, &client, "first").await;
    let second = launched_session(&home, &client, "second").await;

    assert_eq!(
        logged_github_variables(&home, &first).await,
        (TOKEN.to_owned(), "unset".to_owned())
    );
    assert_eq!(
        logged_github_variables(&home, &second).await,
        ("unset".to_owned(), "unset".to_owned())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_token_value_reaches_the_logs_or_torlek_output() {
    let home = Home::new();
    home.write_github_token(ONBOARD_PROJECT, TOKEN, 0o600);
    let _daemon = home.start();
    let client = home.connect().await;
    let id = launched_session(&home, &client, ONBOARD_PROJECT).await;
    assert_eq!(logged_github_variables(&home, &id).await.0, TOKEN);
    let brief = wait_for_onboarding(&home).await;
    let shown = home
        .cockpit(&["project", "show", ONBOARD_PROJECT], &[])
        .await;
    let evidence = home.cockpit(&["session", "sandbox", &id], &[]).await;

    let daemon_log =
        std::fs::read_to_string(home.state_dir().join("logs/daemon/cockpit.log")).unwrap();
    let outputs = [
        ("the daemon's log", daemon_log),
        ("the daemon's standard error", home.stderr()),
        ("the brief", brief.clone()),
        (
            "project show",
            String::from_utf8_lossy(&shown.stdout).into_owned(),
        ),
        (
            "session sandbox",
            String::from_utf8_lossy(&evidence.stdout).into_owned(),
        ),
    ];

    for (name, output) in &outputs {
        assert!(!output.contains(TOKEN), "{name} holds the token:\n{output}");
    }
    assert!(
        brief.contains("this project has its own token, set as `GH_TOKEN`"),
        "{brief}"
    );
    assert!(shown.status.success(), "{shown:?}");
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("github-token\tconfigured\t"),
        "{shown:?}"
    );
    assert!(evidence.status.success(), "{evidence:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn project_show_says_a_project_without_a_token_gets_none() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    home.project(&client, "first").await;

    let shown = home.cockpit(&["project", "show", "first"], &[]).await;

    assert!(shown.status.success(), "{shown:?}");
    let stdout = String::from_utf8_lossy(&shown.stdout);
    assert!(
        stdout.contains("github-token\tnone\tsessions get no GH_TOKEN"),
        "{stdout}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_file_other_users_may_read_refuses_the_projects_launch() {
    let home = Home::new();
    home.write_github_token("first", TOKEN, 0o644);
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "first").await;

    let answer = client
        .request(session_new_command(&root), Duration::from_secs(60))
        .await;

    let refusal = format!("{answer:?}");
    assert!(
        !matches!(answer, Ok(Reply::Session { .. })),
        "the launch ran: {refusal}"
    );
    assert!(refusal.contains("chmod 600"), "{refusal}");
    assert!(!refusal.contains(TOKEN), "{refusal}");
    assert!(home.launches().is_empty(), "{:?}", home.launches());
}

/// What the [`ONBOARD_PROJECT`] agent read, once it wrote it.
async fn wait_for_onboarding(home: &Home) -> String {
    let deadline = Instant::now() + GENEROUS;
    loop {
        if let Ok(written) = std::fs::read_to_string(home.onboard_log()) {
            return written;
        }
        assert!(Instant::now() < deadline, "the onboard agent wrote nothing");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_project_list_carries_each_projects_token_state() {
    let home = Home::new();
    home.write_github_token("first", TOKEN, 0o600);
    let _daemon = home.start();
    let client = home.connect().await;
    home.project(&client, "first").await;
    home.project(&client, "second").await;

    let answer = client.request(Command::ProjectList, GENEROUS).await;

    let Ok(Reply::Projects { projects }) = answer else {
        panic!("project-list answered {answer:?}");
    };
    let state = |name: &str| {
        projects
            .iter()
            .find(|project| project.name == name)
            .and_then(|project| project.github_token.clone())
    };
    assert!(
        matches!(
            state("first"),
            Some(cockpit_protocol::GithubTokenView::Configured { .. })
        ),
        "{projects:?}"
    );
    assert!(
        matches!(
            state("second"),
            Some(cockpit_protocol::GithubTokenView::Missing { .. })
        ),
        "{projects:?}"
    );
}
