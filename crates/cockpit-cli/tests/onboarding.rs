//! The first-run flow (#100): `cockpit init`, and `cockpit` alone until the setup is done,
//! driven as a process through a clean home, step by step, against a scripted daemon that
//! answers the readiness report, the projects, the sessions and the network grants.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::process::Output;

mod common;
mod scripted;

use cockpit_protocol::{
    Command, DecidedThroughView, ErrorCode, LaunchKindView, LaunchView, NetworkDecisionView,
    NetworkGrantView, NetworkTargetView, Outcome, ProjectView, ProtocolError, ReadinessStatusView,
    Reply, RequirementView, SessionStateView, SessionView,
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
        let path = self.dir.path().join("run/torlek-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    fn owner_config(&self) -> PathBuf {
        self.dir.path().join(".config/torlek-dev/config.toml")
    }

    fn write_owner_config(&self, contents: &str) {
        let path = self.owner_config();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// The consent that lets agents launch unsandboxed (#828).
    fn accept_sandbox(&self) {
        self.write_owner_config("[agents]\nsandbox = \"off\"\n");
    }

    /// `cockpit` with `arguments` in this home, off the test's runtime threads.
    async fn cockpit(&self, arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"));
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
        let conversation = finished(conversation, &output).await;
        let mut expected = vec![
            // `torlek` alone needs one signed-in provider, not each one's answer (#1048).
            Command::Readiness {
                project: None,
                is_one_sign_in_enough: arguments.is_empty(),
            },
            Command::ProjectList,
            Command::SessionList { ended: true },
            Command::NetworkGrants { project: None },
        ];
        // The launch gates of the session the checklist offers (#1250).
        if let (Some(project), Some(agent)) = (daemon.projects.first(), daemon.offered_agent()) {
            expected.push(Command::SandboxShow {
                dir: project.root.clone(),
                agent: Some(agent),
            });
        }
        assert_eq!(conversation, expected);
        output
    }
}

/// The scripted daemon's received commands once `cockpit` exited; a panic instead of a hang
/// when `cockpit` never connected (a socket path the test does not share with it).
async fn finished<T: Send + 'static>(
    conversation: tokio::task::JoinHandle<T>,
    output: &Output,
) -> T {
    if !conversation.is_finished() {
        tokio::task::spawn_blocking(|| std::thread::sleep(std::time::Duration::from_secs(2)))
            .await
            .unwrap();
    }
    if !conversation.is_finished() {
        conversation.abort();
        panic!(
            "cockpit never reached the scripted daemon: stdout {}, stderr {}",
            text(&output.stdout),
            text(&output.stderr)
        );
    }
    conversation.await.unwrap()
}

/// What the scripted daemon reports.
struct Daemon {
    requirements: Vec<RequirementView>,
    projects: Vec<ProjectView>,
    sessions: Vec<SessionView>,
    grants: Vec<NetworkGrantView>,
    /// What `sandbox show` answers for the first project.
    launch_gates: Vec<RequirementView>,
}

