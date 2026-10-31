//! `published_ids` on a real state database: what a producer that publishes from its own
//! state reads to find the entities it must retract (#199).
mod common;

use cockpit_messaging::{Error, Refusal};
use common::Fixture;
use serde_json::{Map, Value, json};

fn state(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(state) => state,
        other => panic!("not an object: {other}"),
    }
}

async fn publish(fixture: &Fixture, kind: &str, id: &str) {
    let write = fixture.database.write().await.unwrap();
    cockpit_messaging::publish(&write, kind, id, &state(json!({"id": id})))
        .await
        .unwrap();
    write.commit().await.unwrap();
}

async fn published_ids(fixture: &Fixture, kind: &str) -> Result<Vec<String>, Error> {
    let read = fixture.database.read().await.unwrap();
    cockpit_messaging::published_ids(&read, kind).await
}

#[tokio::test]
async fn lists_the_ids_of_one_kind_in_order() {
    let fixture = Fixture::new().await;
    publish(&fixture, "session", "s-2").await;
    publish(&fixture, "session", "s-1").await;
    publish(&fixture, "checkout", "7").await;

    assert_eq!(
        published_ids(&fixture, "session").await.unwrap(),
        ["s-1", "s-2"]
    );
}

#[tokio::test]
async fn leaves_tombstones_out() {
    let fixture = Fixture::new().await;
    publish(&fixture, "session", "s-1").await;
    publish(&fixture, "session", "s-2").await;
    let write = fixture.database.write().await.unwrap();
    cockpit_messaging::retract(&write, "session", "s-1")
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(published_ids(&fixture, "session").await.unwrap(), ["s-2"]);
}

#[tokio::test]
async fn refuses_a_kind_that_is_not_kebab_case() {
    let fixture = Fixture::new().await;

    let refused = published_ids(&fixture, "Session").await;

    assert!(
        matches!(
            refused,
            Err(Error::Refused(Refusal::InvalidArgument {
                field: "kind",
                ..
            }))
        ),
        "{refused:?}"
    );
}
