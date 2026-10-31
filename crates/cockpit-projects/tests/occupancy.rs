//! Checkout occupancy: one holder at a time, kept across restarts, bound to the incarnation.
mod common;

use cockpit_projects::{Checkout, Error, checkouts, claim, release};
use cockpit_turso::Database;
use common::{Scratch, holder, register};

async fn claim_committed(
    database: &Database,
    checkout: cockpit_projects::CheckoutId,
    who: &str,
) -> Result<Checkout, Error> {
    let write = database.write().await.unwrap();
    let claimed = claim(&write, checkout, &holder(who)).await;
    write.commit().await.unwrap();
    claimed
}

#[tokio::test]
async fn a_claimed_checkout_refuses_another_holder_naming_the_occupant() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let (_, main) = register(&database, &scratch.repository("app")).await;
    claim_committed(&database, main.id, "session-a")
        .await
        .unwrap();

    let refused = claim_committed(&database, main.id, "session-b").await;

    assert!(
        matches!(&refused, Err(Error::Occupied { holder, .. }) if holder.as_str() == "session-a"),
        "{refused:?}"
    );
}

#[tokio::test]
async fn the_occupant_claiming_again_keeps_its_occupancy() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let (_, main) = register(&database, &scratch.repository("app")).await;
    let first = claim_committed(&database, main.id, "session-a")
        .await
        .unwrap();

    let again = claim_committed(&database, main.id, "session-a")
        .await
        .unwrap();

    assert_eq!(again, first);
}

#[tokio::test]
async fn only_the_occupant_releases_a_checkout() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let (_, main) = register(&database, &scratch.repository("app")).await;
    claim_committed(&database, main.id, "session-a")
        .await
        .unwrap();

    let write = database.write().await.unwrap();
    let refused = release(&write, main.id, &holder("session-b")).await;

    assert!(
        matches!(&refused, Err(Error::NotOccupant { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn a_released_checkout_can_be_claimed_by_another_holder() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let (_, main) = register(&database, &scratch.repository("app")).await;
    claim_committed(&database, main.id, "session-a")
        .await
        .unwrap();
    let write = database.write().await.unwrap();
    release(&write, main.id, &holder("session-a"))
        .await
        .unwrap();
    write.commit().await.unwrap();

    let claimed = claim_committed(&database, main.id, "session-b")
        .await
        .unwrap();

    assert_eq!(claimed.occupancy.unwrap().holder.as_str(), "session-b");
}

#[tokio::test]
async fn occupancy_survives_reopening_the_database() {
    let scratch = Scratch::new();
    let root = scratch.repository("app");
    let main_id = {
        let database = scratch.open().await;
        let (_, main) = register(&database, &root).await;
        claim_committed(&database, main.id, "session-a")
            .await
            .unwrap();
        main.id
    };

    let database = scratch.open().await;
    let read = database.read().await.unwrap();
    let listed = checkouts(&read, None).await.unwrap();

    let occupant = listed
        .iter()
        .find(|checkout| checkout.id == main_id)
        .and_then(|checkout| checkout.occupancy.as_ref())
        .map(|occupancy| occupancy.holder.as_str());
    assert_eq!(occupant, Some("session-a"));
}

#[tokio::test]
async fn a_working_copy_replaced_outside_cockpit_refuses_claims() {
    let scratch = Scratch::new();
    let database = scratch.open().await;
    let root = scratch.repository("app");
    let (_, main) = register(&database, &root).await;
    std::fs::rename(root.join(".git"), scratch.path().join("old-git")).unwrap();
    common::git(&root, &["init", "--quiet"]);

    let refused = claim_committed(&database, main.id, "session-a").await;

    assert!(
        matches!(&refused, Err(Error::Replaced { .. })),
        "{refused:?}"
    );
}

#[tokio::test]
async fn an_unknown_checkout_is_refused() {
    let scratch = Scratch::new();
    let database = scratch.open().await;

    let refused = claim_committed(&database, cockpit_projects::CheckoutId(999), "session-a").await;

    assert!(
        matches!(&refused, Err(Error::UnknownCheckout { .. })),
        "{refused:?}"
    );
}
