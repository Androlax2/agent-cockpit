//! The first-run flow (#100): `cockpit init`, and `cockpit` alone until the setup is done,
//! driven as a process through a clean home, step by step, against a scripted daemon that
//! answers the readiness report, the projects and the sessions.

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

mod common;
mod scripted;

use cockpit_protocol::{
    Command, ErrorCode, LaunchKindView, LaunchView, Outcome, ProjectView, ProtocolError,
    ReadinessStatusView, Reply, RequirementView, SessionStateView, SessionView,
};
use scripted::{scripted_conversation, text};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn daemon_socket(&self) -> PathBuf {
        let path = self.dir.path().join("run/cockpit/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    fn owner_config(&self) -> PathBuf {
        self.dir.path().join(".config/cockpit/config.toml")
    }

    fn write_owner_config(&self, contents: &str) {
        let path = self.owner_config();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn accept_sandbox(&self) {
        self.write_owner_config("[agents]\nsandbox = \"required\"\n");
    }

    /// `cockpit` with `arguments` in this home, off the test's runtime threads.
    async fn cockpit(&self, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"));
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }

    /// Runs `cockpit <arguments>` against a daemon that answers with `daemon`'s facts.
    async fn against(&self, daemon: &Daemon, arguments: &[&str]) -> Output {
        let conversation = scripted_conversation(&self.daemon_socket(), daemon.outcomes());
        let output = self.cockpit(arguments).await;
        assert_eq!(
            conversation.await.unwrap(),
            vec![
                Command::Readiness { project: None },
                Command::ProjectList,
                Command::SessionList { ended: true },
            ]
        );
        output
    }
}

/// What the scripted daemon reports.
struct Daemon {
    requirements: Vec<RequirementView>,
    projects: Vec<ProjectView>,
    sessions: Vec<SessionView>,
}

impl Daemon {
    /// A host where nothing blocks, with Claude Code installed and its sign-in unknown (#443).
    fn ready() -> Daemon {
        Daemon {
            requirements: vec![
                available("tmux", "tmux 3.5a"),
                available("sandbox", "greywall 0.4"),
                available("provider claude", "2.1 (tested)"),
                requirement(
                    "provider claude sign-in",
                    ReadinessStatusView::Unknown,
                    "start claude once in a terminal and sign in if it asks",
                ),
                available("resource controls", "cpu enforced"),
            ],
            projects: Vec::new(),
            sessions: Vec::new(),
        }
    }

    fn with_project(mut self) -> Daemon {
        self.projects.push(ProjectView {
            name: "app".to_owned(),
            root: "/home/me/code/my app".to_owned(),
        });
        self
    }

    fn with_session(mut self) -> Daemon {
        self.sessions.push(session());
        self
    }

    fn outcomes(&self) -> Vec<Outcome> {
        vec![
            Outcome::Ok(Reply::Readiness {
                requirements: self.requirements.clone(),
            }),
            Outcome::Ok(Reply::Projects {
                projects: self.projects.clone(),
            }),
            Outcome::Ok(Reply::Sessions {
                sessions: self.sessions.clone(),
            }),
        ]
    }

    fn replace(mut self, replacement: RequirementView) -> Daemon {
        let slot = self
            .requirements
            .iter_mut()
            .find(|requirement| requirement.name == replacement.name)
            .expect("the requirement is reported");
        *slot = replacement;
        self
    }
}

fn available(name: &str, detail: &str) -> RequirementView {
    RequirementView {
        name: name.to_owned(),
        status: ReadinessStatusView::Available,
        detail: detail.to_owned(),
        next: None,
    }
}

fn requirement(name: &str, status: ReadinessStatusView, next: &str) -> RequirementView {
    RequirementView {
        name: name.to_owned(),
        status,
        detail: format!("{name} is not available"),
        next: Some(next.to_owned()),
    }
}

fn session() -> SessionView {
    SessionView {
        id: "s0123456789ab".to_owned(),
        agent: "claude".to_owned(),
        checkout: 1,
        holds_checkout: false,
        working_dir: "/home/me/code/my app".to_owned(),
        conversation: None,
        design_link: None,
        state: SessionStateView::Ended,
        reason: None,
        revision: 1,
        created_at: 1,
        changed_at: 1,
        launch: Some(LaunchView {
            number: 1,
            kind: LaunchKindView::Start,
            launched_at: 1,
        }),
        changed_start_files: Vec::new(),
        restart_pending: false,
        finished: true,
    }
}

/// The step's header line, `[state] n. title`.
fn step<'checklist>(checklist: &'checklist str, title: &str) -> &'checklist str {
    checklist
        .lines()
        .find(|line| line.ends_with(&format!(". {title}")))
        .unwrap_or_else(|| panic!("no step {title} in:\n{checklist}"))
}

