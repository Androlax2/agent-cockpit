//! The commands `init` and `doctor` print name the command that reaches the profile they ran
//! under (#964): an owner who copies `torlek project add` from `torlek-next init` must not
//! register the project in the default profile's installation.

use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    HandshakeReply, Outcome, ProjectView, ProtocolVersion, ReadinessStatusView, Reply, Request,
    RequirementView, Response, ServerFrame, Welcome,
};
use scripted::{scripted_conversation, text};
use tokio::io::AsyncBufReadExt;

/// How the owner reached profile `next`, and the command prefix every hint then needs.
struct Invocation {
    program_name: &'static str,
    leading_arguments: &'static [&'static str],
    expected_prefix: &'static str,
}

const BY_FLAG: Invocation = Invocation {
    program_name: "torlek",
    leading_arguments: &["--profile", "next"],
    expected_prefix: "torlek --profile next",
};

const BY_COMMAND_NAME: Invocation = Invocation {
    program_name: "torlek-next",
    leading_arguments: &[],
    expected_prefix: "torlek-next",
};

const DAEMON_PREFIX: &str = "torlek-daemon --profile next";

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        let home = Home {
            dir: tempfile::tempdir().unwrap(),
        };
        let owner_config = home.dir.path().join(".config/torlek-next/config.toml");
        std::fs::create_dir_all(owner_config.parent().unwrap()).unwrap();
        std::fs::write(owner_config, "[agents]\nsandbox = \"off\"\n").unwrap();
        home
    }

    fn daemon_socket(&self) -> PathBuf {
        let path = self.dir.path().join("run/torlek-next/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    async fn run(&self, invocation: &Invocation, verb_arguments: &[&str]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"));
        command
            .arg0(invocation.program_name)
            .args(invocation.leading_arguments)
            .args(verb_arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"));
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn readiness_with_hints() -> Vec<RequirementView> {
    let available = |name: &str| RequirementView {
        name: name.to_owned(),
        status: ReadinessStatusView::Available,
        detail: "ready".to_owned(),
        next: None,
    };
    vec![
        available("tmux"),
        available("sandbox"),
        available("provider claude"),
        RequirementView {
            name: "provider claude sign-in".to_owned(),
            status: ReadinessStatusView::NeedsAuthorization,
            detail: "not signed in".to_owned(),
            next: Some("run `claude /login`, then run `torlek doctor` again".to_owned()),
        },
        RequirementView {
            name: "project".to_owned(),
            status: ReadinessStatusView::MissingPrerequisite,
            detail: "not registered".to_owned(),
            next: Some("register it with `torlek project add DIR`".to_owned()),
        },
    ]
}

fn project() -> ProjectView {
    ProjectView {
        name: "app".to_owned(),
        root: "/home/me/code/app".to_owned(),
        github_token: None,
    }
}

/// Whether `command` begins with the whole words of `prefix`: `torlek-nextfoo` does not.
fn starts_with_words(command: &str, prefix: &str) -> bool {
    command == prefix
        || command
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(' '))
}

/// Every command a hint puts between backticks and starts with `torlek` must be one of the
/// profile's own spellings.
fn assert_commands_reach_the_profile(output: &Output, invocation: &Invocation) {
    let report = text(&output.stdout);
    let mut commands_seen = 0;
    for command in report.split('`').skip(1).step_by(2) {
        if !command.starts_with("torlek") {
            continue;
        }
        commands_seen += 1;
        assert!(
            starts_with_words(command, invocation.expected_prefix)
                || starts_with_words(command, DAEMON_PREFIX),
            "`{command}` does not reach profile next; expected `{}`:\n{report}",
            invocation.expected_prefix
        );
    }
    assert!(commands_seen > 0, "no hint names a command:\n{report}");
}

#[tokio::test]
async fn init_without_a_daemon_names_the_profiles_commands() {
    for invocation in [&BY_FLAG, &BY_COMMAND_NAME] {
        let home = Home::new();

        let output = home.run(invocation, &["init"]).await;

        assert_commands_reach_the_profile(&output, invocation);
        assert!(
            text(&output.stdout).contains(&format!("`{DAEMON_PREFIX}`")),
            "{}",
            text(&output.stdout)
        );
    }
}

