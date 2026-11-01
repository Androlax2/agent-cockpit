//! Recording receipts and claims on a real state database: bounded output, duplicate
//! delivery, restart, and the requests refused at the boundary.
mod common;

use cockpit_turso::Database;
use cockpit_verification::{CheckStatus, OUTPUT_MAX_BYTES, Outcome, Refusal};
use common::{Fixture, NOW, check, claim, clean_source, refusal, refused_field};

#[tokio::test]
async fn a_receipt_keeps_what_was_checked() {
    let fixture = Fixture::new().await;
    fixture.write_file("src/lib.rs", "pub fn answer() {}\n");
    fixture.write_file("Cargo.lock", "# lock\n");
    let tests = check("test");
    let mut new = fixture.receipt(
        "run-1",
        &tests,
        &clean_source(),
        &["src/lib.rs", "Cargo.lock"],
        Outcome::Failed,
    );
    new.output_references = vec![
        "logs/run-1.log".to_owned(),
        "logs/run-1.junit.xml".to_owned(),
    ];
    let recorded = fixture.record(new.clone()).await.unwrap();

    let evidence = fixture.evidence(&clean_source(), &[tests]).await;
    let stored = evidence.receipt("test").unwrap();
    assert_eq!(stored, &recorded);
    assert_eq!(stored.recorded, new);
    assert_eq!(stored.recorded_at, NOW);
    let paths: Vec<&str> = stored
        .recorded
        .artifacts
        .iter()
        .map(|artifact| artifact.path.as_str())
        .collect();
    assert_eq!(paths, ["Cargo.lock", "src/lib.rs"]);
}

#[tokio::test]
async fn long_output_keeps_its_tail_and_its_full_size() {
    let fixture = Fixture::new().await;
    let mut new = fixture.receipt(
        "run-1",
        &check("test"),
        &clean_source(),
        &[],
        Outcome::Failed,
    );
    new.output = format!(
        "{}é{}the failing line\n",
        "x".repeat(OUTPUT_MAX_BYTES),
        "y".repeat(10)
    );
    let full_size = new.output.len() as u64;

    let recorded = fixture.record(new).await.unwrap();
    assert!(recorded.recorded.output.len() <= OUTPUT_MAX_BYTES);
    assert!(recorded.recorded.output.ends_with("the failing line\n"));
    assert_eq!(recorded.output_bytes, full_size);
    assert!(recorded.is_output_truncated());
}

#[tokio::test]
async fn short_output_is_kept_whole() {
    let fixture = Fixture::new().await;
    let new = fixture.receipt(
        "run-1",
        &check("test"),
        &clean_source(),
        &[],
        Outcome::Passed,
    );
    let recorded = fixture.record(new.clone()).await.unwrap();
    assert_eq!(recorded.recorded.output, new.output);
    assert!(!recorded.is_output_truncated());
}

#[tokio::test]
async fn the_same_run_delivered_twice_is_one_receipt() {
    let fixture = Fixture::new().await;
    let new = fixture.receipt(
        "run-1",
        &check("test"),
        &clean_source(),
        &[],
        Outcome::Passed,
    );
    let first = fixture.record(new.clone()).await.unwrap();
    let second = fixture.record(new).await.unwrap();

    assert_eq!(first, second);
    let read = fixture.database.read().await.unwrap();
    let stored = read
        .prepare("SELECT count(*) FROM verification_receipts")
        .await
        .unwrap()
        .query_row(())
        .await
        .unwrap()
        .get::<i64>(0)
        .unwrap();
    assert_eq!(stored, 1);
}

#[tokio::test]
async fn a_run_delivered_again_with_another_result_is_refused() {
    let fixture = Fixture::new().await;
    let tests = check("test");
    let first = fixture
        .record_run("run-1", &tests, &clean_source(), &[], Outcome::Failed)
        .await;
    let rewritten = fixture.receipt("run-1", &tests, &clean_source(), &[], Outcome::Passed);

    assert_eq!(
        refusal(fixture.record(rewritten).await),
        Refusal::RunRecordedDifferently {
            run_id: "run-1".to_owned(),
            receipt: first.id,
        }
    );
}

#[tokio::test]
async fn receipts_and_claims_survive_a_restart() {
    let fixture = Fixture::new().await;
    fixture.write_file("src/lib.rs", "one\n");
    let tests = check("test");
    fixture
        .record_run(
            "run-1",
            &tests,
            &clean_source(),
            &["src/lib.rs"],
            Outcome::Passed,
        )
        .await;
    fixture.claim(claim("lint")).await.unwrap();
    let path = fixture.dir.path().join("state.db");
    let common::Fixture { dir, database } = fixture;
    drop(database);

    let reopened = Database::open(&path, &[cockpit_verification::MIGRATIONS])
        .await
        .unwrap();
    let read = reopened.read().await.unwrap();
    let evidence = cockpit_verification::evidence(
        &read,
        common::PROJECT,
        &clean_source(),
        &[tests, check("lint")],
    )
    .await
    .unwrap();
    let report = evidence.evaluate(&dir.path().join("checkout")).unwrap();
    assert_eq!(
        report.checks[0].status,
        CheckStatus::Checked(Outcome::Passed)
    );
    assert_eq!(report.checks[1].status, CheckStatus::Unverified);
    assert_eq!(evidence.claims("lint")[0].statement, "All tests pass.");
}