impl Daemon {
    /// A host where nothing blocks, with Claude Code installed and its sign-in unknown (#443).
    fn ready() -> Daemon {
        Daemon {
            requirements: vec![
                available("tmux", "tmux 3.6a"),
                available("sandbox", "no sandbox: agents run UNSANDBOXED"),
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
            grants: Vec::new(),
            launch_gates: vec![available(
                "launch backend",
                "none: agents.sandbox = \"off\"",
            )],
        }
    }

    fn with_project(mut self) -> Daemon {
        self.projects.push(ProjectView {
            name: "app".to_owned(),
            root: "/home/me/code/my app".to_owned(),
            github_token: None,
        });
        self
    }

    /// The `app` project's github.com grant, made without a request (#1256).
    fn with_github_grant(mut self) -> Daemon {
        self.grants.push(NetworkGrantView {
            id: 1,
            project: "app".to_owned(),
            allows: NetworkTargetView::Host {
                host: "github.com".to_owned(),
                port: 443,
            },
            target: "github.com:443".to_owned(),
            request: None,
            granted: NetworkDecisionView {
                at: 1_000,
                through: Some(DecidedThroughView::Cli),
            },
            revoked: None,
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
            Outcome::Ok(Reply::NetworkGrants {
                grants: self.grants.clone(),
            }),
            Outcome::Ok(Reply::SandboxProfile {
                gates: self.launch_gates.clone(),
                evidence: None,
                command: Vec::new(),
            }),
        ]
    }

    /// The first provider readiness lists as available with a sign-in row: the one the
    /// session step offers.
    fn offered_agent(&self) -> Option<String> {
        self.requirements.iter().find_map(|requirement| {
            let name = requirement.name.strip_prefix("provider ")?;
            let has_sign_in = self
                .requirements
                .iter()
                .any(|other| other.name == format!("provider {name} sign-in"));
            (requirement.status == ReadinessStatusView::Available && has_sign_in)
                .then(|| name.to_owned())
        })
    }

    fn with_launch_gate(mut self, gate: RequirementView) -> Daemon {
        self.launch_gates.push(gate);
        self
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
        model: None,
        effort: None,
        budget_minutes: None,
        budget_spent: false,
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
            account: None,
            switched_from: None,
            passed_over: Vec::new(),
            harness_trust: None,
        }),
        exit: None,
        changed_start_files: Vec::new(),
        restart_pending: false,
        finished: true,
        coordinator_grants: Vec::new(),
        inbound_message: None,
        compaction_threshold: None,
        sign_in_needed: None,
        quota_stop: None,
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
        checklist.starts_with("Torlek setup: 0 of 6 steps done"),
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
        final_next(&checklist).starts_with("next: `torlek daemon install`"),
        "{checklist}"
    );
}

#[test]
fn without_a_home_cockpit_alone_names_the_configuration_error_first() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"))
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
        "install tmux 3.6 or later",
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
        final_next(&checklist).starts_with("next: install tmux 3.6 or later"),
        "{checklist}"
    );
}

#[tokio::test]
async fn the_sandbox_step_says_what_each_setting_does_and_asks_for_consent() {
    let home = Home::new();

    let output = home.against(&Daemon::ready(), &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Sandbox and approvals").starts_with("[todo"),
        "{checklist}"
    );
    for topic in [
        "sandbox: agents.sandbox = ",
        "needs you: ",
        "approvals: ",
        "desktop: ",
    ] {
        assert!(checklist.contains(topic), "no {topic:?} in:\n{checklist}");
    }
    let next = final_next(&checklist);
    assert!(next.contains("sandbox = \"off\""), "{next}");
    assert!(
        next.contains(&home.owner_config().display().to_string()),
        "{next}"
    );
}

/// #625: on a fresh profile the config folder does not exist, so the action creates it first.
#[tokio::test]
async fn on_a_fresh_profile_the_sandbox_action_creates_the_config_folder_first() {
    let home = Home::new();
    let folder = home.owner_config().parent().unwrap().to_owned();
    assert!(!folder.exists());

    let output = home.against(&Daemon::ready(), &["init"]).await;

    let checklist = text(&output.stdout);
    let next = final_next(&checklist);
    let create = next
        .split('`')
        .find(|part| part.starts_with("mkdir "))
        .unwrap_or_else(|| panic!("no mkdir command in: {next}"));
    let created = std::process::Command::new("sh")
        .args(["-c", create])
        .status()
        .unwrap();
    assert!(created.success(), "{create}");
    let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700, "{create}");
}

