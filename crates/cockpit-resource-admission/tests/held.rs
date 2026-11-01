//! Held reservations (#386): charged until their holder releases them, never by an expiry or
//! an execution's end, and seen by every admission decision and `in_use`.

mod common;

use cockpit_resource_admission::{
    Admission, Capacity, Demand, Error, Hold, ReservationId, ReservationState, Resource, Swept,
    Workload,
};
use cockpit_turso::Database;
use common::*;

const NOW: i64 = 1_000_000;

async fn held(database: &Database, cpu_millis: u64) -> ReservationId {
    let write = database.write().await.unwrap();
    let admission = cockpit_resource_admission::hold(
        &write,
        &request(Workload::AgentTurn, cpu_millis, 1024),
        &policy(),
        &[],
        NOW,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    match admission {
        Admission::Admitted(id) => id,
        Admission::Held(hold) => panic!("held back: {hold}"),
    }
}

async fn release(database: &Database, id: ReservationId) -> Result<(), Error> {
    let write = database.write().await.unwrap();
    cockpit_resource_admission::release_held(&write, id, NOW).await?;
    write.commit().await.unwrap();
    Ok(())
}

#[tokio::test]
async fn a_held_reservation_takes_the_room_an_admitted_build_would_need() {
    let home = Home::new();
    let database = home.open_database().await;
    held(&database, 6000).await;

    let write = database.write().await.unwrap();
    let build = cockpit_resource_admission::admit(
        &write,
        &request(Workload::Build, 3000, 1024),
        &policy(),
        &[],
        NOW,
    )
    .await
    .unwrap();

    assert_eq!(
        build,
        Admission::Held(Hold::NoRoom {
            resource: Resource::Cpu,
            demand: 3000,
            available: 2000,
            limited_by: Capacity::Budget,
        })
    );
}

#[tokio::test]
async fn a_held_reservation_is_in_use_past_any_expiry_and_no_sweep_releases_it() {
    let home = Home::new();
    let database = home.open_database().await;
    let id = held(&database, 1500).await;
    let long_after = NOW + 365 * 24 * 3600;

    let swept = cockpit_resource_admission::sweep(&database, &home.supervisor(), long_after)
        .await
        .unwrap();

    assert_eq!(swept, Swept::default());
    let read = database.read().await.unwrap();
    let in_use = cockpit_resource_admission::in_use(&read, long_after)
        .await
        .unwrap();
    assert_eq!(
        in_use.all,
        Demand {
            cpu_millis: 1500,
            memory_mib: 1024
        }
    );
    let reservation = cockpit_resource_admission::reservation(&read, id)
        .await
        .unwrap();
    assert_eq!(reservation.state, ReservationState::Held);
}

#[tokio::test]
async fn releasing_a_held_reservation_gives_its_room_back() {
    let home = Home::new();
    let database = home.open_database().await;
    let id = held(&database, 6000).await;

    release(&database, id).await.unwrap();

    let read = database.read().await.unwrap();
    let in_use = cockpit_resource_admission::in_use(&read, NOW)
        .await
        .unwrap();
    assert_eq!(in_use.all, Demand::default());
}

#[tokio::test]
async fn a_reservation_not_held_cannot_be_released_as_held() {
    let home = Home::new();
    let database = home.open_database().await;
    let id = held(&database, 1000).await;
    release(&database, id).await.unwrap();

    let again = release(&database, id).await;

    assert!(
        matches!(
            again,
            Err(Error::WrongState {
                state: "released",
                needed: "held",
                ..
            })
        ),
        "{again:?}"
    );
}
