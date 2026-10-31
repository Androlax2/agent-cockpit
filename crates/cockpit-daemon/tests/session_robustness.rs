//! Issue #157 at the daemon's socket: `resume` refuses a conversation still alive in
//! another session, `restart` works from an unknown status, and a session is labelled
//! finished only when the task list says so (decided in #201), with `session-end-finished`
//! ending those together.
//!
//! Same setting as `sessions.rs`: the daemon runs in this process with the fake provider
//! (baseline contract only), a real state database, real processes and a private tmux
//! server. The periodic pass is left at the fixture's hour, so it never runs mid-test.
#![cfg(target_os = "linux")]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{Command, ErrorCode, LaunchKindView, Reply, SessionStateView, SessionView};
use cockpit_provider_api::ProviderName;
use cockpit_provider_api::fake::{FakeLaunch as AgentLaunch, FakeProvider};
use common::GENEROUS;

struct Home {
    dir: tempfile::TempDir,
    database: Mutex<Option<Arc<cockpit_turso::Database>>>,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
            database: Mutex::new(None),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    fn launches(&self) -> Vec<AgentLaunch> {
        FakeProvider::launches(&self.path().join("agent.log"))
    }

    async fn start(&self) -> Daemon {
        let deadline = Instant::now() + GENEROUS;
        loop {
            let provider = FakeProvider::new(
                ProviderName::new("fake").unwrap(),
                self.path().join("agent.log"),
            );
            let mut wiring = common::wiring_in(&self.path(), vec![Arc::new(provider)]);
            wiring.sessions = cockpit_sessions::Settings {
                stop: cockpit_processes::Escalation {
                    grace: Duration::from_secs(2),
                    kill_wait: Duration::from_secs(5),
                },
                ..cockpit_sessions::Settings::default()
            };
            let options = Options {
                runtime_dir: self.path().join("run/cockpit"),
                state_dir: self.path().join("state/cockpit"),
                limits: Limits::default(),
            };
            let started = match Instance::acquire(options) {
                Ok(instance) => {
                    instance
                        .start(wiring, |capabilities| {
                            *self.database.lock().unwrap() =
                                Some(Arc::clone(&capabilities.database));
                            Arc::new(Routes::new(
                                capabilities,
                                cockpit_config::Environment {
                                    home: Some(self.path().join("home").into()),
                                    ..cockpit_config::Environment::default()
                                },
                            ))
                        })
                        .await
                }
                Err(error) => Err(error),
            };
            match started {
                Ok(daemon) => return daemon,
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => panic!("the daemon did not start: {error}"),
            }
        }
    }

    fn database(&self) -> Arc<cockpit_turso::Database> {
        self.database.lock().unwrap().clone().unwrap()
    }

    /// Writes `sql` with `session` as `?1` to the stored state: the fixture for a state an
    /// error, a crash or a provider quirk leaves behind.
    async fn store(&self, sql: &str, session: &str) {
        let database = self.database();
        let write = database.write().await.unwrap();
        write.execute(sql, [session]).await.unwrap();
        write.commit().await.unwrap();
    }

    /// A Git project `app` registered through the daemon, returning its root.
    async fn project(&self, client: &Connection) -> PathBuf {
        let root = self.path().join("projects/app");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("README"), "fixture\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "--quiet", "-m", "initial"]);
        request(
            client,
            Command::ProjectAdd {
                path: root.display().to_string(),
                name: None,
            },
        )
        .await
        .unwrap();
        root
    }

    /// The owner files an item in `app`; `session` starts it, as its agent would.
    async fn session_takes_an_item(&self, client: &Connection, session: &str) -> i64 {
        let added = request(
            client,
            Command::TaskAdd {
                launch_token: None,
                project: Some("app".to_owned()),
                title: format!("Work for {session}"),
                notes: String::new(),
                item_kind: None,
                priority: None,
                after: Vec::new(),
            },
        )
        .await;
        let Ok(Reply::Task { task }) = added else {
            panic!("task-add: {added:?}");
        };
        let database = self.database();
        let write = database.write().await.unwrap();
        cockpit_tasks::start(&write, &agent(session), cockpit_tasks::ItemId(task.id), 1)
            .await
            .unwrap();
        write.commit().await.unwrap();
        task.id
    }