/// The checklist's last line: the one action to take now.
fn final_next(checklist: &str) -> &str {
    checklist.lines().last().expect("a checklist")
}

#[tokio::test]
async fn on_a_clean_home_without_a_daemon_cockpit_alone_starts_with_the_daemon() {
    let home = Home::new();

    let output = home.cockpit(&[]).await;

    let checklist = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(
        checklist.starts_with("Cockpit setup: 0 of 6 steps done"),
        "{checklist}"
    );
    assert!(
        step(&checklist, "Host readiness").starts_with("[todo"),
        "{checklist}"
    );
    for later in ["Pick a provider", "Add a project", "Start a session"] {
        assert!(step(&checklist, later).starts_with("[waits"), "{checklist}");
    }
    assert!(
        final_next(&checklist).starts_with("next: `cockpit daemon install`"),
        "{checklist}"
    );
}

#[test]
fn without_a_home_cockpit_alone_names_the_configuration_error_first() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"))
        .env_clear()
        .output()
        .unwrap();

    let checklist = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(checklist.contains("config: "), "{checklist}");
    assert!(
        final_next(&checklist).starts_with("next: fix what is named here"),
        "{checklist}"
    );
}

#[tokio::test]
async fn a_blocking_host_requirement_is_the_first_action() {
    let home = Home::new();
    let daemon = Daemon::ready().replace(requirement(
        "tmux",
        ReadinessStatusView::MissingPrerequisite,
        "install tmux 3.2 or later",
    ));

    let output = home.against(&daemon, &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        step(&checklist, "Host readiness").starts_with("[todo"),
        "{checklist}"
    );
    assert!(checklist.contains("tmux: tmux is not available (missing-prerequisite)"));
    assert!(
        final_next(&checklist).starts_with("next: install tmux 3.2 or later"),
        "{checklist}"
    );
}

