//! Issue #1298: a coordinator's watcher printed no `QUESTION` for a worker's `--needs-input`
//! report that came after two refused reports. Driven through the real daemon and the real
//! `torlek` run by two launched agents: the worker reports, the coordinator follows
//! `torlek agent sessions --watch`, and its output must carry the worker's question whether
//! or not the report changes the worker's state. A question always means `waiting`, so only
//! a worker already waiting asks without a state change.

mod candidate;
mod real_daemon;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_protocol::{Command, Reply};
use real_daemon::{COMMANDED_PROJECT, GENEROUS, Home, cockpit_program, new_session};

const ASKED: &str = "Which branch should I base the fix on?";
const DOING: &str = "coding the fix now";
const WATCH_LOG: &str = "watch.log";

fn cockpit_line(arguments: &[&str]) -> String {
    std::iter::once(cockpit_program().display().to_string())
        .chain(arguments.iter().map(|argument| (*argument).to_owned()))
        .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A coordinator following `agent sessions --watch` into [`WATCH_LOG`], and a worker in
/// another project that reported [`DOING`] in a given state.
struct Watched {
    home: Home,
    _daemon: real_daemon::Daemon,
    coordinator_root: PathBuf,
    worker_root: PathBuf,
    worker: String,
}

impl Watched {
    async fn new(worker_state: &str) -> Watched {
        let cockpit = cockpit_program();
        assert!(cockpit.is_file(), "{} is missing", cockpit.display());
        let home = Home::new();
        let daemon = home.start();
        let owner = home.connect().await;
        let (coordinator_root, _) = home
            .project_at(&owner, &format!("alpha/{COMMANDED_PROJECT}"), Some("alpha"))
            .await;
        let (worker_root, _) = home
            .project_at(
                &owner,
                &format!("worker/{COMMANDED_PROJECT}"),
                Some("worker"),
            )
            .await;
        let coordinator = new_session(&owner, &coordinator_root).await.id;
        let worker = new_session(&owner, &worker_root).await.id;
        grant_observe_sessions(&owner, &coordinator).await;
        let watched = Watched {
            home,
            _daemon: daemon,
            coordinator_root,
            worker_root,
            worker,
        };
        // In the foreground: a background job would leave the launched process tree, and
        // Torlek refuses a sender outside it. The coordinator runs nothing else.
        let watch = format!(
            "timeout 120 {} > {WATCH_LOG} 2> watch.err",
            cockpit_line(&["agent", "sessions", "--watch"])
        );
        watched
            .home
            .command_agent(&watched.coordinator_root, &watch);
        let reported = watched
            .report(&["--state", worker_state, "--doing", DOING])
            .await;
        assert!(reported.ends_with("exit=0\n"), "{reported}");
        watched
            .wait_for_worker_line(|line| {
                line.contains(&format!("\t{worker_state}\t"))
                    && line.contains(&format!("doing={DOING:?}"))
            })
            .await;
        watched
    }

    async fn run(&self, root: &Path, line: &str) -> String {
        let number = self.home.command_agent(root, line);
        self.home.agent_output(root, number).await
    }

    /// What the worker's `torlek agent report ARGUMENTS` printed, ending `exit=<code>`.
    async fn report(&self, arguments: &[&str]) -> String {
        let words: Vec<&str> = ["agent", "report"]
            .into_iter()
            .chain(arguments.iter().copied())
            .collect();
        self.run(&self.worker_root, &cockpit_line(&words)).await
    }

    /// The first line the watch printed for the worker that `wanted` accepts.
    async fn wait_for_worker_line(&self, wanted: impl Fn(&str) -> bool) -> String {
        let log = self.coordinator_root.join(WATCH_LOG);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let printed = std::fs::read_to_string(&log).unwrap_or_default();
            if let Some(line) = printed
                .lines()
                .find(|line| line.starts_with(&format!("{}\t", self.worker)) && wanted(line))
            {
                return line.to_owned();
            }
            assert!(
                Instant::now() < deadline,
                "the watch printed no such line for {}:\n{printed}\nstderr: {}",
                self.worker,
                std::fs::read_to_string(self.coordinator_root.join("watch.err"))
                    .unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The worker's two refused reports from the incident: a question and a doing-now each
    /// over its length limit.
    async fn refused_reports(&self) {
        for (flag, text) in [
            ("--needs-input", "q".repeat(301)),
            ("--doing", "overlong ".repeat(10)),
        ] {
            let refused = self.report(&[flag, text.trim_end()]).await;
            assert!(
                !refused.ends_with("exit=0\n"),
                "{flag} was not refused: {refused}"
            );
        }
    }
}

async fn grant_observe_sessions(owner: &Connection, session: &str) {
    let answer = owner
        .request(
            Command::SessionGrant {
                session: session.to_owned(),
                permission: "observe-sessions".to_owned(),
                all_projects: true,
                without_confirmation: false,
                per_hour: None,
                max_active: None,
            },
            GENEROUS,
        )
        .await;
    assert!(
        matches!(answer, Ok(Reply::CoordinatorGrants { .. })),
        "session-grant observe-sessions: {answer:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_watch_prints_a_question_that_moves_a_working_worker_to_waiting() {
    let watched = Watched::new("working").await;
    watched.refused_reports().await;

    let asked = watched.report(&["--needs-input", ASKED]).await;

    assert!(asked.ends_with("exit=0\n"), "{asked}");
    let line = watched
        .wait_for_worker_line(|line| line.contains(&format!("needs-input={ASKED:?}")))
        .await;
    assert!(line.contains("\twaiting\t"), "{line}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_watch_prints_a_question_from_a_worker_already_waiting() {
    let watched = Watched::new("waiting").await;
    watched.refused_reports().await;

    let asked = watched.report(&["--needs-input", ASKED]).await;

    assert!(asked.ends_with("exit=0\n"), "{asked}");
    let line = watched
        .wait_for_worker_line(|line| line.contains(&format!("needs-input={ASKED:?}")))
        .await;
    assert!(line.contains("\twaiting\t"), "{line}");
}