#[tokio::test]
async fn an_existing_config_folder_is_not_created_again() {
    let home = Home::new();
    std::fs::create_dir_all(home.owner_config().parent().unwrap()).unwrap();

    let output = home.against(&Daemon::ready(), &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(!final_next(&checklist).contains("mkdir"), "{checklist}");
}

/// #829: `required` is the owner's recorded choice; whether this host has the sandbox is
/// the readiness step's to say.
#[tokio::test]
async fn a_required_setting_completes_the_step_and_points_at_readiness() {
    let home = Home::new();
    home.write_owner_config("[agents]\nsandbox = \"required\"\n");

    let output = home.against(&Daemon::ready(), &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Sandbox and approvals").starts_with("[done"),
        "{checklist}"
    );
    assert!(
        checklist.contains("the readiness step's `sandbox` line says whether this host has it"),
        "{checklist}"
    );
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
    assert!(
        checklist.contains(
            "an unsandboxed agent can reach the owner socket and act as the owner, so Torlek's \
             grants only hold under the sandbox"
        ),
        "{checklist}"
    );
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
        final_next(&checklist).starts_with("next: `torlek project add DIR`"),
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
        final_next(&checklist).starts_with("Torlek is set up"),
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
    finished(conversation, &output).await;

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
        final_next(&checklist).starts_with("next: `torlek session new '/home/me/code/my app'`"),
        "{checklist}"
    );
}

const REQUIRED: &str = "[agents]\nsandbox = \"required\"\n";

/// #1256: a project without the github.com grant is offered it as a line under the done
/// step, so declining it keeps the setup complete.
#[tokio::test]
async fn a_project_without_the_github_grant_is_offered_it_without_holding_the_setup() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);

    let output = home
        .against(&Daemon::ready().with_project().with_session(), &["init"])
        .await;

    let checklist = text(&output.stdout);
    assert!(
        checklist.contains(
            "app: its sandboxed sessions reach github.com only after you allow a network \
             request, then a relaunch; `torlek network grant app github.com` allows it always"
        ),
        "{checklist}"
    );
    assert!(
        checklist.contains("Torlek is set up: `torlek tui` opens the manager"),
        "{checklist}"
    );
}

#[tokio::test]
async fn under_an_unsandboxed_setting_no_github_grant_is_offered() {
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
    assert!(!checklist.contains("network grant"), "{checklist}");
}

#[tokio::test]
async fn a_project_holding_the_github_grant_is_not_offered_it_again() {
    let home = Home::new();
    home.write_owner_config(REQUIRED);

    let output = home
        .against(
            &Daemon::ready().with_project().with_github_grant(),
            &["init"],
        )
        .await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Add a project").starts_with("[done"),
        "{checklist}"
    );
    assert!(!checklist.contains("network grant"), "{checklist}");
}

#[tokio::test]
async fn with_several_providers_the_session_action_picks_one_with_agent() {
    let home = Home::new();
    home.accept_sandbox();
    let mut daemon = Daemon::ready().with_project();
    daemon
        .requirements
        .push(available("provider codex", "0.5 (tested)"));
    daemon
        .requirements
        .push(available("provider codex sign-in", "signed in"));

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

/// #624: readiness reports an unusable provider as `available` with an action, so that it
/// does not block; the provider step names it with its reason instead of offering it.
#[tokio::test]
async fn a_provider_readiness_reports_not_usable_is_named_with_its_reason_not_offered() {
    let home = Home::new();
    home.accept_sandbox();
    let mut daemon = Daemon::ready().with_project();
    daemon.requirements.push(RequirementView {
        next: Some("to use it, install codex on the daemon's PATH".to_owned()),
        ..available("provider codex", "not usable, optional: no `codex` on PATH")
    });

    let output = home.against(&daemon, &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        checklist.contains("sessions start with claude"),
        "{checklist}"
    );
    assert!(
        checklist.contains(
            "provider codex: not usable, optional: no `codex` on PATH; \
             to use it, install codex on the daemon's PATH\n"
        ),
        "{checklist}"
    );
    assert!(!final_next(&checklist).contains("--agent"), "{checklist}");
}

/// #624: with no usable provider, the action is the blocking `providers` row's, which names
/// every supported agent, not the first unusable one's.
#[tokio::test]
async fn without_a_usable_provider_the_action_is_the_blocking_requirements() {
    let home = Home::new();
    home.accept_sandbox();
    let mut daemon = Daemon::ready();
    daemon
        .requirements
        .retain(|requirement| !requirement.name.starts_with("provider claude"));
    daemon.requirements.push(RequirementView {
        next: Some("to use it, install claude on the daemon's PATH".to_owned()),
        ..available(
            "provider claude",
            "not usable, optional: no `claude` on PATH",
        )
    });
    daemon.requirements.push(requirement(
        "providers",
        ReadinessStatusView::MissingPrerequisite,
        "install a supported agent (claude, codex, opencode) on the daemon's PATH",
    ));

    let output = home.against(&daemon, &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        final_next(&checklist).starts_with("next: install a supported agent"),
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
        checklist.starts_with("Torlek setup: 5 of 6 steps done"),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).starts_with("Torlek is set up"),
        "{checklist}"
    );
}

