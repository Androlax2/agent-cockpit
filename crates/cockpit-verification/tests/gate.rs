//! Which receipts apply to an exact result, and the outcome a gate of required checks has.
mod common;

use cockpit_verification::{CheckStatus, GateOutcome, Outcome, Source, StaleReason};
use common::{Fixture, check, claim, clean_source};

#[tokio::test]
async fn an_edited_file_makes_its_receipt_stale() {
    let fixture = Fixture::new().await;
    fixture.write_file("src/lib.rs", "pub fn answer() -> u8 { 42 }\n");
    let tests = check("test");
    fixture
        .record(fixture.receipt(
            "run-1",
            &tests,
            &clean_source(),
            &["src/lib.rs"],
            Outcome::Passed,
        ))
        .await
        .unwrap();
    let before = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(std::slice::from_ref(&tests), &fixture.checkout())
        .unwrap();
    assert_eq!(
        before.checks[0].status,
        CheckStatus::Checked(Outcome::Passed)
    );

    fixture.write_file("src/lib.rs", "pub fn answer() -> u8 { 41 }\n");

    let after = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[tests], &fixture.checkout())
        .unwrap();
    assert_eq!(
        after.checks[0].status,
        CheckStatus::Stale(StaleReason::ArtifactChanged {
            path: "src/lib.rs".to_owned()
        })
    );
    assert_eq!(after.outcome, GateOutcome::Open);
}

#[tokio::test]
async fn a_removed_file_makes_its_receipt_stale() {
    let fixture = Fixture::new().await;
    fixture.write_file("docs/report.md", "# Report\n");
    let tests = check("test");
    fixture
        .record(fixture.receipt(
            "run-1",
            &tests,
            &clean_source(),
            &["docs/report.md"],
            Outcome::Passed,
        ))
        .await
        .unwrap();
    std::fs::remove_file(fixture.checkout().join("docs/report.md")).unwrap();

    let report = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[tests], &fixture.checkout())
        .unwrap();
    assert_eq!(
        report.checks[0].status,
        CheckStatus::Stale(StaleReason::ArtifactMissing {
            path: "docs/report.md".to_owned()
        })
    );
}

#[tokio::test]
async fn a_changed_check_definition_makes_its_receipts_stale() {
    let recorded = check("test");
    let mut other_command = recorded.clone();
    other_command.command = "make test-all".to_owned();
    let mut other_directory = recorded.clone();
    other_directory.working_directory = "crates".to_owned();
    let mut other_environment = recorded.clone();
    other_environment.environment = "sandbox:default".to_owned();
    for (case, current) in [
        ("command", other_command),
        ("working directory", other_directory),
        ("environment", other_environment),
    ] {
        let fixture = Fixture::new().await;
        fixture
            .record(fixture.receipt("run-1", &recorded, &clean_source(), &[], Outcome::Passed))
            .await
            .unwrap();
        let report = fixture
            .evidence(&clean_source(), &["test"])
            .await
            .evaluate(&[current], &fixture.checkout())
            .unwrap();
        assert_eq!(
            report.checks[0].status,
            CheckStatus::Stale(StaleReason::CheckChanged),
            "{case}"
        );
    }
}

#[tokio::test]
async fn a_receipt_of_another_head_or_dirty_state_does_not_apply() {
    let dirty = Source {
        dirty: Some("a1".repeat(32)),
        ..clean_source()
    };
    let other_dirty = Source {
        dirty: Some("b2".repeat(32)),
        ..clean_source()
    };
    let other_head = Source {
        head: "9".repeat(40),
        ..clean_source()
    };
    for (case, recorded, current) in [
        ("other head", clean_source(), other_head),
        ("now dirty", clean_source(), dirty.clone()),
        ("now clean", dirty.clone(), clean_source()),
        ("other changes", dirty, other_dirty),
    ] {
        let fixture = Fixture::new().await;
        let tests = check("test");
        fixture
            .record(fixture.receipt("run-1", &tests, &recorded, &[], Outcome::Passed))
            .await
            .unwrap();
        let report = fixture
            .evidence(&current, &["test"])
            .await
            .evaluate(&[tests], &fixture.checkout())
            .unwrap();
        assert_eq!(
            report.checks[0].status,
            CheckStatus::Stale(StaleReason::SourceChanged {
                recorded: recorded.clone()
            }),
            "{case}"
        );
    }
}

