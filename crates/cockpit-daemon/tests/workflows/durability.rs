//! Report 04: a completion prepared for an obsolete attempt cannot finish its replacement.
use super::*;

#[tokio::test]
async fn an_obsolete_completion_cannot_finish_a_replacement_attempt_on_the_same_session() {
    let mut workflows = prepared("").await;
    let run = workflows.begun_with_checkpoint("first attempt").await;
    workflows.daemon = Some(workflows.home.start());
    let obsolete = Command::WorkflowComplete {
        launch_token: workflows.token(),
        workflow: run,
        expected_step: Some(1),
        expected_attempt: Some(1),
        handoff: handoff("Result prepared for the first attempt"),
        waive: Vec::new(),
    };
    workflow(
        workflows
            .as_owner(Command::WorkflowRelease {
                launch_token: None,
                workflow: run,
                handoff: None,
            })
            .await,
    );
    workflows.daemon.take().unwrap().kill();
    // As in the parent fixture, begin through the capability: this test process has an
    // authenticated launch but no provider-backed session record for WorkflowBegin.
    {
        let database = workflows.home.open_state().await;
        let write = database.write().await.unwrap();
        let actor = cockpit_tasks::Actor::Session {
            session: SessionId::new("s0123456789ab").unwrap(),
            project: Some("app".into()),
        };
        let capabilities = Capabilities::none()
            .with(Capability::Events, Provenance::Discovered)
            .with(Capability::Outcome, Provenance::Discovered);
        let replacement = cockpit_workflows::begin(
            &write,
            &actor,
            cockpit_workflows::RunId(run),
            &capabilities,
            NOW + 1,
        )
        .await
        .unwrap();
        assert_eq!(replacement.current_step().unwrap().attempts, 2);
        write.commit().await.unwrap();
    }
    workflows.daemon = Some(workflows.home.start());
    let before = workflows.packet(run).await;
    let result = workflows.as_agent(obsolete.clone()).await;
    let after = workflows.packet(run).await;
    let refusal = refused(result);
    assert!(
        matches!(refusal.code, ErrorCode::StaleRevision | ErrorCode::Conflict),
        "expected an obsolete-attempt refusal: {refusal:?}"
    );
    assert_eq!(after, before, "obsolete work cannot mutate the replacement");
    let mut fresh = obsolete;
    if let Command::WorkflowComplete {
        expected_step,
        expected_attempt,
        ..
    } = &mut fresh
    {
        *expected_step = None;
        *expected_attempt = None;
    }
    assert_eq!(
        refused(workflows.as_agent(fresh.clone()).await).code,
        ErrorCode::StaleRevision
    );
    if let Command::WorkflowComplete {
        expected_step,
        expected_attempt,
        ..
    } = &mut fresh
    {
        *expected_step = Some(1);
        *expected_attempt = Some(2);
    }
    assert_eq!(
        workflow(workflows.as_agent(fresh).await).steps[0].status,
        WorkflowStepStatusView::Done
    );
}
