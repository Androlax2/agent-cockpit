//! The outbox on a real state database: issue #41's duplicate-delivery and lost-ack fixtures,
//! restart, expiry and coalescing.
mod common;

use std::collections::HashMap;
use std::sync::Mutex;

use cockpit_messaging::{
    Claim, Coalescing, Deliverer, Delivery, DeliveryState, Enqueued, Envelope, EnvelopeId, Error,
    Failure, Received, Redelivery, Refusal, Resolution,
};
use cockpit_turso::Database;
use common::{Fixture, NOW, envelope};

async fn enqueue(database: &Database, envelope: &Envelope) -> Enqueued {
    let write = database.write().await.unwrap();
    let enqueued = cockpit_messaging::enqueue(&write, envelope, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
    enqueued
}

fn queued_id(enqueued: Enqueued) -> EnvelopeId {
    match enqueued {
        Enqueued::Queued { id, .. } => id,
        other => panic!("not newly queued: {other:?}"),
    }
}

async fn state(database: &Database, id: EnvelopeId) -> DeliveryState {
    let read = database.read().await.unwrap();
    cockpit_messaging::envelope(&read, id)
        .await
        .unwrap()
        .unwrap()
        .state
}

async fn claim(database: &Database, now: i64) -> Vec<Claim> {
    let write = database.write().await.unwrap();
    let claims = cockpit_messaging::claim_due(&write, "teams", now, 10)
        .await
        .unwrap();
    write.commit().await.unwrap();
    claims
}

async fn record(database: &Database, claim: &Claim, delivery: Delivery) -> DeliveryState {
    let write = database.write().await.unwrap();
    let state = cockpit_messaging::record_delivery(&write, claim.id, claim.attempt, &delivery, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
    state
}

fn lost_ack() -> Delivery {
    Delivery::Failed(Failure::Unacknowledged {
        reason: "the receiver did not answer in time".to_owned(),
        retry_at: NOW,
    })
}

/// A receiver outside Cockpit that applies each effect it is handed and deduplicates on the
/// effect key; `drop_acknowledgements` loses that many answers after applying the effect.
#[derive(Default)]
struct Receiver {
    applied: Mutex<HashMap<String, u32>>,
    deliveries: Mutex<u32>,
    drop_acknowledgements: Mutex<u32>,
    deduplicates: bool,
}

impl Receiver {
    fn deduplicating() -> Receiver {
        Receiver {
            deduplicates: true,
            ..Receiver::default()
        }
    }

    fn losing_acknowledgements(mut self, count: u32) -> Receiver {
        self.drop_acknowledgements = Mutex::new(count);
        self
    }

    fn effects(&self, effect_key: &str) -> u32 {
        self.applied
            .lock()
            .unwrap()
            .get(effect_key)
            .copied()
            .unwrap_or(0)
    }

    fn deliveries(&self) -> u32 {
        *self.deliveries.lock().unwrap()
    }
}

impl Deliverer for Receiver {
    async fn deliver(&self, claim: &Claim) -> Delivery {
        *self.deliveries.lock().unwrap() += 1;
        {
            let mut applied = self.applied.lock().unwrap();
            let effects = applied.entry(claim.effect_key.clone()).or_default();
            if *effects == 0 || !self.deduplicates {
                *effects += 1;
            }
        }
        let mut dropping = self.drop_acknowledgements.lock().unwrap();
        if *dropping > 0 {
            *dropping -= 1;
            return lost_ack();
        }
        Delivery::Received {
            receipt: format!("receipt-of-{}", claim.effect_key),
        }
    }
}

async fn deliver(database: &Database, receiver: &Receiver) -> Vec<(EnvelopeId, DeliveryState)> {
    cockpit_messaging::deliver_due(database, "teams", receiver, NOW, 10)
        .await
        .unwrap()
}

#[tokio::test]
async fn enqueuing_the_same_effect_twice_delivers_it_once() {
    let fixture = Fixture::new().await;
    let decision = envelope("decision-7", "session-a", "approve the plan");
    let id = queued_id(enqueue(&fixture.database, &decision).await);

    let again = enqueue(&fixture.database, &decision).await;
    let receiver = Receiver::default();
    deliver(&fixture.database, &receiver).await;
    deliver(&fixture.database, &receiver).await;

    assert_eq!(
        again,
        Enqueued::AlreadyEnqueued {
            id,
            state: DeliveryState::Queued
        }
    );
    assert_eq!(receiver.deliveries(), 1);
    assert_eq!(state(&fixture.database, id).await, DeliveryState::Received);
}

#[tokio::test]
async fn an_effect_key_reused_for_other_content_is_refused() {
    let fixture = Fixture::new().await;
    enqueue(
        &fixture.database,
        &envelope("decision-7", "session-a", "approve"),
    )
    .await;

    let write = fixture.database.write().await.unwrap();
    let reused =
        cockpit_messaging::enqueue(&write, &envelope("decision-7", "session-a", "decline"), NOW)
            .await;

    assert!(
        matches!(
            reused,
            Err(Error::Refused(Refusal::EffectKeyReused { ref effect_key })) if effect_key == "decision-7"
        ),
        "{reused:?}"
    );
}

#[tokio::test]
async fn a_delivery_received_twice_has_its_effect_once() {
    let fixture = Fixture::new().await;
    let receive = || async {
        let write = fixture.database.write().await.unwrap();
        let received = cockpit_messaging::receive(&write, "chief", "answer-3", "ship it", NOW)
            .await
            .unwrap();
        if let Received::First(_) = received {
            write
                .execute("CREATE TABLE IF NOT EXISTS effects (n INTEGER)", ())
                .await
                .unwrap();
            write
                .execute("INSERT INTO effects VALUES (1)", ())
                .await
                .unwrap();
        }
        write.commit().await.unwrap();
        received
    };

    let first = receive().await;
    let second = receive().await;

    let read = fixture.database.read().await.unwrap();
    let mut rows = read
        .query("SELECT count(*) FROM effects", ())
        .await
        .unwrap();
    let effects: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert!(matches!(first, Received::First(_)), "{first:?}");
    assert_eq!(second, Received::Duplicate(first.receipt()));
    assert_eq!(effects, 1);
}

#[tokio::test]
async fn a_receiver_refuses_the_same_key_with_other_content() {
    let fixture = Fixture::new().await;
    let write = fixture.database.write().await.unwrap();
    cockpit_messaging::receive(&write, "chief", "answer-3", "ship it", NOW)
        .await
        .unwrap();

    let reused = cockpit_messaging::receive(&write, "chief", "answer-3", "hold it", NOW).await;

    assert!(
        matches!(reused, Err(Error::Refused(Refusal::EffectKeyReused { .. }))),
        "{reused:?}"
    );
}

#[tokio::test]
async fn a_lost_acknowledgement_to_a_deduplicating_receiver_is_retried_under_the_same_key() {
    let fixture = Fixture::new().await;
    let mut decision = envelope("decision-8", "session-a", "go");
    decision.redelivery = Redelivery::ReceiverDeduplicates;
    let id = queued_id(enqueue(&fixture.database, &decision).await);
    let receiver = Receiver::deduplicating().losing_acknowledgements(1);

    let first_pass = deliver(&fixture.database, &receiver).await;
    let second_pass = deliver(&fixture.database, &receiver).await;

    assert_eq!(first_pass, [(id, DeliveryState::Queued)]);
    assert_eq!(second_pass, [(id, DeliveryState::Received)]);
    assert_eq!(receiver.deliveries(), 2);
    assert_eq!(
        receiver.effects("decision-8"),
        1,
        "the receiver deduplicated"
    );
}

#[tokio::test]
async fn a_lost_acknowledgement_to_any_other_receiver_is_uncertain_and_never_resent() {
    let fixture = Fixture::new().await;
    let id = queued_id(
        enqueue(
            &fixture.database,
            &envelope("decision-9", "session-a", "go"),
        )
        .await,
    );
    let receiver = Receiver::default().losing_acknowledgements(1);

    let first_pass = deliver(&fixture.database, &receiver).await;
    let second_pass = deliver(&fixture.database, &receiver).await;

    let read = fixture.database.read().await.unwrap();
    let uncertain = cockpit_messaging::uncertain_envelopes(&read).await.unwrap();
    assert_eq!(first_pass, [(id, DeliveryState::Uncertain)]);
    assert_eq!(second_pass, []);
    assert_eq!(receiver.deliveries(), 1);
    assert_eq!(
        uncertain
            .iter()
            .map(|envelope| envelope.id)
            .collect::<Vec<_>>(),
        [id]
    );
}

#[tokio::test]
async fn retries_that_run_out_without_an_acknowledgement_leave_the_delivery_uncertain() {
    let fixture = Fixture::new().await;
    let mut observation = envelope("decision-10", "session-a", "go");
    observation.redelivery = Redelivery::ReceiverDeduplicates;
    observation.max_attempts = 2;
    let id = queued_id(enqueue(&fixture.database, &observation).await);
    let receiver = Receiver::deduplicating().losing_acknowledgements(5);

    deliver(&fixture.database, &receiver).await;
    let last_pass = deliver(&fixture.database, &receiver).await;

    assert_eq!(last_pass, [(id, DeliveryState::Uncertain)]);
    assert_eq!(receiver.deliveries(), 2);
}

#[tokio::test]
async fn a_late_receipt_counts_and_a_stale_failure_changes_nothing() {
    let fixture = Fixture::new().await;
    let mut decision = envelope("decision-11", "session-a", "go");
    decision.redelivery = Redelivery::ReceiverDeduplicates;
    let id = queued_id(enqueue(&fixture.database, &decision).await);
    let first = claim(&fixture.database, NOW).await.remove(0);
    record(&fixture.database, &first, lost_ack()).await;
    let second = claim(&fixture.database, NOW).await.remove(0);

    let stale_failure = record(&fixture.database, &first, lost_ack()).await;
    let late_receipt = record(
        &fixture.database,
        &first,
        Delivery::Received {
            receipt: "r-1".to_owned(),
        },
    )
    .await;
    let after_receipt = record(&fixture.database, &second, lost_ack()).await;

    assert_eq!(second.attempt, 2);
    assert_eq!(stale_failure, DeliveryState::Submitting);
    assert_eq!(late_receipt, DeliveryState::Received);
    assert_eq!(after_receipt, DeliveryState::Received);
    assert_eq!(state(&fixture.database, id).await, DeliveryState::Received);
}

#[tokio::test]
async fn a_delivery_the_receiver_never_got_is_retried_until_attempts_run_out() {
    let fixture = Fixture::new().await;
    let mut decision = envelope("decision-12", "session-a", "go");
    decision.max_attempts = 2;
    let id = queued_id(enqueue(&fixture.database, &decision).await);
    let not_sent = || {
        Delivery::Failed(Failure::NotSent {
            reason: "the session is not running".to_owned(),
            retry_at: NOW + 30,
        })
    };

    let first = claim(&fixture.database, NOW).await.remove(0);
    let after_first = record(&fixture.database, &first, not_sent()).await;
    let too_early = claim(&fixture.database, NOW + 29).await;
    let second = claim(&fixture.database, NOW + 30).await.remove(0);
    let after_second = record(&fixture.database, &second, not_sent()).await;

    assert_eq!(after_first, DeliveryState::Queued);
    assert_eq!(too_early, []);
    assert_eq!(after_second, DeliveryState::Failed);
    assert_eq!(state(&fixture.database, id).await, DeliveryState::Failed);
}

#[tokio::test]
async fn a_restart_during_delivery_retries_only_what_the_receiver_deduplicates() {
    let fixture = Fixture::new().await;
    let decision = queued_id(enqueue(&fixture.database, &envelope("decision-13", "a", "go")).await);
    let mut observation = envelope("observation-1", "a", "idle");
    observation.redelivery = Redelivery::ReceiverDeduplicates;
    let observation = queued_id(enqueue(&fixture.database, &observation).await);
    assert_eq!(claim(&fixture.database, NOW).await.len(), 2);
    let path = fixture.path();
    let Fixture { dir, database } = fixture;
    drop(database);

    let database = Database::open(&path, &[cockpit_messaging::MIGRATIONS])
        .await
        .unwrap();
    let write = database.write().await.unwrap();
    let reconciled = cockpit_messaging::reconcile_deliveries(&write, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(
        reconciled,
        [
            (decision, DeliveryState::Uncertain),
            (observation, DeliveryState::Queued)
        ]
    );
    drop(dir);
}

#[tokio::test]
async fn the_owner_settles_an_uncertain_delivery_once() {
    let fixture = Fixture::new().await;
    let id = queued_id(enqueue(&fixture.database, &envelope("decision-14", "a", "go")).await);
    let receiver = Receiver::default().losing_acknowledgements(1);
    deliver(&fixture.database, &receiver).await;

    let write = fixture.database.write().await.unwrap();
    let resent = cockpit_messaging::resolve_uncertain(&write, id, &Resolution::SendAgain, NOW)
        .await
        .unwrap();
    let twice = cockpit_messaging::resolve_uncertain(&write, id, &Resolution::SendAgain, NOW).await;
    write.commit().await.unwrap();
    let delivered = deliver(&fixture.database, &receiver).await;

    assert_eq!(resent, DeliveryState::Queued);
    assert!(
        matches!(
            twice,
            Err(Error::Refused(Refusal::NotUncertain { state: "queued" }))
        ),
        "{twice:?}"
    );
    assert_eq!(delivered, [(id, DeliveryState::Received)]);
}

#[tokio::test]
async fn an_envelope_past_its_expiry_is_never_sent() {
    let fixture = Fixture::new().await;
    let mut decision = envelope("decision-15", "a", "go");
    decision.expires_at = Some(NOW - 1);
    let id = queued_id(enqueue(&fixture.database, &decision).await);

    let claims = claim(&fixture.database, NOW).await;

    assert_eq!(claims, []);
    assert_eq!(state(&fixture.database, id).await, DeliveryState::Expired);
}

fn observation(effect_key: &str, slot: &str) -> Envelope {
    Envelope {
        coalescing: Coalescing::ReplaceableObservation {
            slot: slot.to_owned(),
        },
        ..envelope(effect_key, "chief", effect_key)
    }
}

#[tokio::test]
async fn a_newer_availability_observation_replaces_the_one_still_waiting() {
    let fixture = Fixture::new().await;
    let older = queued_id(enqueue(&fixture.database, &observation("idle-1", "availability")).await);

    let newer = enqueue(&fixture.database, &observation("idle-2", "availability")).await;
    let claims = claim(&fixture.database, NOW).await;

    assert!(
        matches!(newer, Enqueued::Queued { superseded: Some(id), .. } if id == older),
        "{newer:?}"
    );
    assert_eq!(
        state(&fixture.database, older).await,
        DeliveryState::Superseded
    );
    assert_eq!(
        claims
            .iter()
            .map(|claim| claim.effect_key.as_str())
            .collect::<Vec<_>>(),
        ["idle-2"]
    );
}

#[tokio::test]
async fn decisions_and_observations_already_in_flight_are_never_replaced() {
    let fixture = Fixture::new().await;
    let in_flight =
        queued_id(enqueue(&fixture.database, &observation("idle-1", "availability")).await);
    claim(&fixture.database, NOW).await;
    let decision_a =
        queued_id(enqueue(&fixture.database, &envelope("decision-a", "chief", "a")).await);
    let decision_b =
        queued_id(enqueue(&fixture.database, &envelope("decision-b", "chief", "b")).await);

    let newer = enqueue(&fixture.database, &observation("idle-2", "availability")).await;

    assert!(
        matches!(
            newer,
            Enqueued::Queued {
                superseded: None,
                ..
            }
        ),
        "{newer:?}"
    );
    assert_eq!(
        state(&fixture.database, in_flight).await,
        DeliveryState::Submitting
    );
    assert_eq!(
        state(&fixture.database, decision_a).await,
        DeliveryState::Queued
    );
    assert_eq!(
        state(&fixture.database, decision_b).await,
        DeliveryState::Queued
    );
}

#[tokio::test]
async fn an_observation_sent_back_for_a_retry_gives_way_to_a_newer_one() {
    let fixture = Fixture::new().await;
    let older = queued_id(enqueue(&fixture.database, &observation("idle-1", "availability")).await);
    let claims = claim(&fixture.database, NOW).await;
    let newer = queued_id(enqueue(&fixture.database, &observation("idle-2", "availability")).await);
    let not_sent = Delivery::Failed(Failure::NotSent {
        reason: "the chief is not running".to_owned(),
        retry_at: NOW,
    });

    let recorded = record(&fixture.database, &claims[0], not_sent).await;
    let read = fixture.database.read().await.unwrap();
    let stored = cockpit_messaging::envelope(&read, older)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(recorded, DeliveryState::Superseded);
    assert_eq!(stored.superseded_by, Some(newer));
}

/// Answers every delivery received, but with an empty receipt for `bad_key`.
struct EmptyReceiptFor {
    bad_key: &'static str,
}

impl Deliverer for EmptyReceiptFor {
    async fn deliver(&self, claim: &Claim) -> Delivery {
        let receipt = if claim.effect_key == self.bad_key {
            String::new()
        } else {
            format!("receipt-{}", claim.effect_key)
        };
        Delivery::Received { receipt }
    }
}

#[tokio::test]
async fn an_outcome_that_cannot_be_recorded_leaves_the_other_deliveries_recorded() {
    let fixture = Fixture::new().await;
    let first = queued_id(enqueue(&fixture.database, &envelope("first", "chief", "a")).await);
    let bad = queued_id(enqueue(&fixture.database, &envelope("bad", "chief", "b")).await);
    let last = queued_id(enqueue(&fixture.database, &envelope("last", "chief", "c")).await);

    let pass = cockpit_messaging::deliver_due(
        &fixture.database,
        "teams",
        &EmptyReceiptFor { bad_key: "bad" },
        NOW,
        10,
    )
    .await;

    assert!(pass.is_err(), "{pass:?}");
    assert_eq!(
        state(&fixture.database, first).await,
        DeliveryState::Received
    );
    assert_eq!(
        state(&fixture.database, bad).await,
        DeliveryState::Submitting
    );
    assert_eq!(
        state(&fixture.database, last).await,
        DeliveryState::Received
    );
}

#[tokio::test]
async fn a_worker_claims_only_its_own_producers_envelopes() {
    let fixture = Fixture::new().await;
    let ours = queued_id(enqueue(&fixture.database, &envelope("ours", "chief", "a")).await);
    let theirs = queued_id(
        enqueue(
            &fixture.database,
            &Envelope {
                producer: "scheduling".to_owned(),
                ..envelope("theirs", "chief", "b")
            },
        )
        .await,
    );

    let claimed: Vec<EnvelopeId> = claim(&fixture.database, NOW)
        .await
        .into_iter()
        .map(|claim| claim.id)
        .collect();

    assert_eq!(claimed, vec![ours]);
    assert_eq!(
        state(&fixture.database, theirs).await,
        DeliveryState::Queued
    );
}

#[tokio::test]
async fn an_earlier_queued_envelope_to_the_same_destination_holds_a_later_one() {
    let fixture = Fixture::new().await;
    let database = &fixture.database;
    enqueue(database, &envelope("first", "s1", "A")).await;
    enqueue(database, &envelope("other", "s2", "C")).await;
    enqueue(database, &envelope("second", "s1", "B")).await;
    let claims = claim(database, NOW).await;
    let [first, other, second] = claims.as_slice() else {
        panic!("{claims:?}");
    };
    record(
        database,
        first,
        Delivery::Failed(Failure::NotSent {
            reason: "busy".to_owned(),
            retry_at: NOW,
        }),
    )
    .await;
    let read = database.read().await.unwrap();

    let is_second_held = cockpit_messaging::has_earlier_queued(&read, second)
        .await
        .unwrap();
    let is_other_held = cockpit_messaging::has_earlier_queued(&read, other)
        .await
        .unwrap();
    let is_first_held = cockpit_messaging::has_earlier_queued(&read, first)
        .await
        .unwrap();

    assert!(is_second_held);
    assert!(!is_other_held);
    assert!(!is_first_held);
}
