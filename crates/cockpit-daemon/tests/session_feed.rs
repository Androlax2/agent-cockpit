//! Issue #199 at the daemon's socket: sessions and checkouts reach a change-feed subscriber
//! as they change, with no list request. Starting, sleeping and ending a session each end in
//! a change carrying exactly what the command answered (intermediate states such as
//! `launching` or `stopping` may arrive before it); a registered project's checkout follows
//! the same way.
//!
//! The daemon runs in this process, as in `sessions.rs`, for the fake provider.
#![cfg(target_os = "linux")]

mod common;

use std::collections::HashMap;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_client::{Connection, Subscription};
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{
    Change, CheckoutView, Command, EntityKind, Reply, SessionStateView, SessionView,
};
use cockpit_provider_api::ProviderName;
use cockpit_provider_api::fake::FakeProvider;
use common::GENEROUS;
use serde_json::{Map, Value};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    /// The owner's configuration file the daemon reads (`wiring_in`'s home).
    fn config_file(&self) -> PathBuf {
        self.path().join("home/.config/cockpit/config.toml")
    }

    /// Publishes a `session` entity on the daemon's state while no daemon runs, as a
    /// producer other than the daemon would.
    async fn publish_foreign_session(&self, id: &str) {
        let state_dir = self.path().join("state/cockpit");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&state_dir)
            .unwrap();
        let database = cockpit_turso::Database::open(
            &state_dir.join("state.db"),
            &cockpit_daemon::MIGRATION_SETS,
        )
        .await
        .unwrap();
        let write = database.write().await.unwrap();
        let state = serde_json::json!({"id": id});
        cockpit_messaging::publish(&write, "session", id, state.as_object().unwrap())
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    async fn start(&self) -> Daemon {
        let provider = FakeProvider::new(
            ProviderName::new("fake").unwrap(),
            self.path().join("agent.log"),
        );
        let mut wiring = common::wiring_in(&self.path(), vec![Arc::new(provider)]);
        wiring.sessions.stop = cockpit_processes::Escalation {
            grace: Duration::from_secs(2),
            kill_wait: Duration::from_secs(5),
        };
        let options = Options {
            runtime_dir: self.path().join("run/cockpit"),
            state_dir: self.path().join("state/cockpit"),
            limits: Limits::default(),
        };
        let environment = wiring.environment.clone();
        Instance::acquire(options)
            .unwrap()
            .start(wiring, |capabilities| {
                Arc::new(Routes::new(capabilities, environment))
            })
            .await
            .expect("the daemon starts")
    }

    /// A Git project registered through the daemon, returning its root.
    async fn project(&self, client: &Connection) -> PathBuf {
        let root = self.path().join("projects/app");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("README"), "fixture\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "--quiet", "-m", "initial"]);
        let origin = self.path().join("origin.git");
        git(
            &self.path(),
            &["init", "--quiet", "--bare", origin.to_str().unwrap()],
        );
        git(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        request(
            client,
            Command::ProjectAdd {
                path: root.display().to_string(),
                name: None,
            },
        )
        .await;
        root
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .arg("kill-server")
            .env_remove("TMUX")
            .output();
    }
}