/// The base is reported, not matched: the same head is the same code whatever it was
/// compared against.
#[tokio::test]
async fn a_receipt_applies_whatever_base_it_named() {
    let fixture = Fixture::new().await;
    let tests = check("test");
    fixture
        .record(fixture.receipt("run-1", &tests, &clean_source(), &[], Outcome::Passed))
        .await
        .unwrap();
    let rebased_base = Source {
        base: None,
        ..clean_source()
    };
    let report = fixture
        .evidence(&rebased_base, &["test"])
        .await
        .evaluate(&[tests], &fixture.checkout())
        .unwrap();
    assert_eq!(
        report.checks[0].status,
        CheckStatus::Checked(Outcome::Passed)
    );
}

#[tokio::test]
async fn the_newest_receipt_that_applies_decides() {
    let fixture = Fixture::new().await;
    let tests = check("test");
    let other_head = Source {
        head: "9".repeat(40),
        ..clean_source()
    };
    let failed = fixture
        .record(fixture.receipt("run-1", &tests, &clean_source(), &[], Outcome::Failed))
        .await
        .unwrap();
    let passed = fixture
        .record(fixture.receipt("run-2", &tests, &clean_source(), &[], Outcome::Passed))
        .await
        .unwrap();
    fixture
        .record(fixture.receipt("run-3", &tests, &other_head, &[], Outcome::Failed))
        .await
        .unwrap();

    let report = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[tests], &fixture.checkout())
        .unwrap();
    assert_eq!(
        report.checks[0].status,
        CheckStatus::Checked(Outcome::Passed)
    );
    assert_eq!(report.checks[0].receipt, Some(passed.id));
    assert_ne!(report.checks[0].receipt, Some(failed.id));
}

#[tokio::test]
async fn a_prose_claim_alone_is_unverified() {
    let fixture = Fixture::new().await;
    fixture.claim(claim("test")).await.unwrap();

    let report = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[check("test")], &fixture.checkout())
        .unwrap();
    assert_eq!(report.checks[0].status, CheckStatus::Unverified);
    assert_eq!(report.outcome, GateOutcome::Open);
}

#[tokio::test]
async fn a_claim_after_a_stale_receipt_leaves_it_stale() {
    let fixture = Fixture::new().await;
    fixture.write_file("src/lib.rs", "one\n");
    let tests = check("test");
    fixture
        .record(fixture.receipt(
            "run-1",
            &tests,
            &clean_source(),
            &["src/lib.rs"],
            Outcome::Passed,
        ))
        .await
        .unwrap();
    fixture.write_file("src/lib.rs", "two\n");
    fixture.claim(claim("test")).await.unwrap();

    let report = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[tests], &fixture.checkout())
        .unwrap();
    assert!(
        matches!(report.checks[0].status, CheckStatus::Stale(_)),
        "{:?}",
        report.checks[0].status
    );
}

#[tokio::test]
async fn a_check_without_evidence_is_missing() {
    let fixture = Fixture::new().await;
    let report = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[check("test")], &fixture.checkout())
        .unwrap();
    assert_eq!(report.checks[0].status, CheckStatus::Missing);
    assert_eq!(report.checks[0].receipt, None);
}

#[tokio::test]
async fn another_projects_receipts_are_not_evidence_here() {
    let fixture = Fixture::new().await;
    let tests = check("test");
    let mut elsewhere = fixture.receipt("run-1", &tests, &clean_source(), &[], Outcome::Passed);
    elsewhere.project = "dotfiles".to_owned();
    fixture.record(elsewhere).await.unwrap();

    let report = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[tests], &fixture.checkout())
        .unwrap();
    assert_eq!(report.checks[0].status, CheckStatus::Missing);
}

