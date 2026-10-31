use cockpit_protocol::{
    Change, ChangeBatch, ChangeEvent, CommitRevision, Cursor, DatabaseEpoch, EntityKind, EntityRef,
    EntityRevision, ServerFrame,
};
use serde_json::json;

#[test]
fn a_change_batch_round_trips_through_its_wire_form() {
    let wire = json!({
        "type": "changes",
        "cursor": {"epoch": "e1", "revision": 41},
        "changes": [
            {
                "entity": {"kind": "task", "id": "t-1"},
                "entity_revision": 3,
                "change": {"kind": "upsert", "state": {"title": "Ship #20"}}
            },
            {
                "entity": {"kind": "session", "id": "s-9"},
                "entity_revision": 8,
                "change": {"kind": "tombstone"}
            }
        ]
    });

    let frame: ServerFrame = serde_json::from_value(wire.clone()).unwrap();

    let ServerFrame::Changes(batch) = &frame else {
        panic!("a change batch");
    };
    assert_eq!(
        batch.cursor,
        Cursor {
            epoch: DatabaseEpoch("e1".into()),
            revision: CommitRevision(41),
        }
    );
    assert_eq!(
        batch.changes[1],
        ChangeEvent {
            entity: EntityRef {
                kind: EntityKind::Session,
                id: "s-9".into(),
            },
            entity_revision: EntityRevision(8),
            change: Change::Tombstone,
        }
    );
    assert_eq!(serde_json::to_value(frame).unwrap(), wire);
}

#[test]
fn an_unknown_entity_kind_from_a_newer_daemon_is_preserved() {
    let wire = json!({
        "entity": {"kind": "garden", "id": "g-1"},
        "entity_revision": 1,
        "change": {"kind": "tombstone"}
    });

    let event: ChangeEvent = serde_json::from_value(wire.clone()).unwrap();

    assert_eq!(event.entity.kind, EntityKind::Unknown("garden".into()));
    assert_eq!(serde_json::to_value(event).unwrap(), wire);
}

#[test]
fn unknown_event_fields_from_a_newer_daemon_are_ignored() {
    let event: ChangeEvent = serde_json::from_value(json!({
        "entity": {"kind": "task", "id": "t-1", "shard": 2},
        "entity_revision": 1,
        "committed_at": "2026-10-31T10:00:00Z",
        "change": {"kind": "tombstone"}
    }))
    .unwrap();

    assert_eq!(event.change, Change::Tombstone);
}

/// A client that cannot apply a change must not advance its cursor past it, so an unknown
/// change kind fails the batch instead of being skipped.
#[test]
fn an_unknown_change_kind_fails_the_batch() {
    let parsed = serde_json::from_value::<ChangeBatch>(json!({
        "cursor": {"epoch": "e1", "revision": 2},
        "changes": [{
            "entity": {"kind": "task", "id": "t-1"},
            "entity_revision": 2,
            "change": {"kind": "merge", "patch": {}}
        }]
    }));

    assert!(parsed.is_err());
}

#[test]
fn an_upsert_state_must_be_an_object() {
    let parsed = serde_json::from_value::<Change>(json!({"kind": "upsert", "state": "done"}));

    assert!(parsed.is_err());
}

#[test]
fn every_known_entity_kind_has_its_published_name() {
    let published = [
        (EntityKind::Task, "task"),
        (EntityKind::Attempt, "attempt"),
        (EntityKind::Session, "session"),
        (EntityKind::Execution, "execution"),
        (EntityKind::Checkout, "checkout"),
    ];

    for (kind, name) in published {
        assert_eq!(serde_json::to_value(&kind).unwrap(), json!(name));
        assert_eq!(
            serde_json::from_value::<EntityKind>(json!(name)).unwrap(),
            kind
        );
    }
}
