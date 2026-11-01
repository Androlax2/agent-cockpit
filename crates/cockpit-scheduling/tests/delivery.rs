//! A placement's delivery to its agent (#385): the pass enqueues it with the placement, and
//! one that fails for good ends the placement and puts the work back in the queue.

mod common;

use cockpit_messaging::{DeliveryState, Redelivery};
use cockpit_scheduling::{Placement, Unit};
use cockpit_tasks::{Actor, Priority, Status};
use cockpit_workflows::RESEARCH;
use common::{Fixture, NOW};

async fn envelope_of(
    fixture: &Fixture,
    placement: &Placement,
) -> cockpit_messaging::StoredEnvelope {
    let read = fixture.database.read().await.unwrap();
    let mut rows = read
        .query(
            "SELECT id FROM messaging_envelopes WHERE effect_key = ?1",
            [cockpit_scheduling::delivery_key(placement.id)],
        )
        .await
        .unwrap();
    let id = rows
        .next()
        .await
        .unwrap()
        .expect("an envelope")
        .get(0)
        .unwrap();
    drop(rows);
    cockpit_messaging::envelope(&read, cockpit_messaging::EnvelopeId(id))
        .await
        .unwrap()
        .unwrap()
}

async fn fail_delivery(fixture: &Fixture, placement: &Placement, now: i64) -> bool {
    let write = fixture.database.write().await.unwrap();
    let ended = cockpit_scheduling::delivery_failed(&write, placement.id, now)
        .await
        .unwrap();
    write.commit().await.unwrap();
    ended
}

#[tokio::test]
async fn a_placed_item_is_enqueued_for_its_session_with_the_line_that_gives_it_the_work() {
    let fixture = Fixture::with_budget(2).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "Fix the login", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    let placement = &dispatched.placed[0];
    let envelope = envelope_of(&fixture, placement).await;
    assert_eq!(
        (
            envelope.producer.as_str(),
            envelope.destination.as_str(),
            envelope.state,
            envelope.redelivery
        ),
        (
            cockpit_scheduling::DELIVERY_PRODUCER,
            "a-1",
            DeliveryState::Queued,
            Redelivery::Never
        )
    );
    assert_eq!(
        envelope.payload,
        format!(
            "Cockpit gives you item {id}: read it with `cockpit todo show {id}`, do it, then \
             finish it with `cockpit todo done {id} --summary '<what you did>'`.",
            id = item.id.0
        )
    );
}

#[tokio::test]
async fn a_placed_step_is_given_as_its_task_packet() {
    let fixture = Fixture::with_budget(2).await;
    let item = fixture.item("a", "question", Priority::NORMAL).await;
    let run = fixture.run(item.id, &RESEARCH).await;
    let session = fixture.session("a-1", "a").await;

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    let placement = &dispatched.placed[0];
    assert!(matches!(placement.unit, Unit::Step { .. }));
    assert_eq!(
        envelope_of(&fixture, placement).await.payload,
        format!(
            "Cockpit gives you step `investigate` of workflow run {run} (item {item}): read its \
             task packet with `cockpit workflow packet {run}` and hand the step off as it says.",
            run = run.id.0,
            item = item.id.0
        )
    );
}

#[tokio::test]
async fn a_failed_delivery_puts_the_item_back_in_the_queue_and_a_later_pass_places_it_again() {
    let fixture = Fixture::with_budget(2).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "Fix the login", Priority::NORMAL).await;
    let first = fixture
        .dispatch_at(NOW, std::slice::from_ref(&session))
        .await;

    assert!(fail_delivery(&fixture, &first.placed[0], NOW + 60).await);

    let read = fixture.database.read().await.unwrap();
    let handed_back = cockpit_tasks::item(&read, &Actor::Owner, item.id)
        .await
        .unwrap();
    drop(read);
    assert_eq!(
        (handed_back.status, handed_back.is_queued),
        (Status::Todo, true)
    );
    let again = fixture.dispatch_at(NOW + 120, &[session]).await;
    assert_eq!(again.placed.len(), 1, "{again:?}");
    assert_eq!(again.placed[0].item, item.id);
    assert_ne!(again.placed[0].id, first.placed[0].id);
}

#[tokio::test]
async fn a_failed_delivery_of_a_step_makes_the_step_ready_again() {
    let fixture = Fixture::with_budget(2).await;
    let item = fixture.item("a", "question", Priority::NORMAL).await;
    let run = fixture.run(item.id, &RESEARCH).await;
    let session = fixture.session("a-1", "a").await;
    let first = fixture.dispatch_at(NOW, &[session]).await;

    fail_delivery(&fixture, &first.placed[0], NOW + 60).await;

    let read = fixture.database.read().await.unwrap();
    let released = cockpit_workflows::run(&read, &cockpit_workflows::Actor::Owner, run.id)
        .await
        .unwrap();
    assert_eq!(released.steps[0].session, None);
}

#[tokio::test]
async fn a_failed_delivery_of_a_placement_already_ended_changes_nothing() {
    let fixture = Fixture::with_budget(2).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "Fix the login", Priority::NORMAL).await;
    let first = fixture
        .dispatch_at(NOW, std::slice::from_ref(&session))
        .await;
    fixture.finish("a-1", "a", item.id).await;
    fixture.dispatch_at(NOW + 60, &[session]).await;

    let ended = fail_delivery(&fixture, &first.placed[0], NOW + 120).await;

    assert!(!ended);
    let read = fixture.database.read().await.unwrap();
    let finished = cockpit_tasks::item(&read, &Actor::Owner, item.id)
        .await
        .unwrap();
    assert_eq!(finished.status, Status::Done);
}
