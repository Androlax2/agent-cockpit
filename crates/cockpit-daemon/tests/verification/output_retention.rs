//! #406: check-output retention through the daemon's owner socket. Receipts and their tails
//! stay; only the files of old or over-budget runs go, never a check's newest run.
use super::*;

/// One MiB per project, no grace: three 600 kB runs of one check are over budget.
const OVER_BUDGET: &str = "[storage]\noutput_budget_mib = 1\noutput_budget_grace_hours = 0\n";
const BIG: &str =
    "[[checks]]\nname = \"big\"\ncommand = \"head -c 600000 /dev/zero | tr '\\\\0' a\"\n";

async fn three_big_runs() -> (Verification, Vec<CheckReceiptView>) {
    let verification = started_configured(true, OVER_BUDGET).await;
    std::fs::write(verification.root.join(".torlek/config.toml"), BIG).unwrap();
    let mut receipts = Vec::new();
    for _ in 0..3 {
        receipts.push(verification.run("big").await);
    }
    // A receipt is recorded before its run is observed ended; retention waits for that.
    let deadline = Instant::now() + GENEROUS;
    while report(&verification, false)
        .await
        .first()
        .map(|row| row.outputs)
        != Some(3)
    {
        assert!(
            Instant::now() < deadline,
            "the runs were never observed ended"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (verification, receipts)
}

async fn report(
    verification: &Verification,
    apply: bool,
) -> Vec<cockpit_protocol::OutputRetentionView> {
    match verification
        .as_owner(Command::StorageOutputs { apply })
        .await
        .unwrap()
    {
        Reply::StorageOutputs {
            projects,
            applied,
            truncated: false,
        } if applied == apply => projects,
        other => panic!("expected the outputs report, got {other:?}"),
    }
}

async fn outputs(
    verification: &Verification,
    apply: bool,
) -> cockpit_protocol::OutputRetentionView {
    let mut projects = report(verification, apply).await;
    assert_eq!(projects.len(), 1, "{projects:?}");
    projects.remove(0)
}

async fn receipt(verification: &Verification, run: &str) -> CheckReceiptView {
    match verification
        .as_owner(Command::CheckReceipt {
            project: "app".to_owned(),
            run: run.to_owned(),
        })
        .await
        .unwrap()
    {
        Reply::CheckReceipt {
            receipt: Some(receipt),
        } => *receipt,
        other => panic!("expected the receipt of {run}, got {other:?}"),
    }
}

fn stdout_of(receipt: &CheckReceiptView) -> PathBuf {
    PathBuf::from(&receipt.output_references[0])
}

#[tokio::test]
async fn the_report_selects_the_oldest_runs_over_budget_and_removes_nothing() {
    let (verification, receipts) = three_big_runs().await;

    let report = outputs(&verification, false).await;

    assert_eq!(
        (
            report.outputs,
            report.protected,
            report.selected,
            report.retired
        ),
        (3, 1, 2, 0)
    );
    assert_eq!(report.selected_bytes, 1_200_000);
    assert!(receipts.iter().all(|receipt| stdout_of(receipt).exists()));
}

#[tokio::test]
async fn prune_retires_the_selected_files_and_keeps_the_newest_run_whole() {
    let (verification, receipts) = three_big_runs().await;

    let report = outputs(&verification, true).await;

    assert_eq!((report.retired, report.retired_bytes), (2, 1_200_000));
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert!(!stdout_of(&receipts[0]).exists());
    assert!(!stdout_of(&receipts[1]).exists());
    assert_eq!(
        std::fs::metadata(stdout_of(&receipts[2])).unwrap().len(),
        600_000
    );
}

#[tokio::test]
async fn a_retired_runs_receipt_keeps_its_tail_and_says_the_rest_was_retired() {
    let (verification, receipts) = three_big_runs().await;
    outputs(&verification, true).await;

    let oldest = receipt(&verification, &receipts[0].run).await;
    let newest = receipt(&verification, &receipts[2].run).await;

    assert!(oldest.output_retired);
    assert_eq!(oldest.output, receipts[0].output);
    assert!(!newest.output_retired);
}

#[tokio::test]
async fn a_second_prune_finds_nothing_left_to_retire() {
    let (verification, _receipts) = three_big_runs().await;
    outputs(&verification, true).await;

    let again = outputs(&verification, true).await;

    assert_eq!((again.outputs, again.selected, again.retired), (1, 0, 0));
}

#[tokio::test]
async fn an_agent_cannot_report_or_retire_outputs() {
    let verification = started().await;

    for apply in [false, true] {
        let refused = verification
            .as_agent(Command::StorageOutputs { apply })
            .await;

        assert!(
            matches!(&refused, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized),
            "{refused:?}"
        );
    }
}

#[tokio::test]
async fn the_storage_report_counts_the_selected_outputs_and_removes_none() {
    let (verification, receipts) = three_big_runs().await;

    let areas = match verification.as_owner(Command::StorageReport).await.unwrap() {
        Reply::Storage {
            areas: Some(areas), ..
        } => areas,
        other => panic!("expected a storage report with its areas, got {other:?}"),
    };

    let projects = areas.outputs.expect("the outputs area");
    assert_eq!(
        projects
            .iter()
            .map(|row| (row.selected, row.selected_bytes, row.retired))
            .collect::<Vec<_>>(),
        [(2, 1_200_000, 0)]
    );
    assert!(receipts.iter().all(|receipt| stdout_of(receipt).exists()));
    assert_eq!(outputs(&verification, false).await.selected, 2);
}