#[tokio::test]
async fn an_exit_code_contradicting_the_outcome_is_refused() {
    for (outcome, exit_code) in [
        (Outcome::Passed, Some(1)),
        (Outcome::Passed, None),
        (Outcome::Failed, Some(0)),
    ] {
        let fixture = Fixture::new().await;
        let mut new = fixture.receipt("run-1", &check("test"), &clean_source(), &[], outcome);
        new.exit_code = exit_code;
        assert_eq!(
            refused_field(refusal(fixture.record(new).await)),
            "exit_code",
            "{outcome:?} {exit_code:?}"
        );
    }
}

#[tokio::test]
async fn an_invalid_receipt_is_refused_naming_its_field() {
    type Change = fn(&mut cockpit_verification::NewReceipt);
    let cases: [(&str, Change); 11] = [
        ("project", |new| new.project = String::new()),
        ("run_id", |new| new.run_id = " ".to_owned()),
        ("check.name", |new| new.check.name = String::new()),
        ("check.command", |new| new.check.command = String::new()),
        ("check.working_directory", |new| {
            new.check.working_directory = "/etc".to_owned()
        }),
        ("check.working_directory", |new| {
            new.check.working_directory = "../elsewhere".to_owned()
        }),
        ("check.working_directory", |new| {
            new.check.working_directory = "crates/x/".to_owned()
        }),
        ("check.environment", |new| {
            new.check.environment = String::new()
        }),
        ("source.head", |new| new.source.head = String::new()),
        ("finished_at", |new| new.finished_at = new.started_at - 1),
        ("output_references", |new| {
            new.output_references = vec![String::new()]
        }),
    ];
    for (field, change) in cases {
        let fixture = Fixture::new().await;
        let mut new = fixture.receipt(
            "run-1",
            &check("test"),
            &clean_source(),
            &[],
            Outcome::Passed,
        );
        change(&mut new);
        match refusal(fixture.record(new).await) {
            Refusal::InvalidRequest { field: refused, .. } => assert_eq!(refused, field),
            other => panic!("{field}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn an_artifact_named_twice_or_outside_the_checkout_is_refused() {
    let fixture = Fixture::new().await;
    fixture.write_file("src/lib.rs", "one\n");
    let mut new = fixture.receipt(
        "run-1",
        &check("test"),
        &clean_source(),
        &["src/lib.rs"],
        Outcome::Passed,
    );
    new.artifacts.push(new.artifacts[0].clone());
    assert_eq!(
        refused_field(refusal(fixture.record(new.clone()).await)),
        "artifacts"
    );
    new.artifacts.truncate(1);
    new.artifacts[0].path = "../secrets".to_owned();
    assert_eq!(
        refused_field(refusal(fixture.record(new).await)),
        "artifacts"
    );
}

#[tokio::test]
async fn fingerprinting_refuses_what_is_not_a_file_in_the_checkout() {
    let fixture = Fixture::new().await;
    fixture.write_file("src/lib.rs", "one\n");
    for path in [
        "missing.rs",
        "src",
        "/etc/hostname",
        "src/../../state.db",
        "",
        "src/./lib.rs",
        "src//lib.rs",
        "src/lib.rs/",
    ] {
        let refused = cockpit_verification::fingerprint(&fixture.checkout(), &[path]);
        assert_eq!(refused_field(refused.unwrap_err()), "artifacts", "{path}");
    }
}

#[tokio::test]
async fn an_empty_claim_is_refused() {
    let fixture = Fixture::new().await;
    let mut empty = claim("test");
    empty.statement = "  ".to_owned();
    assert_eq!(
        refused_field(refusal(fixture.claim(empty).await)),
        "statement"
    );
}

#[tokio::test]
async fn fingerprinting_refuses_a_path_named_twice_or_a_missing_checkout() {
    let fixture = Fixture::new().await;
    fixture.write_file("src/lib.rs", "one\n");
    let twice =
        cockpit_verification::fingerprint(&fixture.checkout(), &["src/lib.rs", "src/lib.rs"]);
    assert_eq!(refused_field(twice.unwrap_err()), "artifacts");
    let nowhere =
        cockpit_verification::fingerprint(&fixture.dir.path().join("gone"), &["src/lib.rs"]);
    assert_eq!(refused_field(nowhere.unwrap_err()), "checkout");
}
