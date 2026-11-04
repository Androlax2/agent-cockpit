//! #754: a project whose owner keeps a GitHub token gives it to its sessions as `GH_TOKEN`,
//! and hides the owner's own `gh` login from them; another project's sessions get no token.
//! Through the real daemon binary, a stand-in Claude Code that logs its GitHub variables and
//! a stand-in Greywall that logs what it was asked.

mod candidate;
mod real_daemon;

use std::path::PathBuf;
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

/// The words the stand-in Greywall got for session `id`'s launch.
fn sandbox_words(home: &Home, id: &str) -> Vec<String> {
    let log = std::fs::read_to_string(home.sandbox_log()).unwrap();
    log.lines()
        .find(|line| line.contains(&format!("--proxy-user cockpit-{id}-")))
        .unwrap_or_else(|| panic!("no launch of {id} in {log}"))
        .split(' ')
        .map(str::to_owned)
        .collect()
}

/// The settings file Greywall was given for session `id`'s launch.
fn settings_file(home: &Home, id: &str) -> PathBuf {
    let words = sandbox_words(home, id);
    let at = words.iter().position(|word| word == "--settings").unwrap();
    PathBuf::from(&words[at + 1])
}

/// The read denials of session `id`'s launch settings.
fn denied_reads(home: &Home, id: &str) -> Vec<String> {
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(settings_file(home, id)).unwrap()).unwrap();
    settings["filesystem"]["denyRead"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule.as_str().unwrap().to_owned())
        .collect()
}

/// Where the owner's `gh` keeps its login: `~/.config/gh`, and `$XDG_CONFIG_HOME/gh`, which
/// the fixture daemon sets to `<home>/config`.
fn owner_gh_logins(home: &Home) -> [String; 2] {
    [
        home.path().join(".config/gh").display().to_string(),
        home.path().join("config/gh").display().to_string(),
    ]
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
async fn with_a_project_token_the_agent_may_not_read_the_owners_gh_login() {
    let home = Home::new();
    home.write_github_token("first", TOKEN, 0o600);
    let _daemon = home.start();
    let client = home.connect().await;
    let first = launched_session(&home, &client, "first").await;
    let second = launched_session(&home, &client, "second").await;

    let with_token = denied_reads(&home, &first);
    let without_token = denied_reads(&home, &second);

    for login in owner_gh_logins(&home) {
        assert!(with_token.contains(&login), "{login} in {with_token:?}");
        // The owner decided agents may use their login until each project has a token.
        assert!(
            !without_token.contains(&login),
            "{login} in {without_token:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_token_passes_through_the_engine_by_name_alone() {
    let home = Home::new();
    home.write_github_token("first", TOKEN, 0o600);
    let _daemon = home.start();
    let client = home.connect().await;
    let first = launched_session(&home, &client, "first").await;
    let second = launched_session(&home, &client, "second").await;

    let pass_through = |id: &str| {
        sandbox_words(&home, id)
            .windows(2)
            .any(|pair| pair[0] == "--ignore-secret" && pair[1] == "GH_TOKEN")
    };

    assert!(pass_through(&first));
    assert!(!pass_through(&second));
}

#[tokio::test(flavor = "multi_thread")]
async fn no_token_value_reaches_the_logs_the_engines_arguments_or_torlek_output() {
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
        (
            "the engine's arguments",
            std::fs::read_to_string(home.sandbox_log()).unwrap(),
        ),
        (
            "the engine's settings",
            std::fs::read_to_string(settings_file(&home, &id)).unwrap(),
        ),
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
    assert!(
        String::from_utf8_lossy(&evidence.stdout).contains("GH_TOKEN"),
        "{evidence:?}"
    );
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