    /// `session` finishes the item it is doing, as its agent would, a second after its latest
    /// launch, so the finish counts for that launch's finished label (#226) whatever second
    /// the test runs in.
    async fn session_finishes(&self, client: &Connection, session: &str, item: i64) {
        let launched_at = listed(client, session).await.launch.unwrap().launched_at;
        let database = self.database();
        let write = database.write().await.unwrap();
        cockpit_tasks::finish(
            &write,
            &agent(session),
            cockpit_tasks::ItemId(item),
            Some("done"),
            launched_at + 1,
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
    }

    /// A finished session whose launch and finish lie a minute in the past, so a relaunch now
    /// is strictly after its finish.
    async fn finished_a_minute_ago(&self, client: &Connection, session: &str) {
        self.store(
            "UPDATE session_launches SET launched_at = launched_at - 60 WHERE session_id = ?1",
            session,
        )
        .await;
        let item = self.session_takes_an_item(client, session).await;
        self.session_finishes(client, session, item).await;
        assert!(listed(client, session).await.finished);
    }

    fn kill_every_agent(&self) {
        let _ = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .arg("kill-server")
            .env_remove("TMUX")
            .output();
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        self.kill_every_agent();
    }
}

fn agent(session: &str) -> cockpit_tasks::Actor {
    cockpit_tasks::Actor::Session {
        session: cockpit_authorization::SessionId::new(session).unwrap(),
        project: Some("app".to_owned()),
    }
}

fn git(dir: &Path, arguments: &[&str]) {
    let output = Process::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
        .args(arguments)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {arguments:?}");
}

async fn connect(daemon: &Daemon) -> Connection {
    Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap()
}

async fn request(client: &Connection, command: Command) -> Result<Reply, ClientError> {
    client.request(command, Duration::from_secs(60)).await
}

async fn session_reply(client: &Connection, command: Command) -> SessionView {
    match request(client, command.clone()).await {
        Ok(Reply::Session { session }) => *session,
        other => panic!("{command:?}: {other:?}"),
    }
}

async fn new_session(client: &Connection, dir: &Path) -> SessionView {
    session_reply(
        client,
        Command::SessionNew {
            dir: dir.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await
}

async fn listed(client: &Connection, id: &str) -> SessionView {
    let Ok(Reply::Sessions { sessions }) =
        request(client, Command::SessionList { ended: true }).await
    else {
        panic!("session-list failed");
    };
    sessions
        .into_iter()
        .find(|session| session.id == id)
        .unwrap()
}

fn is_alive(pid: i32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .unwrap_or_default()
            .contains(") Z ")
}

async fn end(client: &Connection, id: &str) -> SessionView {
    session_reply(
        client,
        Command::SessionEnd {
            session: id.to_owned(),
        },
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_is_refused_while_another_session_runs_its_conversation_and_names_it() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let (first, second) = sharing_one_conversation(&home, &client, "").await;
    end(&client, &first.id).await;

    let refused = common::refused(
        request(
            &client,
            Command::SessionResume {
                session: first.id.clone(),
            },
        )
        .await,
    );

    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(refused.message.contains(&second.id), "{}", refused.message);
    assert_eq!(
        listed(&client, &first.id).await.state,
        SessionStateView::Ended
    );
    assert_eq!(home.launches().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_is_refused_while_a_failed_session_s_agent_of_its_conversation_is_alive() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    // A launch that failed after its agent started, as its record says, on the first
    // session's conversation: its state says nothing runs, its execution says otherwise.
    let (first, second) = sharing_one_conversation(&home, &client, ", state = 'failed'").await;
    end(&client, &first.id).await;

    let refused = common::refused(
        request(
            &client,
            Command::SessionResume {
                session: first.id.clone(),
            },
        )
        .await,
    );

    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(
        refused.message.contains(&second.id) && refused.message.contains("still alive"),
        "{}",
        refused.message
    );
    assert!(is_alive(home.launches()[1].pid));
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_reopens_the_conversation_once_the_other_session_holding_it_ended() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let (first, second) = sharing_one_conversation(&home, &client, "").await;
    end(&client, &first.id).await;
    end(&client, &second.id).await;

    let resumed = session_reply(
        &client,
        Command::SessionResume {
            session: first.id.clone(),
        },
    )
    .await;

    assert_eq!(resumed.state, SessionStateView::Running);
    assert_eq!(home.launches()[2].conversation, "conversation-1");
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_from_a_state_an_error_left_unfinished_relaunches_the_conversation() {
    let unknown_statuses = [
        "state = 'stopping', stopping_to = 'asleep'",
        "state = 'stopping', stopping_to = 'ended'",
        "state = 'stopping', stopping_to = 'restart'",
        "state = 'launching', stopping_to = NULL",
    ];
    for status in unknown_statuses {
        let home = Home::new();
        let daemon = home.start().await;
        let client = connect(&daemon).await;
        let root = home.project(&client).await;
        let started = new_session(&client, &root).await;
        home.store(
            &format!("UPDATE sessions SET {status} WHERE id = ?1"),
            &started.id,
        )
        .await;

        let restarted = session_reply(
            &client,
            Command::SessionRestart {
                session: started.id.clone(),
            },
        )
        .await;

        assert_eq!(restarted.state, SessionStateView::Running, "{status}");
        assert_eq!(restarted.launch.unwrap().kind, LaunchKindView::Restart);
        let launches = home.launches();
        assert_eq!(launches.len(), 2, "{status}");
        assert!(!is_alive(launches[0].pid), "{status}");
        assert_eq!(
            (launches[1].mode.as_str(), launches[1].conversation.as_str()),
            ("resume", "conversation-1"),
            "{status}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_of_a_session_whose_agent_died_unnoticed_relaunches_it() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    Process::new("kill")
        .arg(home.launches()[0].pid.to_string())
        .status()
        .unwrap();

    let restarted = session_reply(
        &client,
        Command::SessionRestart {
            session: started.id.clone(),
        },
    )
    .await;

    assert_eq!(restarted.state, SessionStateView::Running);
    let launches = home.launches();
    assert_eq!(launches.len(), 2);
    assert!(is_alive(launches[1].pid));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_is_finished_only_once_the_item_it_was_doing_is_done() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    assert!(!started.finished, "a session with no item is not finished");
    let item = home.session_takes_an_item(&client, &started.id).await;
    assert!(!listed(&client, &started.id).await.finished);

    home.session_finishes(&client, &started.id, item).await;

    assert!(listed(&client, &started.id).await.finished);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_doing_another_item_after_finishing_one_is_not_finished() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    let first = home.session_takes_an_item(&client, &started.id).await;
    home.session_finishes(&client, &started.id, first).await;

    home.session_takes_an_item(&client, &started.id).await;

    assert!(!listed(&client, &started.id).await.finished);
}

/// #226: a session the owner restarts to keep working is not finished by the work it did
/// before, until it finishes new work.
#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_session_is_finished_again_only_once_it_finishes_new_work() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    home.finished_a_minute_ago(&client, &started.id).await;

    let restarted = session_reply(
        &client,
        Command::SessionRestart {
            session: started.id.clone(),
        },
    )
    .await;

    assert!(!restarted.finished, "{restarted:?}");
    let running = listed(&client, &started.id).await;
    assert_eq!(running.state, SessionStateView::Running);
    assert!(!running.finished);
    let item = home.session_takes_an_item(&client, &started.id).await;
    home.session_finishes(&client, &started.id, item).await;
    assert!(listed(&client, &started.id).await.finished);
}

/// #226: `X` (`session-end-finished`) never ends the conversation the owner just resumed.
#[tokio::test(flavor = "multi_thread")]
async fn end_finished_keeps_a_session_resumed_after_its_work_was_finished() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    home.finished_a_minute_ago(&client, &started.id).await;
    end(&client, &started.id).await;
    session_reply(
        &client,
        Command::SessionResume {
            session: started.id.clone(),
        },
    )
    .await;

    let reply = request(&client, Command::SessionEndFinished { only: None }).await;

    let Ok(Reply::EndedFinished { ended, failed }) = reply else {
        panic!("session-end-finished: {reply:?}");
    };
    assert!(
        ended.is_empty() && failed.is_empty(),
        "{ended:?} {failed:?}"
    );
    let resumed = listed(&client, &started.id).await;
    assert_eq!(resumed.state, SessionStateView::Running);
    assert!(!resumed.finished);
}

#[tokio::test(flavor = "multi_thread")]
async fn end_finished_ends_the_finished_sessions_and_keeps_the_others() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let finished = new_session(&client, &root).await;
    let working = new_session(&client, &root).await;
    let done = home.session_takes_an_item(&client, &finished.id).await;
    home.session_finishes(&client, &finished.id, done).await;
    home.session_takes_an_item(&client, &working.id).await;

    let reply = request(&client, Command::SessionEndFinished { only: None }).await;

    let Ok(Reply::EndedFinished { ended, failed }) = reply else {
        panic!("session-end-finished: {reply:?}");
    };
    assert!(failed.is_empty(), "{failed:?}");
    let ended: Vec<&str> = ended.iter().map(|session| session.id.as_str()).collect();
    assert_eq!(ended, [finished.id.as_str()]);
    assert_eq!(
        listed(&client, &finished.id).await.state,
        SessionStateView::Ended
    );
    assert_eq!(
        listed(&client, &working.id).await.state,
        SessionStateView::Running
    );
    assert!(!is_alive(home.launches()[0].pid));
}

#[tokio::test(flavor = "multi_thread")]
async fn end_finished_with_only_ends_just_the_named_finished_sessions() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let seen = new_session(&client, &root).await;
    let unseen = new_session(&client, &root).await;
    for session in [&seen, &unseen] {
        let item = home.session_takes_an_item(&client, &session.id).await;
        home.session_finishes(&client, &session.id, item).await;
    }

    let reply = request(
        &client,
        Command::SessionEndFinished {
            only: Some(vec![seen.id.clone()]),
        },
    )
    .await;

    let Ok(Reply::EndedFinished { ended, .. }) = reply else {
        panic!("session-end-finished: {reply:?}");
    };
    let ended: Vec<&str> = ended.iter().map(|session| session.id.as_str()).collect();
    assert_eq!(ended, [seen.id.as_str()]);
    assert_eq!(
        listed(&client, &unseen.id).await.state,
        SessionStateView::Running
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ended_session_is_never_labelled_finished() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    let item = home.session_takes_an_item(&client, &started.id).await;
    home.session_finishes(&client, &started.id, item).await;

    end(&client, &started.id).await;

    assert!(!listed(&client, &started.id).await.finished);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_session_is_never_labelled_finished() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let started = new_session(&client, &root).await;
    let item = home.session_takes_an_item(&client, &started.id).await;
    home.session_finishes(&client, &started.id, item).await;

    home.store(
        "UPDATE sessions SET state = 'failed', reason = 'its relaunch failed' WHERE id = ?1",
        &started.id,
    )
    .await;

    assert!(!listed(&client, &started.id).await.finished);
}

#[tokio::test(flavor = "multi_thread")]
async fn wake_is_refused_while_another_session_runs_its_conversation() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let (first, second) = sharing_one_conversation(&home, &client, "").await;
    session_reply(
        &client,
        Command::SessionSleep {
            session: first.id.clone(),
        },
    )
    .await;

    let refused = common::refused(
        request(
            &client,
            Command::SessionWake {
                session: first.id.clone(),
            },
        )
        .await,
    );

    assert_eq!(refused.code, ErrorCode::Conflict);
    assert!(refused.message.contains(&second.id), "{}", refused.message);
    assert_eq!(
        listed(&client, &first.id).await.state,
        SessionStateView::Asleep
    );
}

/// Two sessions of one project whose provider named the first one's conversation again for
/// the second; `more` adds to the second's stored row (`, state = 'failed'`).
async fn sharing_one_conversation(
    home: &Home,
    client: &Connection,
    more: &str,
) -> (SessionView, SessionView) {
    let root = home.project(client).await;
    let first = new_session(client, &root).await;
    let second = new_session(client, &root).await;
    home.store(
        &format!("UPDATE sessions SET conversation = 'conversation-1'{more} WHERE id = ?1"),
        &second.id,
    )
    .await;
    (first, second)
}
