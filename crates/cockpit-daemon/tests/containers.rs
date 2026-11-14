//! `torlek containers record` against the real daemon binary (#390): the owner records a
//! project's engine only once it answers as the project's own; until then the refusal names
//! the step that sets it up. The engine's socket is the system's (`/run/<user>`), so these
//! tests see the host without one; `cockpit-containers`' tests verify a fake engine. With no
//! engine user on the host, the daemon starts no engine proxy: the tests below stand in for
//! one in its folder, recording the engine in the daemon's database.

mod candidate;
mod real_daemon;

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cockpit_sandbox::{AskedDestination, EgressDecisions, EgressRequests, Subject, Target};
use real_daemon::{GENEROUS, Home};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_a_project_without_its_engine_is_refused_naming_the_setup_step() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let record = home.cockpit(&["containers", "record", "app"], &[]).await;

    let stderr = text(&record.stderr);
    assert!(!record.status.success(), "{stderr}");
    assert!(stderr.contains("no engine to record"), "{stderr}");
    assert!(
        stderr.contains(&format!("`torlek-dev doctor --project {}`", root.display())),
        "{stderr}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_an_unregistered_project_is_refused() {
    let home = Home::new();
    let _daemon = home.start();

    let record = home.cockpit(&["containers", "record", "nope"], &[]).await;

    let stderr = text(&record.stderr);
    assert!(!record.status.success(), "{stderr}");
    assert!(stderr.contains("no registered project nope"), "{stderr}");
}

/// A daemon whose registered `project` has its engine recorded. The database is the daemon's
/// alone while it runs, so it is recorded between two starts.
async fn with_recorded_engine(home: &Home, project: &str) -> real_daemon::Daemon {
    let daemon = home.start();
    let client = home.connect().await;
    home.project(&client, project).await;
    drop(client);
    drop(daemon);
    record(home, project, "torlek-dev-e1").await;
    home.start()
}

/// Records `user`'s engine for `project` in the stopped daemon's database, as `torlek
/// containers record` does once the engine answers.
async fn record(home: &Home, project: &str, user: &str) {
    let database = cockpit_turso::Database::open(
        &home.state_dir().join("state.db"),
        &cockpit_daemon::MIGRATION_SETS,
    )
    .await
    .unwrap();
    let write = database.write().await.unwrap();
    cockpit_containers::record_engine(
        &write,
        &cockpit_containers::RecordedEngine {
            project: project.to_owned(),
            user: user.to_owned(),
            engine_id: "engine-a".to_owned(),
            recorded_at: 1_000,
            recorded_through: Some("cli".to_owned()),
        },
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
}

/// The engine proxy's folder, as the daemon makes it, holding what the proxy refused.
fn refused_by_engine_proxy(home: &Home, project: &str, host: &str) -> PathBuf {
    let folder = home.engine_proxy_folder(project);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .unwrap();
    let requests = EgressRequests {
        subject: Subject::Containers {
            project: project.to_owned(),
        },
        revision: 1,
        asked: vec![AskedDestination {
            host: host.to_owned(),
            port: 443,
        }],
        services: Vec::new(),
    };
    std::fs::write(
        folder.join(cockpit_sandbox::REQUESTS_FILE),
        serde_json::to_vec(&requests).unwrap(),
    )
    .unwrap();
    folder
}

/// `torlek network list` once it prints `expected`, or as it is after [`GENEROUS`].
async fn network_list_once_it_contains(home: &Home, expected: &str) -> String {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let listed = home.cockpit(&["network", "list"], &[]).await;
        assert!(listed.status.success(), "{}", text(&listed.stderr));
        let printed = text(&listed.stdout);
        if printed.contains(expected) || Instant::now() > deadline {
            return printed;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// #390 part 6c, #856 Q4 A: a host a project engine's proxy refused is filed at once as a
/// request naming the project's containers.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_the_engines_proxy_refused_is_asked_of_the_owner_for_the_projects_containers() {
    let home = Home::new();
    let _daemon = with_recorded_engine(&home, "app").await;

    refused_by_engine_proxy(&home, "app", "crates.io");

    let asking = "network\t1\tasking\tcontainers of app\tapp\tcrates.io:443";
    let listed = network_list_once_it_contains(&home, asking).await;
    assert!(listed.contains(asking), "{listed}");
}

/// The owner's "allow always" reaches the engine proxy's decisions as the project's grant.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_the_owner_allowed_always_reaches_the_engines_proxy() {
    let home = Home::new();
    let _daemon = with_recorded_engine(&home, "app").await;
    let folder = refused_by_engine_proxy(&home, "app", "crates.io");
    let asking = "network\t1\tasking\tcontainers of app\tapp\tcrates.io:443";
    network_list_once_it_contains(&home, asking).await;
    // Out of the grace in which an allow is refused (#156).
    tokio::time::sleep(Duration::from_millis(2_100)).await;

    let allowed = home
        .cockpit(&["network", "allow", "1", "--always"], &[])
        .await;
    assert!(allowed.status.success(), "{}", text(&allowed.stderr));

    let deadline = Instant::now() + GENEROUS;
    let decisions = loop {
        let decisions: Option<EgressDecisions> =
            std::fs::read(folder.join(cockpit_sandbox::DECISIONS_FILE))
                .ok()
                .and_then(|written| serde_json::from_slice(&written).ok());
        let is_allowed = decisions.as_ref().is_some_and(|decisions| {
            decisions
                .allowed
                .iter()
                .any(|entry| entry.target == Target::Name("crates.io".to_owned()))
        });
        if is_allowed || Instant::now() > deadline {
            break decisions;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let decisions = decisions.expect("no decisions file");
    assert!(
        decisions
            .allowed
            .iter()
            .any(|entry| entry.target == Target::Name("crates.io".to_owned()) && entry.port == 443),
        "{decisions:?}"
    );
}

/// #390, decided 2026-11-09 (B): the owner's "allow Docker Hub" answer reaches the engine's
/// proxy as the project's grants, one per pull host.
#[tokio::test(flavor = "multi_thread")]
async fn docker_hub_granted_by_the_owner_reaches_the_engines_proxy() {
    let home = Home::new();
    let _daemon = with_recorded_engine(&home, "app").await;
    let folder = home.engine_proxy_folder("app");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .unwrap();

    let granted = home
        .cockpit(&["network", "grant", "app", "docker-hub"], &[])
        .await;

    assert!(granted.status.success(), "{}", text(&granted.stderr));
    let printed = text(&granted.stdout);
    for host in cockpit_sandbox::DOCKER_HUB_HOSTS {
        assert!(
            printed.contains(&format!("\tapp\t{host}:443\t")),
            "{printed}"
        );
    }
    let deadline = Instant::now() + GENEROUS;
    let allowed = loop {
        let allowed: Vec<Target> = std::fs::read(folder.join(cockpit_sandbox::DECISIONS_FILE))
            .ok()
            .and_then(|written| serde_json::from_slice::<EgressDecisions>(&written).ok())
            .map(|decisions| {
                decisions
                    .allowed
                    .into_iter()
                    .filter(|entry| entry.port == 443)
                    .map(|entry| entry.target)
                    .collect()
            })
            .unwrap_or_default();
        if allowed.len() == cockpit_sandbox::DOCKER_HUB_HOSTS.len() || Instant::now() > deadline {
            break allowed;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let docker_hub: Vec<Target> = cockpit_sandbox::DOCKER_HUB_HOSTS
        .iter()
        .map(|host| Target::Name((*host).to_owned()))
        .collect();
    assert_eq!(allowed, docker_hub);
}
