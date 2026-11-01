//! What placed agent turns charge (#386): each placement holds an admission reservation, so
//! admission's own decisions and `in_use` see it, and its end gives the room back.

mod common;

use cockpit_resource_admission::{
    Admission, AdmissionRequest, Capacity, Demand, Hold, Limits, ReservationState, Resource,
    Workload,
};
use cockpit_tasks::Priority;
use common::{Fixture, NOW, TURN};

async fn in_use(fixture: &Fixture, now: i64) -> Demand {
    let read = fixture.database.read().await.unwrap();
    cockpit_resource_admission::in_use(&read, now)
        .await
        .unwrap()
        .all
}

#[tokio::test]
async fn a_placed_agent_turn_is_in_use_as_a_held_reservation_labelled_with_its_work() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "Fix the login", Priority::NORMAL).await;

    fixture.dispatch_at(NOW, &[session]).await;

    assert_eq!(in_use(&fixture, NOW).await, TURN);
    let read = fixture.database.read().await.unwrap();
    let reservations = cockpit_resource_admission::reservations(&read)
        .await
        .unwrap();
    assert_eq!(reservations.len(), 1, "{reservations:?}");
    assert_eq!(reservations[0].state, ReservationState::Held);
    assert_eq!(
        reservations[0].label,
        format!("agent turn: item {} on a-1", item.id.0)
    );
}

#[tokio::test]
async fn a_build_admitted_while_turns_are_placed_gets_only_the_room_they_leave() {
    let fixture = Fixture::with_budget(2).await;
    let session = fixture.session("a-1", "a").await;
    fixture.queued("a", "Fix the login", Priority::NORMAL).await;
    fixture.dispatch_at(NOW, &[session]).await;

    let write = fixture.database.write().await.unwrap();
    let build = cockpit_resource_admission::admit(
        &write,
        &AdmissionRequest {
            label: "cargo build".to_owned(),
            workload: Workload::Build,
            demand: Demand {
                cpu_millis: TURN.cpu_millis * 2,
                memory_mib: TURN.memory_mib,
            },
            limits: Limits::default(),
            required: Vec::new(),
        },
        &fixture.policy,
        &[],
        NOW,
    )
    .await
    .unwrap();

    assert_eq!(
        build,
        Admission::Held(Hold::NoRoom {
            resource: Resource::Cpu,
            demand: TURN.cpu_millis * 2,
            available: TURN.cpu_millis,
            limited_by: Capacity::Budget,
        })
    );
}

#[tokio::test]
async fn a_finished_placement_gives_its_room_back() {
    let fixture = Fixture::with_budget(2).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "Fix the login", Priority::NORMAL).await;
    fixture
        .dispatch_at(NOW, std::slice::from_ref(&session))
        .await;
    fixture.finish("a-1", "a", item.id).await;

    let dispatched = fixture.dispatch_at(NOW + 60, &[session]).await;

    assert_eq!(dispatched.ended.len(), 1);
    assert_eq!(in_use(&fixture, NOW + 60).await, Demand::default());
}

#[tokio::test]
async fn a_placement_from_before_reservations_is_still_charged_by_the_pass() {
    let fixture = Fixture::with_budget(1).await;
    let first = fixture.session("a-1", "a").await;
    fixture.queued("a", "Fix the login", Priority::NORMAL).await;
    fixture.dispatch_at(NOW, std::slice::from_ref(&first)).await;
    // As a placement made before migration 2 stands: no reservation charges it.
    let write = fixture.database.write().await.unwrap();
    write
        .execute(
            "UPDATE resource_reservations SET state = 'released', released_reason = 'ended',
                 released_at = ?1",
            [NOW],
        )
        .await
        .unwrap();
    write
        .execute("UPDATE scheduling_placements SET reservation = NULL", ())
        .await
        .unwrap();
    write.commit().await.unwrap();
    let second = fixture.session("a-2", "a").await;
    let item = fixture
        .queued("a", "Fix the logout", Priority::NORMAL)
        .await;

    let dispatched = fixture.dispatch_at(NOW, &[first, second]).await;

    assert!(dispatched.placed.is_empty(), "{dispatched:?}");
    assert_eq!(
        common::explanation_of(&dispatched.explanations, item.id).reason,
        cockpit_scheduling::Reason::NoRoom
    );
}
