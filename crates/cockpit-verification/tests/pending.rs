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

#[tokio::test]
async fn a_sealed_result_keeps_where_its_intent_said_it_ran() {
    let fixture = Fixture::new().await;
    let new = intent(&fixture);
    let write = fixture.database.write().await.unwrap();
    let pending = reserve_result(&write, fixture.database.epoch(), 1, new.clone())
        .await
        .unwrap();
    write.commit().await.unwrap();
    let mut result = new.clone();
    result.finished_at = NOW;
    result.outcome = Outcome::Passed;
    result.exit_code = Some(0);

    let write = fixture.database.write().await.unwrap();
    let mut elsewhere = result.clone();
    elsewhere.ran = Some(Ran {
        environment: "native".to_owned(),
        isolation: "no sandbox".to_owned(),
    });
    assert!(
        seal_result(&write, &pending, elsewhere, 0, NOW, false)
            .await
            .is_err(),
        "a result cannot say it ran somewhere its intent did not"
    );
    let mut passed_nowhere = result.clone();
    passed_nowhere.ran = None;
    assert!(
        seal_result(&write, &pending, passed_nowhere, 0, NOW, false)
            .await
            .is_err(),
        "a run that never ran cannot pass"
    );
    let recorded = seal_result(&write, &pending, result, 0, NOW, false)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(recorded.recorded.ran, Some(sandboxed()));
    let read = fixture.database.read().await.unwrap();
    let reread = pending_result(&read, PROJECT, &new.run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reread.receipt.ran, Some(sandboxed()));
}

#[tokio::test]
async fn a_run_whose_environment_never_started_is_sealed_unavailable_and_ran_nowhere() {
    let fixture = Fixture::new().await;
    let new = intent(&fixture);
    let write = fixture.database.write().await.unwrap();
    let pending = reserve_result(&write, fixture.database.epoch(), 1, new.clone())
        .await
        .unwrap();
    let mut result = new;
    result.finished_at = NOW;
    result.outcome = Outcome::Unavailable;
    result.ran = None;
    result.output = "the sandbox could not start".into();

    let recorded = seal_result(&write, &pending, result, 27, NOW, false)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(
        (recorded.recorded.outcome, recorded.recorded.ran),
        (Outcome::Unavailable, None)
    );
}

fn intent_of(fixture: &Fixture, check_name: &str, execution: i64) -> NewReceipt {
    let mut receipt = fixture.receipt(
        &format!("execution-{}-{execution}", fixture.database.epoch()),
        &check(check_name),
        &clean_source(),
        &[],
        Outcome::Unknown,
    );
    receipt.output.clear();
    receipt.finished_at = receipt.started_at;
    receipt
}

#[tokio::test]
async fn each_check_keeps_its_newest_run_and_its_newest_finished_run() {
    let fixture = Fixture::new().await;
    let epoch = fixture.database.epoch().to_owned();
    let write = fixture.database.write().await.unwrap();
    for (check_name, execution) in [("test", 1), ("lint", 2), ("test", 3), ("test", 4)] {
        reserve_result(
            &write,
            &epoch,
            execution,
            intent_of(&fixture, check_name, execution),
        )
        .await
        .unwrap();
    }
    let finished = pending_result(&write, PROJECT, &format!("execution-{epoch}-3"))
        .await
        .unwrap()
        .unwrap();
    let mut result = finished.receipt.clone();
    result.outcome = Outcome::Passed;
    result.exit_code = Some(0);
    result.finished_at = NOW;
    seal_result(&write, &finished, result, 0, NOW, false)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let read = fixture.database.read().await.unwrap();
    let mut newest = newest_runs(&read, &epoch).await.unwrap();
    newest.sort_unstable();
    let mut one_by_one = Vec::new();
    for execution in 1..=4 {
        if is_newest_run(&read, &epoch, execution).await.unwrap() {
            one_by_one.push(execution);
        }
    }

    assert_eq!(newest, vec![2, 3, 4]);
    assert_eq!(one_by_one, newest, "the one-run check agrees with the list");
}

#[tokio::test]
async fn a_run_of_another_epoch_is_not_this_epochs_newest() {
    let fixture = Fixture::new().await;
    let write = fixture.database.write().await.unwrap();
    reserve_result(&write, fixture.database.epoch(), 1, intent(&fixture))
        .await
        .unwrap();
    write.commit().await.unwrap();

    let newest = newest_runs(&fixture.database.read().await.unwrap(), "another-epoch")
        .await
        .unwrap();

    assert!(newest.is_empty(), "{newest:?}");
}
