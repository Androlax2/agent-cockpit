mod common;
use cockpit_verification::*;
use common::*;

fn intent(fixture: &Fixture) -> NewReceipt {
    let mut receipt = fixture.receipt(
        &format!("execution-{}-1", fixture.database.epoch()),
        &check("test"),
        &clean_source(),
        &[],
        Outcome::Unknown,
    );
    receipt.output.clear();
    receipt.finished_at = receipt.started_at;
    receipt
}

#[tokio::test]
async fn pending_intent_survives_and_only_its_revision_can_seal_without_weakening_replays() {
    let fixture = Fixture::new().await;
    let new = intent(&fixture);
    let write = fixture.database.write().await.unwrap();
    let pending = reserve_result(&write, fixture.database.epoch(), 1, new.clone())
        .await
        .unwrap();
    write.commit().await.unwrap();
    let read = fixture.database.read().await.unwrap();
    assert_eq!(
        pending_result(&read, PROJECT, &new.run_id).await.unwrap(),
        Some(pending.clone())
    );
    assert_eq!(receipt(&read, PROJECT, &new.run_id).await.unwrap(), None);
    drop(read);
    let mut result = new.clone();
    result.finished_at = NOW;
    result.outcome = Outcome::Passed;
    result.exit_code = Some(0);
    result.output = "ok".into();
    let write = fixture.database.write().await.unwrap();
    assert!(
        record_receipt(&write, result.clone(), NOW).await.is_err(),
        "unfenced recording must not bypass a pending producer"
    );
    let mut stale = pending.clone();
    stale.revision += 1;
    assert!(
        seal_result(&write, &stale, result.clone(), 2, NOW, false)
            .await
            .is_err()
    );
    let recorded = seal_result(&write, &pending, result.clone(), 2, NOW, false)
        .await
        .unwrap();
    write.commit().await.unwrap();
    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        seal_result(&write, &pending, result.clone(), 2, NOW, false)
            .await
            .unwrap(),
        recorded
    );
    assert_eq!(
        record_receipt(&write, result.clone(), NOW).await.unwrap(),
        recorded
    );
    result.output = "changed".into();
    assert!(matches!(
        record_receipt(&write, result, NOW).await,
        Err(Error::Refused(Refusal::RunRecordedDifferently { .. }))
    ));
}

#[tokio::test]
async fn recovery_cannot_record_success_and_a_late_result_cannot_replace_unknown() {
    let fixture = Fixture::new().await;
    let new = intent(&fixture);
    let write = fixture.database.write().await.unwrap();
    let pending = reserve_result(&write, fixture.database.epoch(), 1, new.clone())
        .await
        .unwrap();
    write.commit().await.unwrap();
    let mut result = new;
    result.finished_at = NOW;
    result.outcome = Outcome::Passed;
    result.exit_code = Some(0);
    let write = fixture.database.write().await.unwrap();
    assert!(
        seal_result(&write, &pending, result.clone(), 0, NOW, true)
            .await
            .is_err()
    );
    let mut recovered = result.clone();
    recovered.outcome = Outcome::Unknown;
    recovered.exit_code = None;
    let record = seal_result(&write, &pending, recovered, 0, NOW, true)
        .await
        .unwrap();
    write.commit().await.unwrap();
    let write = fixture.database.write().await.unwrap();
    assert!(
        seal_result(&write, &pending, result.clone(), 0, NOW, false)
            .await
            .is_err()
    );
    assert!(record_receipt(&write, result, NOW).await.is_err());
    assert_eq!(
        receipt(&write, PROJECT, &record.recorded.run_id)
            .await
            .unwrap(),
        Some(record)
    );
}
