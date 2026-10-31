//! Note images and drafts on a real state database.

mod common;

use cockpit_tasks::{Actor, DraftTarget, Edit, Refusal};
use common::{Fixture, NOW, refusal};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest of the image";

#[tokio::test]
async fn an_image_is_numbered_typed_from_its_bytes_and_is_an_intent_revision() {
    let fixture = Fixture::new().await;
    let item = fixture.owner_adds("app", "Fix the layout").await;
    let write = fixture.database.write().await.unwrap();

    let first = cockpit_tasks::add_image(&write, &Actor::Owner, item.id, PNG, NOW)
        .await
        .unwrap();
    let second = cockpit_tasks::add_image(&write, &Actor::Owner, item.id, b"GIF89a...", NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!((first.number, first.media_type), (1, "image/png"));
    assert_eq!((second.number, second.media_type), (2, "image/gif"));
    let read = fixture.database.read().await.unwrap();
    let stored = cockpit_tasks::image(&read, &Actor::Owner, item.id, 1)
        .await
        .unwrap();
    assert_eq!(stored.bytes, PNG);
    assert_eq!(fixture.item(item.id).await.intent_revision, 3);
}

#[tokio::test]
async fn bytes_that_are_not_an_image_are_refused() {
    let fixture = Fixture::new().await;
    let item = fixture.owner_adds("app", "Fix the layout").await;
    let write = fixture.database.write().await.unwrap();

    let refused = refusal(
        cockpit_tasks::add_image(&write, &Actor::Owner, item.id, b"#!/bin/sh\n", NOW).await,
    );

    assert!(
        matches!(refused, Refusal::InvalidImage { .. }),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_draft_survives_reopening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let sets = [cockpit_authorization::MIGRATIONS, cockpit_tasks::MIGRATIONS];
    let target = DraftTarget::New {
        project: "app".to_owned(),
    };
    {
        let database = cockpit_turso::Database::open(&path, &sets).await.unwrap();
        let write = database.write().await.unwrap();
        cockpit_tasks::save_draft(&write, &Actor::Owner, &target, "Half a ti", "", None, NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    let database = cockpit_turso::Database::open(&path, &sets).await.unwrap();
    let read = database.read().await.unwrap();
    let drafts = cockpit_tasks::drafts(&read, &Actor::Owner).await.unwrap();

    assert_eq!(drafts.len(), 1);
    assert_eq!(
        (drafts[0].target.clone(), drafts[0].title.as_str()),
        (target, "Half a ti")
    );
}

#[tokio::test]
async fn saving_a_draft_again_replaces_it() {
    let fixture = Fixture::new().await;
    let target = DraftTarget::New {
        project: "app".to_owned(),
    };
    let write = fixture.database.write().await.unwrap();
    for title in ["First", "Second"] {
        cockpit_tasks::save_draft(&write, &Actor::Owner, &target, title, "", None, NOW)
            .await
            .unwrap();
    }

    let drafts = cockpit_tasks::drafts(&write, &Actor::Owner).await.unwrap();

    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].title, "Second");
}

#[tokio::test]
async fn an_items_draft_is_stale_once_its_intent_changes() {
    let fixture = Fixture::new().await;
    let item = fixture.owner_adds("app", "Schema").await;
    let target = DraftTarget::Item(item.id);
    let write = fixture.database.write().await.unwrap();
    let draft = cockpit_tasks::save_draft(
        &write,
        &Actor::Owner,
        &target,
        "Schema v2",
        "",
        Some(1),
        NOW,
    )
    .await
    .unwrap();
    assert!(!draft.is_stale);

    cockpit_tasks::edit(
        &write,
        &Actor::Owner,
        item.id,
        None,
        Edit {
            notes: Some("Changed elsewhere.".to_owned()),
            ..Edit::default()
        },
        NOW,
    )
    .await
    .unwrap();
    cockpit_tasks::set_priority(
        &write,
        &Actor::Owner,
        item.id,
        cockpit_tasks::Priority::HIGH,
        NOW,
    )
    .await
    .unwrap();
    let drafts = cockpit_tasks::drafts(&write, &Actor::Owner).await.unwrap();

    assert!(drafts[0].is_stale);
    assert_eq!(drafts[0].base_intent_revision, Some(1));
}
