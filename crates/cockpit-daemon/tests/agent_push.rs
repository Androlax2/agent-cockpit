//! #155: a session's agent has the daemon push its own checkout with `torlek push`, under the
//! `push-branch` grant its launch gave it over itself. Through the real daemon and CLI, the
//! agent's `torlek` on the agent socket; the daemon's Git reaches a local bare repository
//! in place of GitHub through a wrapper that rewrites the origin for remote calls alone.

mod candidate;
mod real_daemon;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_protocol::{Command, ErrorCode};
use real_daemon::{COMMANDED_PROJECT, Daemon, GENEROUS, Home, cockpit_program, new_session};

const ORIGIN: &str = "https://github.com/owner/commanded.git";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn git(dir: &Path, arguments: &[&str]) -> String {
    let output = std::process::Command::new("/usr/bin/git")
        .current_dir(dir)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        text(&output.stderr)
    );
    text(&output.stdout)
}

/// A folder holding a `git` that runs the host's, with `ORIGIN` rewritten to `remote` for
/// the remote calls alone, as `cockpit-projects`' push tests do.
fn forge_git(home: &Home, remote: &Path) -> PathBuf {
    let folder = home.path().join("forge-bin");
    std::fs::create_dir_all(&folder).unwrap();
    let rewrite = home.path().join("rewrite.gitconfig");
    std::fs::write(
        &rewrite,
        format!("[url \"{}\"]\n\tinsteadOf = {ORIGIN}\n", remote.display()),
    )
    .unwrap();
    let wrapper = folder.join("git");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nfor argument; do\n  case \"$argument\" in\n    \
             push|ls-remote|fetch) export GIT_CONFIG_GLOBAL='{}' GIT_ALLOW_PROTOCOL=file ;;\n  \
             esac\ndone\nexec /usr/bin/git \"$@\"\n",
            rewrite.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    folder
}

/// The commanded project, its origin on GitHub, its `main` pushed by the owner to the bare
/// repository standing for it, the push right bound and a push token saved; and a session in
/// its main checkout.
struct Pushing {
    home: Home,
    _daemon: Daemon,
    owner: cockpit_client::Connection,
    root: PathBuf,
    remote: PathBuf,
    session: String,
}

impl Pushing {
    async fn new() -> Pushing {
        let home = Home::new();
        let remote = home.path().join("remote.git");
        git(
            &home.path(),
            &[
                "init",
                "--quiet",
                "--bare",
                "--initial-branch=main",
                remote.to_str().unwrap(),
            ],
        );
        let daemon = home.start_with_path_first(&[forge_git(&home, &remote)]);
        let owner = home.connect().await;
        let (root, _) = home.project(&owner, COMMANDED_PROJECT).await;
        git(&root, &["remote", "add", "origin", ORIGIN]);
        home.write_github_token(COMMANDED_PROJECT, "push_token_fixture\n", 0o600);
        let pushing = Pushing {
            session: new_session(&owner, &root).await.id,
            home,
            _daemon: daemon,
            owner,
            root,
            remote,
        };
        pushing
            .owner_succeeds(&["network", "grant", COMMANDED_PROJECT, "github.com"])
            .await;
        pushing
            .owner_succeeds(&[
                "network",
                "grant",
                COMMANDED_PROJECT,
                "github.com",
                "--push",
            ])
            .await;
        pushing.owner_succeeds(&["checkout", "push", "1"]).await;
        pushing
    }

    async fn owner_succeeds(&self, arguments: &[&str]) -> String {
        let output = self.home.cockpit(arguments, &[]).await;
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
        text(&output.stdout)
    }

    /// What the agent's shell `line` printed, ending `exit=<code>`.
    async fn agent_runs(&self, line: &str) -> String {
        let number = self.home.command_agent(&self.root, line);
        self.home.agent_output(&self.root, number).await
    }