#[tokio::test]
async fn init_names_the_profiles_commands_for_a_project_and_a_session() {
    for invocation in [&BY_FLAG, &BY_COMMAND_NAME] {
        for projects in [Vec::new(), vec![project()]] {
            let home = Home::new();
            let conversation = scripted_conversation(
                &home.daemon_socket(),
                vec![
                    Outcome::Ok(Reply::Readiness {
                        requirements: readiness_with_hints(),
                    }),
                    Outcome::Ok(Reply::Projects { projects }),
                    Outcome::Ok(Reply::Sessions {
                        sessions: Vec::new(),
                    }),
                    Outcome::Ok(Reply::NetworkGrants { grants: Vec::new() }),
                ],
            );

            let output = home.run(invocation, &["init"]).await;

            conversation.await.unwrap();
            assert_commands_reach_the_profile(&output, invocation);
        }
    }
}

#[tokio::test]
async fn a_hint_the_daemon_wrote_names_the_profiles_command() {
    let home = Home::new();
    let conversation = scripted_conversation(
        &home.daemon_socket(),
        vec![
            Outcome::Ok(Reply::Readiness {
                requirements: readiness_with_hints(),
            }),
            Outcome::Ok(Reply::Projects {
                projects: Vec::new(),
            }),
            Outcome::Ok(Reply::Sessions {
                sessions: Vec::new(),
            }),
            Outcome::Ok(Reply::NetworkGrants { grants: Vec::new() }),
        ],
    );

    let output = home.run(&BY_COMMAND_NAME, &["init"]).await;

    conversation.await.unwrap();
    let checklist = text(&output.stdout);
    assert!(
        checklist.contains("run `claude /login`, then run `torlek-next doctor` again"),
        "{checklist}"
    );
}

#[tokio::test]
async fn doctor_without_a_daemon_names_the_profiles_commands() {
    for invocation in [&BY_FLAG, &BY_COMMAND_NAME] {
        let home = Home::new();

        let output = home.run(invocation, &["doctor"]).await;

        assert_commands_reach_the_profile(&output, invocation);
    }
}

/// A daemon that answers the readiness report on every connection that speaks: doctor first
/// connects only to see whether the daemon answers.
fn answer_readiness_to_every_client(
    socket: &std::path::Path,
    requirements: Vec<RequirementView>,
) -> tokio::task::JoinHandle<()> {
    let listener = tokio::net::UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let requirements = requirements.clone();
            tokio::spawn(async move {
                let (read_half, mut write_half) = stream.into_split();
                let mut lines = tokio::io::BufReader::new(read_half).lines();
                if lines.next_line().await.unwrap().is_none() {
                    return;
                }
                let welcome = HandshakeReply::Accepted(Welcome {
                    protocol_version: ProtocolVersion(1),
                    capabilities: Vec::new(),
                });
                scripted::send(&mut write_half, serde_json::to_value(welcome).unwrap()).await;
                let Some(line) = lines.next_line().await.unwrap() else {
                    return;
                };
                let request: Request = serde_json::from_str(&line).unwrap();
                let response = ServerFrame::Response(Response {
                    id: request.id,
                    outcome: Outcome::Ok(Reply::Readiness { requirements }),
                });
                scripted::send(&mut write_half, serde_json::to_value(response).unwrap()).await;
            });
        }
    })
}

#[tokio::test]
async fn doctor_renames_the_commands_in_the_hints_the_daemon_wrote() {
    for (invocation, renamed) in [
        (
            &BY_FLAG,
            "register it with `torlek --profile next project add DIR`",
        ),
        (
            &BY_COMMAND_NAME,
            "register it with `torlek-next project add DIR`",
        ),
    ] {
        let home = Home::new();
        let daemon =
            answer_readiness_to_every_client(&home.daemon_socket(), readiness_with_hints());

        let output = home.run(invocation, &["doctor"]).await;

        daemon.abort();
        let report = text(&output.stdout);
        assert!(report.contains(renamed), "{report}");
        assert_commands_reach_the_profile(&output, invocation);
    }
}
