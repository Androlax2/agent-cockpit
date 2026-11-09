//! Report 04's dispatch crash windows. Only transport is fake; reservation, outbox,
//! startup reconciliation, process reattachment and admission use production code.
use super::*;
use cockpit_messaging::DeliveryState;
use cockpit_scheduling::{Candidate, Inputs, Presence, Settings};

async fn crash_window(delivered: bool) {
    let home = Home::new();
    let daemon = home.start().await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let session = home
        .session_reporting(&client, &root, AgentState::Waiting)
        .await;
    let item = queued_item(&client, "durable dispatch").await;
    let original_launches = home.launches_of(&session.id);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // Stop at the committed reservation boundary before the daemon's transport worker.
    let database = home.database();
    let write = database.write().await.unwrap();
    let result = cockpit_scheduling::dispatch(
        &write,
        &Inputs {
            candidates: vec![Candidate {
                session: cockpit_authorization::SessionId::new(&session.id).unwrap(),
                project: Some("app".into()),
                role: None,
                capabilities: cockpit_provider_api::Capabilities::none().with(
                    cockpit_provider_api::Capability::Submit,
                    cockpit_provider_api::Provenance::Discovered,
                ),
                presence: Presence::Waiting { since: now - 3600 },
                provider: "claude".into(),
                account: None,
            }],
            viewed: None,
            usage: Vec::new(),
            now,
        },
        &Settings {
            idle_window: Duration::ZERO,
            ..Settings::default()
        },
        &cockpit_resource_admission::Policy::for_budget(cockpit_resource_admission::Demand {
            cpu_millis: 10000,
            memory_mib: 10000,
        }),
        &[],
    )
    .await
    .unwrap();
    assert_eq!(result.placed.len(), 1);
    let placement = result.placed[0].clone();
    write.commit().await.unwrap();
    let read = database.read().await.unwrap();
    let envelope =
        cockpit_messaging::envelope_by_key(&read, &cockpit_scheduling::delivery_key(placement.id))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(envelope.state, DeliveryState::Queued);
    drop(read);
    if delivered {
        let write = database.write().await.unwrap();
        let claims =
            cockpit_messaging::claim_due(&write, cockpit_scheduling::DELIVERY_PRODUCER, now, 1)
                .await
                .unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].id, envelope.id);
        write.commit().await.unwrap();
        request(
            &client,
            Command::SessionSend {
                session: session.id.clone(),
                message: claims[0].payload.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            home.messages_of(&session.id, 1).await,
            std::slice::from_ref(&envelope.payload)
        );
        // The process has the work; the outbox acknowledgement was never committed.
    }
    drop(client);
    drop(database);
    drop(daemon);
    home.database.lock().unwrap().take();
    let restarted = home.start().await;
    let client = connect(&restarted).await;
    // Explicit passes drive delivery, but startup's immediate pass may already own it.
    for _ in 0..2 {
        let reply = request(&client, Command::QueueDispatch).await.unwrap();
        let Reply::QueueDispatched { placed, .. } = reply else {
            panic!("{reply:?}")
        };
        assert!(placed.is_empty(), "no second reservation: {placed:?}");
    }
    let database = home.database();
    let deadline = Instant::now() + GENEROUS;
    let (read, recovered) = loop {
        let read = database.read().await.unwrap();
        let recovered = cockpit_messaging::envelope(&read, envelope.id)
            .await
            .unwrap()
            .unwrap();
        if recovered.state != DeliveryState::Submitting {
            break (read, recovered);
        }
        assert!(
            Instant::now() < deadline,
            "delivery never finished: {recovered:?}"
        );
        // Release this snapshot so the background pass can commit its receipt.
        drop(read);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        cockpit_scheduling::placements(&read).await.unwrap(),
        std::slice::from_ref(&placement)
    );
    assert_eq!(recovered.effect_key, envelope.effect_key);
    assert_eq!(
        recovered.state,
        if delivered {
            DeliveryState::Uncertain
        } else {
            DeliveryState::Received
        }
    );
    let reservations = cockpit_resource_admission::reservations(&read)
        .await
        .unwrap();
    assert_eq!(reservations.len(), 1);
    assert_eq!(Some(reservations[0].id), placement.reservation);
    assert_eq!(
        reservations[0].state,
        cockpit_resource_admission::ReservationState::Held
    );
    drop(read);
    assert_eq!(task_status(&client, item).await, TaskStatusView::Doing);
    assert_eq!(home.messages_of(&session.id, 1).await, [envelope.payload]);
    assert_eq!(home.launches_of(&session.id), original_launches);
    assert_eq!(recovered.attempts, 1);
    assert_eq!(recovered.receipt.is_some(), !delivered);
}

#[tokio::test(flavor = "multi_thread")]
async fn reservation_survives_restart_before_work_is_launched_without_a_second_reservation() {
    crash_window(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn launched_work_without_a_receipt_is_uncertain_and_keeps_occupancy_without_redelivery() {
    crash_window(true).await;
}