    fn remote_tip(&self, branch: &str) -> Option<String> {
        let output = std::process::Command::new("/usr/bin/git")
            .arg("--git-dir")
            .arg(&self.remote)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .unwrap();
        output
            .status
            .success()
            .then(|| text(&output.stdout).trim().to_owned())
    }
}

fn torlek(arguments: &str) -> String {
    format!("'{}' {arguments}", cockpit_program().display())
}

/// The agent's commit on a new branch `branch`, then its `torlek push ARGUMENTS`.
fn commit_then_push(branch: &str, arguments: &str) -> String {
    format!(
        "/usr/bin/git switch --quiet --create {branch} && /usr/bin/git -c user.name=agent \
         -c user.email=agent@example.com commit --quiet --allow-empty -m work && {}",
        torlek(&format!("push {arguments}"))
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_pushes_a_branch_its_projects_pattern_names_and_the_push_is_audited() {
    let pushing = Pushing::new().await;
    pushing
        .owner_succeeds(&[
            "project",
            "push-branch",
            "add",
            COMMANDED_PROJECT,
            "agent/*",
        ])
        .await;

    let printed = pushing.agent_runs(&commit_then_push("agent/one", "")).await;

    assert!(printed.ends_with("exit=0\n"), "{printed}");
    assert!(
        printed.contains(&format!("pushed agent/one to {ORIGIN}")),
        "{printed}"
    );
    let pushed = pushing
        .remote_tip("agent/one")
        .expect("the branch is on the remote");
    let grants = pushing
        .owner_succeeds(&["session", "grants", &pushing.session])
        .await;
    assert!(grants.contains("pushed"), "{grants}");
    assert!(grants.contains(&pushed), "{grants}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_push_of_a_branch_no_pattern_names_is_refused_naming_the_owners_command() {
    let pushing = Pushing::new().await;

    let printed = pushing.agent_runs(&commit_then_push("work", "")).await;

    assert!(printed.ends_with("exit=1\n"), "{printed}");
    assert!(
        printed.contains(&format!(
            "`torlek project push-branch add {COMMANDED_PROJECT} PATTERN`"
        )),
        "{printed}"
    );
    assert_eq!(pushing.remote_tip("work"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_push_of_the_remotes_default_branch_is_refused_whatever_the_patterns() {
    let pushing = Pushing::new().await;
    pushing
        .owner_succeeds(&["project", "push-branch", "add", COMMANDED_PROJECT, "*"])
        .await;
    let before = pushing.remote_tip("main");

    let printed = pushing
        .agent_runs(&format!(
            "/usr/bin/git -c user.name=agent -c user.email=agent@example.com commit --quiet \
             --allow-empty -m work && {}",
            torlek("push")
        ))
        .await;

    assert!(printed.ends_with("exit=1\n"), "{printed}");
    assert!(printed.contains("the remote's default branch"), "{printed}");
    assert_eq!(pushing.remote_tip("main"), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_forced_push_from_a_checkout_without_a_launch_branch_is_refused() {
    let pushing = Pushing::new().await;
    pushing
        .owner_succeeds(&[
            "project",
            "push-branch",
            "add",
            COMMANDED_PROJECT,
            "agent/*",
        ])
        .await;

    let printed = pushing
        .agent_runs(&commit_then_push("agent/two", "--force"))
        .await;

    assert!(printed.ends_with("exit=1\n"), "{printed}");
    assert!(printed.contains("no launch branch"), "{printed}");
    assert_eq!(pushing.remote_tip("agent/two"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_push_asked_on_the_owners_socket_is_refused() {
    let pushing = Pushing::new().await;

    let answer = pushing
        .owner
        .request(
            Command::AgentPush {
                launch_token: None,
                force: false,
            },
            GENEROUS,
        )
        .await;

    match answer {
        Err(cockpit_client::ClientError::Daemon(error)) => {
            assert_eq!(error.code, ErrorCode::NotAuthorized, "{error:?}");
            assert!(error.message.contains("a session asks for it"), "{error:?}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}