#[tokio::test]
async fn the_sandbox_step_explains_restrictions_and_approvals_and_asks_for_consent() {
    let home = Home::new();

    let output = home.against(&Daemon::ready(), &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Sandbox and approvals").starts_with("[todo"),
        "{checklist}"
    );
    for topic in ["restricted: ", "needs you: ", "approvals: ", "desktop: "] {
        assert!(checklist.contains(topic), "no {topic:?} in:\n{checklist}");
    }
    let next = final_next(&checklist);
    assert!(next.contains("sandbox = \"required\""), "{next}");
    assert!(
        next.contains(&home.owner_config().display().to_string()),
        "{next}"
    );
}

#[tokio::test]
async fn the_owners_sandbox_setting_is_the_recorded_consent() {
    let home = Home::new();
    home.accept_sandbox();

    let output = home.against(&Daemon::ready(), &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Sandbox and approvals").starts_with("[done"),
        "{checklist}"
    );
    assert!(checklist.contains("you chose the sandbox"), "{checklist}");
}

#[tokio::test]
async fn an_unsandboxed_choice_is_named_loudly() {
    let home = Home::new();
    home.write_owner_config("[agents]\nsandbox = \"off\"\n");

    let output = home.against(&Daemon::ready(), &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Sandbox and approvals").starts_with("[done"),
        "{checklist}"
    );
    assert!(checklist.contains("UNSANDBOXED"), "{checklist}");
}

#[tokio::test]
async fn an_unknown_sign_in_names_how_to_check_without_holding_the_setup() {
    let home = Home::new();
    home.accept_sandbox();

    let output = home.against(&Daemon::ready(), &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Pick a provider").starts_with("[done"),
        "{checklist}"
    );
    assert!(
        checklist.contains("sessions start with claude"),
        "{checklist}"
    );
    assert!(
        step(&checklist, "Sign in to the provider").starts_with("[unknown"),
        "{checklist}"
    );
    assert!(
        checklist.contains("next: start claude once in a terminal and sign in if it asks"),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).starts_with("next: `cockpit project add DIR`"),
        "{checklist}"
    );
}

#[tokio::test]
async fn a_sign_in_the_provider_reports_missing_is_an_open_step() {
    let home = Home::new();
    home.accept_sandbox();
    let daemon = Daemon::ready().replace(requirement(
        "provider claude sign-in",
        ReadinessStatusView::NeedsAuthorization,
        "run `claude /login`",
    ));

    let output = home.against(&daemon, &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Sign in to the provider").starts_with("[todo"),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).starts_with("next: run `claude /login`"),
        "{checklist}"
    );
}

#[tokio::test]
async fn one_signed_in_provider_completes_the_sign_in_step() {
    let home = Home::new();
    home.accept_sandbox();
    let mut daemon = Daemon::ready()
        .replace(available("provider claude sign-in", "signed in"))
        .with_project()
        .with_session();
    daemon
        .requirements
        .push(available("provider codex", "0.5 (tested)"));
    daemon.requirements.push(requirement(
        "provider codex sign-in",
        ReadinessStatusView::NeedsAuthorization,
        "run `codex login`",
    ));

    let output = home.against(&daemon, &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Sign in to the provider").starts_with("[done"),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).starts_with("Cockpit is set up"),
        "{checklist}"
    );
}

#[tokio::test]
async fn a_daemon_that_answers_with_an_error_is_a_step_not_a_crash() {
    let home = Home::new();
    let conversation = scripted_conversation(
        &home.daemon_socket(),
        vec![Outcome::Error(ProtocolError::new(
            ErrorCode::Busy,
            "saturated",
        ))],
    );

    let output = home.cockpit(&[]).await;
    conversation.await.unwrap();

    let checklist = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(
        checklist.contains("daemon: running, but it failed to answer"),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).starts_with("next: read the daemon's log"),
        "{checklist}"
    );
}

#[tokio::test]
async fn without_a_provider_the_provider_step_names_its_install() {
    let home = Home::new();
    home.accept_sandbox();
    let daemon = Daemon::ready().replace(requirement(
        "provider claude",
        ReadinessStatusView::MissingPrerequisite,
        "install claude on the daemon's PATH",
    ));

    let output = home.against(&daemon, &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Host readiness").starts_with("[done"),
        "{checklist}"
    );
    assert!(
        step(&checklist, "Pick a provider").starts_with("[todo"),
        "{checklist}"
    );
    assert!(
        step(&checklist, "Sign in to the provider").starts_with("[waits"),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).starts_with("next: install claude on the daemon's PATH"),
        "{checklist}"
    );
}

#[tokio::test]
async fn with_a_project_the_next_action_starts_a_session_there() {
    let home = Home::new();
    home.accept_sandbox();

    let output = home
        .against(&Daemon::ready().with_project(), &["init"])
        .await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Add a project").starts_with("[done"),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).starts_with("next: `cockpit session new '/home/me/code/my app'`"),
        "{checklist}"
    );
}

#[tokio::test]
async fn with_several_providers_the_session_action_picks_one_with_agent() {
    let home = Home::new();
    home.accept_sandbox();
    let mut daemon = Daemon::ready().with_project();
    daemon
        .requirements
        .push(available("provider codex", "0.5 (tested)"));

    let output = home.against(&daemon, &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        checklist.contains("available: claude, codex; each session picks one with `--agent NAME`"),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).contains("--agent claude"),
        "{checklist}"
    );
}

#[tokio::test]
async fn once_a_session_started_init_says_the_setup_is_done() {
    let home = Home::new();
    home.accept_sandbox();

    let output = home
        .against(&Daemon::ready().with_project().with_session(), &["init"])
        .await;

    let checklist = text(&output.stdout);
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        checklist.starts_with("Cockpit setup: 5 of 6 steps done"),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).starts_with("Cockpit is set up"),
        "{checklist}"
    );
}

#[tokio::test]
async fn once_the_setup_is_done_cockpit_alone_prints_the_usage() {
    let home = Home::new();
    home.accept_sandbox();

    let output = home
        .against(&Daemon::ready().with_project().with_session(), &[])
        .await;

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stdout).is_empty(), "{}", text(&output.stdout));
    let usage = text(&output.stderr);
    assert!(usage.starts_with("usage: cockpit "), "{usage}");
    assert!(usage.contains("\n       cockpit init\n"), "{usage}");
}

#[tokio::test]
async fn init_takes_no_arguments() {
    let home = Home::new();

    let output = home.cockpit(&["init", "--yes"]).await;

    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output.stderr).contains("init takes no arguments"));
}

/// The real daemon serves every request the checklist makes, and a project it registers
/// moves the flow on.
#[test]
fn against_the_real_daemon_adding_a_project_completes_that_step() {
    let home = common::TestHome::new();
    let _daemon = home.start();
    let project = home.project("app");

    let before = home.cockpit_ok(&["init"]);
    home.cockpit_ok(&["project", "add", project.to_str().unwrap()]);
    let after = home.cockpit_ok(&["init"]);

    assert!(
        step(&before, "Add a project").starts_with("[todo"),
        "{before}"
    );
    assert!(
        step(&after, "Add a project").starts_with("[done"),
        "{after}"
    );
    assert!(after.contains(project.to_str().unwrap()), "{after}");
}