#[tokio::test]
async fn the_gate_passes_only_when_every_required_check_passed() {
    for (lint_outcome, expected) in [
        (Outcome::Passed, GateOutcome::Passed),
        (Outcome::Failed, GateOutcome::Failed),
        (Outcome::Unavailable, GateOutcome::Open),
        (Outcome::Blocked, GateOutcome::Open),
        (Outcome::Cancelled, GateOutcome::Open),
        (Outcome::Unknown, GateOutcome::Open),
    ] {
        let fixture = Fixture::new().await;
        let (tests, lint) = (check("test"), check("lint"));
        fixture
            .record(fixture.receipt("run-1", &tests, &clean_source(), &[], Outcome::Passed))
            .await
            .unwrap();
        fixture
            .record(fixture.receipt("run-2", &lint, &clean_source(), &[], lint_outcome))
            .await
            .unwrap();

        let report = fixture
            .evidence(&clean_source(), &["test", "lint"])
            .await
            .evaluate(&[tests, lint], &fixture.checkout())
            .unwrap();
        assert_eq!(report.outcome, expected, "{lint_outcome:?}");
        assert_eq!(report.checks[1].status, CheckStatus::Checked(lint_outcome));
    }
}

#[tokio::test]
async fn a_failed_check_fails_the_gate_even_when_another_is_missing() {
    let fixture = Fixture::new().await;
    let tests = check("test");
    fixture
        .record(fixture.receipt("run-1", &tests, &clean_source(), &[], Outcome::Failed))
        .await
        .unwrap();
    let report = fixture
        .evidence(&clean_source(), &["test", "lint"])
        .await
        .evaluate(&[tests, check("lint")], &fixture.checkout())
        .unwrap();
    assert_eq!(report.outcome, GateOutcome::Failed);
}

#[tokio::test]
async fn every_check_that_did_not_pass_names_its_next_action() {
    let fixture = Fixture::new().await;
    fixture.write_file("src/lib.rs", "one\n");
    let (failed, stale, unverified, missing) = (
        check("failed"),
        check("stale"),
        check("unverified"),
        check("missing"),
    );
    fixture
        .record(fixture.receipt("run-1", &failed, &clean_source(), &[], Outcome::Failed))
        .await
        .unwrap();
    fixture
        .record(fixture.receipt(
            "run-2",
            &stale,
            &clean_source(),
            &["src/lib.rs"],
            Outcome::Passed,
        ))
        .await
        .unwrap();
    fixture.write_file("src/lib.rs", "two\n");
    fixture.claim(claim("unverified")).await.unwrap();

    let report = fixture
        .evidence(
            &clean_source(),
            &["failed", "stale", "unverified", "missing"],
        )
        .await
        .evaluate(&[failed, stale, unverified, missing], &fixture.checkout())
        .unwrap();
    for check_report in &report.checks {
        let next_action = check_report.next_action().unwrap_or_default();
        assert!(
            next_action.contains(&format!("`{}`", check_report.name)),
            "{}: {next_action:?}",
            check_report.name
        );
    }
}

#[tokio::test]
async fn a_passed_check_needs_no_action() {
    let fixture = Fixture::new().await;
    let tests = check("test");
    fixture
        .record(fixture.receipt("run-1", &tests, &clean_source(), &[], Outcome::Passed))
        .await
        .unwrap();
    let report = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[tests], &fixture.checkout())
        .unwrap();
    assert_eq!(report.checks[0].next_action(), None);
}

