//! Report 04: an offline owner's approval remains bound to the work it displayed.
use super::*;

#[tokio::test]
async fn an_offline_approval_is_refused_after_edit_and_restart_until_the_client_resnapshots() {
    let mut asking = started(true).await;
    let old = asking.asked("printf old").await;
    let offline = asking.home.connect().await;
    let snapshot = offline
        .request(Command::ApprovalQueue, GENEROUS)
        .await
        .unwrap();
    let Reply::ApprovalQueue { runs, .. } = snapshot else {
        panic!("{snapshot:?}")
    };
    assert_eq!(runs[0].id, old.id);
    drop(offline);
    let edited = run(asking
        .owner
        .request(
            Command::ApprovalEdit {
                request: old.id,
                revision: old.revision,
                command: "printf replacement".into(),
            },
            GENEROUS,
        )
        .await);
    asking.restart().await;
    assert_eq!(
        refused(asking.approve_once(&old).await).code,
        ErrorCode::StaleRevision
    );
    let mut wrong_digest = edited.clone();
    wrong_digest.digest = old.digest;
    assert_eq!(
        refused(asking.approve_once(&wrong_digest).await).code,
        ErrorCode::StaleRevision
    );
    let snapshot = asking
        .owner
        .request(Command::ApprovalQueue, GENEROUS)
        .await
        .unwrap();
    let Reply::ApprovalQueue { runs, .. } = snapshot else {
        panic!("{snapshot:?}")
    };
    let fresh = runs.iter().find(|run| run.id == old.id).unwrap();
    assert_eq!(fresh.revision, edited.revision);
    assert_eq!(fresh.digest, edited.digest);
    assert!(fresh.result.is_none());
    run(asking.approve(fresh).await);
    assert_eq!(
        asking.ended(old.id).await.result.unwrap().output,
        "replacement"
    );
    assert_eq!(
        refused(asking.approve_once(fresh).await).code,
        ErrorCode::Conflict
    );
}
