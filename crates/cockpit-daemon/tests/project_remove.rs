//! Issue #727 at the daemon's socket: a project with a session that has not ended stays
//! registered, and the refusal names the session; once it ended, the project is removed.
#![cfg(target_os = "linux")]
mod candidate;
mod common;
mod messaging_fixture;

use std::sync::Arc;

use cockpit_protocol::{Command, ErrorCode, Reply};
use cockpit_provider_api::ProviderName;
use cockpit_provider_api::fake::FakeProvider;
use common::{GENEROUS, refused};
use messaging_fixture::Home;

fn remove(project: &str) -> Command {
    Command::ProjectRemove {
        project: project.to_owned(),
    }
}

async fn project_names(client: &cockpit_client::Connection) -> Vec<String> {
    match client
        .request(Command::ProjectList, GENEROUS)
        .await
        .unwrap()
    {
        Reply::Projects { projects } => projects.into_iter().map(|project| project.name).collect(),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_project_with_a_running_session_is_kept_and_the_refusal_names_the_session() {
    let home = Home::new();
    let provider = FakeProvider::new(
        ProviderName::new("fake").unwrap(),
        home.path().join("launches"),
    );
    let _daemon = home.start(vec![Arc::new(provider)]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;

    let refusal = refused(client.request(remove("project"), GENEROUS).await);

    assert_eq!(refusal.code, ErrorCode::Conflict);
    assert!(
        refusal.message.contains(&format!("session {}", session.id)),
        "{refusal:?}"
    );
    assert!(
        refusal
            .unblock
            .is_some_and(|unblock| unblock.action.contains("session end")),
    );
    assert_eq!(project_names(&client).await, ["project"]);
}

#[tokio::test]
async fn a_project_whose_sessions_all_ended_is_removed() {
    let home = Home::new();
    let provider = FakeProvider::new(
        ProviderName::new("fake").unwrap(),
        home.path().join("launches"),
    );
    let _daemon = home.start(vec![Arc::new(provider)]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    client
        .request(
            Command::SessionEnd {
                session: session.id.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();

    let removed = client.request(remove("project"), GENEROUS).await.unwrap();

    assert!(
        matches!(removed, Reply::ProjectRemoved { .. }),
        "{removed:?}"
    );
    assert!(project_names(&client).await.is_empty());
}

#[tokio::test]
async fn a_live_sessions_own_clone_is_not_offered_for_release() {
    let home = Home::new();
    let provider = FakeProvider::new(
        ProviderName::new("fake").unwrap(),
        home.path().join("launches"),
    );
    let _daemon = home.start(vec![Arc::new(provider)]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let reply = client
        .request(
            Command::SessionNew {
                dir: root.display().to_string(),
                agent: Some("fake".to_owned()),
                prompt: None,
                clone: Some("feature".to_owned()),
                model: None,
                effort: None,
            },
            GENEROUS,
        )
        .await
        .unwrap();
    assert!(matches!(reply, Reply::Session { .. }), "{reply:?}");

    let refusal = refused(client.request(remove("project"), GENEROUS).await);

    assert!(refusal.message.contains("has not ended"), "{refusal:?}");
    assert!(!refusal.message.contains("checkout release"), "{refusal:?}");
}