#[tokio::test]
async fn a_gate_without_checks_or_with_a_check_twice_is_refused() {
    let fixture = Fixture::new().await;
    let evidence = fixture.evidence(&clean_source(), &["test"]).await;
    for (case, checks) in [
        ("empty", Vec::new()),
        ("twice", vec![check("test"), check("test")]),
    ] {
        let refused = evidence.evaluate(&checks, &fixture.checkout());
        assert!(
            matches!(
                refused,
                Err(cockpit_verification::Refusal::InvalidRequest {
                    field: "checks",
                    ..
                })
            ),
            "{case}: {refused:?}"
        );
    }
}

#[tokio::test]
async fn a_gate_asking_for_evidence_it_was_not_loaded_with_is_refused() {
    let fixture = Fixture::new().await;
    let evidence = fixture.evidence(&clean_source(), &["test"]).await;
    let refused = evidence.evaluate(&[check("lint")], &fixture.checkout());
    assert!(
        matches!(
            refused,
            Err(cockpit_verification::Refusal::InvalidRequest {
                field: "checks",
                ..
            })
        ),
        "{refused:?}"
    );
}

/// A newer run of the same check on the same source decides even once its artifacts
/// changed: an older pass does not stand in for a newer failure.
#[tokio::test]
async fn an_older_pass_does_not_stand_in_for_a_newer_stale_failure() {
    let fixture = Fixture::new().await;
    fixture.write_file("target/generated.rs", "one\n");
    let tests = check("test");
    fixture
        .record(fixture.receipt("run-1", &tests, &clean_source(), &[], Outcome::Passed))
        .await
        .unwrap();
    fixture
        .record(fixture.receipt(
            "run-2",
            &tests,
            &clean_source(),
            &["target/generated.rs"],
            Outcome::Failed,
        ))
        .await
        .unwrap();
    fixture.write_file("target/generated.rs", "two\n");

    let report = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[tests], &fixture.checkout())
        .unwrap();
    assert_eq!(
        report.checks[0].status,
        CheckStatus::Stale(StaleReason::ArtifactChanged {
            path: "target/generated.rs".to_owned()
        })
    );
}

/// Parallel clones record the same check on other heads; they do not push this source's
/// receipt out of the ones loaded.
#[tokio::test]
async fn receipts_of_other_sources_do_not_crowd_out_this_ones() {
    let fixture = Fixture::new().await;
    let tests = check("test");
    let passed = fixture
        .record(fixture.receipt("run-mine", &tests, &clean_source(), &[], Outcome::Passed))
        .await
        .unwrap();
    for clone in 0..cockpit_verification::RECEIPTS_LOADED_PER_CHECK + 5 {
        let elsewhere = Source {
            head: format!("{clone:040}"),
            ..clean_source()
        };
        fixture
            .record(fixture.receipt(
                &format!("run-{clone}"),
                &tests,
                &elsewhere,
                &[],
                Outcome::Failed,
            ))
            .await
            .unwrap();
    }

    let report = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[tests], &fixture.checkout())
        .unwrap();
    assert_eq!(report.checks[0].receipt, Some(passed.id));
}

#[tokio::test]
async fn a_claim_about_another_head_does_not_show_here() {
    let fixture = Fixture::new().await;
    let mut old_claim = claim("test");
    old_claim.source.head = "9".repeat(40);
    fixture.claim(old_claim).await.unwrap();

    let report = fixture
        .evidence(&clean_source(), &["test"])
        .await
        .evaluate(&[check("test")], &fixture.checkout())
        .unwrap();
    assert_eq!(report.checks[0].status, CheckStatus::Missing);
}

#[tokio::test]
async fn evaluating_in_a_checkout_that_is_gone_is_refused() {
    let fixture = Fixture::new().await;
    let evidence = fixture.evidence(&clean_source(), &["test"]).await;
    let refused = evidence.evaluate(&[check("test")], &fixture.dir.path().join("gone"));
    assert!(
        matches!(
            refused,
            Err(cockpit_verification::Refusal::InvalidRequest {
                field: "checkout",
                ..
            })
        ),
        "{refused:?}"
    );
}
