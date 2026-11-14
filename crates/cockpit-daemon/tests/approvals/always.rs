//! #1387, part 3, through the daemon's sockets: the owner approves a command always, and its
//! project's later asks of exactly that command line and script run with no prompt; a changed
//! script or argument asks again, and a revoked one approves nothing more. The test's session
//! runs unsandboxed, so a script in its directory is one it can edit, and pinned.
use super::*;

/// The owner approves `shown` always, as soon as the grace after its ask allows.
async fn approve_always(asking: &Asking, shown: &RunRequestView) -> Result<Reply, ClientError> {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let reply = asking
            .owner
            .request(
                Command::ApprovalApprove {
                    request: shown.id,
                    revision: shown.revision,
                    digest: shown.digest.clone(),
                    through: None,
                    always: true,
                },
                GENEROUS,
            )
            .await;
        let too_soon = matches!(
            &reply,
            Err(ClientError::Daemon(error)) if error.message.contains("just now")
        );
        if !too_soon || Instant::now() > deadline {
            return reply;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// In a registered project, the owner approved `sh ./x.sh` always and it ran.
async fn approved_always_once() -> Asking {
    let asking = started(true).await;
    in_a_registered_project(&asking).await;
    std::fs::write(asking.cwd.path().join("x.sh"), "echo built\n").unwrap();
    let asked = asking.asked("sh ./x.sh").await;
    let shown = shown_to_owner(&asking, asked.id).await;
    run(approve_always(&asking, &shown).await);
    let ended = asking.ended(asked.id).await;
    assert_eq!(ended.state, RunStateView::Finished, "{ended:?}");
    asking
}

async fn always_approvals(asking: &Asking) -> Vec<cockpit_protocol::AlwaysApprovalView> {
    match asking
        .owner
        .request(Command::ApprovalAlways, GENEROUS)
        .await
    {
        Ok(Reply::ApprovalAlways { approvals }) => approvals,
        other => panic!("expected the always approvals, got {other:?}"),
    }
}

#[tokio::test]
async fn an_always_approval_runs_the_same_ask_again_without_the_owner() {
    let asking = approved_always_once().await;
    let [approval] = always_approvals(&asking).await.try_into().unwrap();

    let again = asking.asked("sh ./x.sh").await;

    assert_ne!(again.state, RunStateView::Asking, "{again:?}");
    assert_eq!(
        again.decision.and_then(|decision| decision.always),
        Some(approval.id)
    );
    let ended = asking.ended(again.id).await;
    assert_eq!(ended.state, RunStateView::Finished, "{ended:?}");
    assert!(ended.result.unwrap().output.contains("built"));
}

#[tokio::test]
async fn an_always_approval_does_not_cover_the_command_after_its_script_changed() {
    let asking = approved_always_once().await;
    std::fs::write(asking.cwd.path().join("x.sh"), "echo replaced\n").unwrap();

    let again = asking.asked("sh ./x.sh").await;

    assert_eq!(again.state, RunStateView::Asking, "{again:?}");
    assert_eq!(again.decision, None);
}

#[tokio::test]
async fn an_always_approval_does_not_cover_another_argument() {
    let asking = approved_always_once().await;

    let again = asking.asked("sh ./x.sh --release").await;

    assert_eq!(again.state, RunStateView::Asking, "{again:?}");
}

#[tokio::test]
async fn a_revoked_always_approval_asks_again() {
    let asking = approved_always_once().await;
    let [approval] = always_approvals(&asking).await.try_into().unwrap();

    let revoked = asking
        .owner
        .request(
            Command::ApprovalAlwaysRevoke {
                always: approval.id,
                through: None,
            },
            GENEROUS,
        )
        .await;

    let Ok(Reply::AlwaysApproval { approval: revoked }) = revoked else {
        panic!("expected the revoked approval, got {revoked:?}");
    };
    assert!(revoked.revoked_at.is_some());
    assert_eq!(always_approvals(&asking).await, Vec::new());
    assert_eq!(asking.asked("sh ./x.sh").await.state, RunStateView::Asking);
}

#[tokio::test]
async fn approving_always_a_command_torlek_cannot_tell_is_refused_and_approves_nothing() {
    let asking = started(true).await;
    in_a_registered_project(&asking).await;
    let asked = asking.asked("eval \"$BUILD\"").await;
    let shown = shown_to_owner(&asking, asked.id).await;

    let refusal = refused(approve_always(&asking, &shown).await);

    assert_eq!(refusal.code, ErrorCode::Refused, "{refusal:?}");
    assert!(
        refusal.message.contains("cannot be approved always"),
        "{refusal:?}"
    );
    assert_eq!(
        shown_to_owner(&asking, asked.id).await.state,
        RunStateView::Asking
    );
    assert_eq!(always_approvals(&asking).await, Vec::new());
}

#[tokio::test]
async fn an_agent_cannot_list_or_revoke_always_approvals() {
    let asking = started(true).await;

    for command in [
        Command::ApprovalAlways,
        Command::ApprovalAlwaysRevoke {
            always: 1,
            through: None,
        },
    ] {
        let refusal = refused(asking.agent.request(command, GENEROUS).await);
        assert_eq!(refusal.code, ErrorCode::NotAuthorized, "{refusal:?}");
    }
}