fn git(dir: &Path, arguments: &[&str]) {
    let output = Process::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn request(client: &Connection, command: Command) -> Reply {
    client
        .request(command.clone(), Duration::from_secs(60))
        .await
        .unwrap_or_else(|error| panic!("{command:?}: {error}"))
}

async fn session_reply(client: &Connection, command: Command) -> SessionView {
    match request(client, command.clone()).await {
        Reply::Session { session } => *session,
        other => panic!("{command:?}: {other:?}"),
    }
}

/// What a subscriber holds of the feed: each entity's last state, `None` once retracted.
#[derive(Default)]
struct Followed {
    entities: HashMap<(EntityKind, String), Option<Map<String, Value>>>,
}

impl Followed {
    /// The state of `kind` `id` as last applied: `None` when never seen, `Some(None)` once
    /// retracted.
    fn state(&self, kind: EntityKind, id: &str) -> Option<Option<&Map<String, Value>>> {
        self.entities
            .get(&(kind, id.to_owned()))
            .map(Option::as_ref)
    }

    /// Applies batches until `kind` `id` is in the state `expected` (`None`: retracted).
    async fn until(
        &mut self,
        feed: &mut Subscription,
        kind: EntityKind,
        id: &str,
        expected: Option<&Map<String, Value>>,
    ) {
        let key = (kind, id.to_owned());
        let deadline = Instant::now() + GENEROUS;
        while self.entities.get(&key).map(Option::as_ref) != Some(expected) {
            let left = deadline.saturating_duration_since(Instant::now());
            let pending = tokio::time::timeout(left, feed.next_batch())
                .await
                .unwrap_or_else(|_| {
                    panic!("{key:?} never reached {expected:?}: {:?}", self.entities)
                })
                .expect("the feed continues");
            for event in &pending.batch().changes {
                let state = match &event.change {
                    Change::Upsert { state } => Some(state.clone()),
                    Change::Tombstone => None,
                };
                self.entities
                    .insert((event.entity.kind.clone(), event.entity.id.clone()), state);
            }
            pending.applied();
        }
    }
}

fn state_of(view: &impl serde::Serialize) -> Map<String, Value> {
    match serde_json::to_value(view).unwrap() {
        Value::Object(state) => state,
        other => panic!("not an object: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn starting_sleeping_and_ending_a_session_each_reach_a_subscriber() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    let mut feed = Subscription::new(daemon.socket_path().to_path_buf(), None, GENEROUS);
    let mut followed = Followed::default();

    let started = session_reply(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: None,
        },
    )
    .await;
    assert_eq!(started.state, SessionStateView::Running);
    followed
        .until(
            &mut feed,
            EntityKind::Session,
            &started.id,
            Some(&state_of(&started)),
        )
        .await;

    let asleep = session_reply(
        &client,
        Command::SessionSleep {
            session: started.id.clone(),
        },
    )
    .await;
    assert_eq!(asleep.state, SessionStateView::Asleep);
    followed
        .until(
            &mut feed,
            EntityKind::Session,
            &started.id,
            Some(&state_of(&asleep)),
        )
        .await;

    let ended = session_reply(
        &client,
        Command::SessionEnd {
            session: started.id.clone(),
        },
    )
    .await;
    assert_eq!(ended.state, SessionStateView::Ended);
    followed
        .until(&mut feed, EntityKind::Session, &started.id, None)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_registered_projects_checkout_reaches_a_subscriber() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let mut feed = Subscription::new(daemon.socket_path().to_path_buf(), None, GENEROUS);
    let mut followed = Followed::default();

    home.project(&client).await;
    let checkouts: Vec<CheckoutView> =
        match request(&client, Command::CheckoutList { project: None }).await {
            Reply::Checkouts { checkouts } => checkouts,
            other => panic!("{other:?}"),
        };

    let [main] = checkouts.as_slice() else {
        panic!("one checkout: {checkouts:?}");
    };
    followed
        .until(
            &mut feed,
            EntityKind::Checkout,
            &main.id.to_string(),
            Some(&state_of(main)),
        )
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_entity_the_daemon_did_not_derive_is_retracted() {
    let home = Home::new();
    home.publish_foreign_session("s-foreign").await;
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let mut feed = Subscription::new(daemon.socket_path().to_path_buf(), None, GENEROUS);
    let mut followed = Followed::default();

    // A later commit's checkout is a fence: the first pass, which retracts, came before it.
    home.project(&client).await;
    let checkouts = match request(&client, Command::CheckoutList { project: None }).await {
        Reply::Checkouts { checkouts } => checkouts,
        other => panic!("{other:?}"),
    };
    followed
        .until(
            &mut feed,
            EntityKind::Checkout,
            &checkouts[0].id.to_string(),
            Some(&state_of(&checkouts[0])),
        )
        .await;

    let foreign = followed.state(EntityKind::Session, "s-foreign");
    assert!(matches!(foreign, None | Some(None)), "{foreign:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_owner_config_holds_back_checkouts_but_not_sessions() {
    let home = Home::new();
    let daemon = home.start().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    let root = home.project(&client).await;
    let mut feed = Subscription::new(daemon.socket_path().to_path_buf(), None, GENEROUS);
    let mut followed = Followed::default();
    std::fs::create_dir_all(home.config_file().parent().unwrap()).unwrap();
    std::fs::write(home.config_file(), "this is not toml = = =\n").unwrap();

    let started = session_reply(
        &client,
        Command::SessionNew {
            dir: root.display().to_string(),
            agent: Some("fake".to_owned()),
            prompt: None,
            clone: Some("work".to_owned()),
        },
    )
    .await;
    followed
        .until(
            &mut feed,
            EntityKind::Session,
            &started.id,
            Some(&state_of(&started)),
        )
        .await;
    let clone = started.checkout.to_string();
    assert_eq!(followed.state(EntityKind::Checkout, &clone), None);

    // Once the configuration reads again, the next pass (a commit or the period) publishes it.
    std::fs::remove_file(home.config_file()).unwrap();
    let checkouts = match request(&client, Command::CheckoutList { project: None }).await {
        Reply::Checkouts { checkouts } => checkouts,
        other => panic!("{other:?}"),
    };
    let clone_view = checkouts
        .iter()
        .find(|checkout| checkout.id == started.checkout)
        .unwrap();
    followed
        .until(
            &mut feed,
            EntityKind::Checkout,
            &clone,
            Some(&state_of(clone_view)),
        )
        .await;
}