/// #965, #1250: init never says the setup is done while the next session would be refused.
#[tokio::test]
async fn a_launch_gate_that_refuses_holds_the_session_step_with_its_fix() {
    let home = Home::new();
    home.accept_sandbox();
    let daemon = Daemon::ready()
        .with_project()
        .with_session()
        .with_launch_gate(requirement(
            "launch desktop session",
            ReadinessStatusView::MissingPrerequisite,
            "set agents.desktop_session = \"withheld\"",
        ));

    let output = home.against(&daemon, &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Start a session").starts_with("[todo"),
        "{checklist}"
    );
    assert!(
        checklist.contains(
            "launch desktop session: launch desktop session is not available (missing-prerequisite)"
        ),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).starts_with("next: set agents.desktop_session = \"withheld\""),
        "{checklist}"
    );
}

/// #968, #1178: what the first session will ask of the owner is said before it starts.
#[tokio::test]
async fn a_launch_gate_that_needs_the_owner_is_named_in_the_session_step() {
    let home = Home::new();
    home.accept_sandbox();
    let daemon = Daemon::ready().with_project().with_launch_gate(requirement(
        "launch folder trust",
        ReadinessStatusView::NeedsAuthorization,
        "add its main checkout to projects.trusted",
    ));

    let output = home.against(&daemon, &["init"]).await;

    let checklist = text(&output.stdout);
    assert!(
        checklist.contains(
            "launch folder trust: launch folder trust is not available (needs-authorization)"
        ),
        "{checklist}"
    );
    assert!(
        final_next(&checklist).starts_with("next: `torlek session new '/home/me/code/my app'`"),
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
    assert!(usage.starts_with("usage: torlek "), "{usage}");
    assert!(usage.contains("\n       torlek init\n"), "{usage}");
}

/// #1048: the second request is for a sign-in the first answer may lack; with each provider's
/// answer in it, the scripted daemon's one conversation is all `torlek` alone needs.
#[tokio::test]
async fn cockpit_alone_asks_once_when_every_sign_in_answered() {
    let home = Home::new();
    home.accept_sandbox();
    let daemon = Daemon::ready().replace(available("provider claude sign-in", "signed in"));

    let output = home.against(&daemon, &[]).await;

    let checklist = text(&output.stdout);
    assert!(
        step(&checklist, "Sign in to the provider").starts_with("[done"),
        "{checklist}"
    );
    assert!(
        step(&checklist, "Add a project").starts_with("[todo"),
        "{checklist}"
    );
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
    home.require_sandbox();
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
    assert!(
        after.contains("`torlek network grant app github.com` allows it always"),
        "{after}"
    );
    home.cockpit_ok(&["network", "grant", "app", "github.com"]);
    let granted = home.cockpit_ok(&["init"]);
    assert!(!granted.contains("network grant"), "{granted}");
}
