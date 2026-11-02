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

#[tokio::test]
async fn partial_and_mismatched_completion_identities_refuse_for_both_peers_without_mutation() {
    let mut workflows = prepared("").await;
    let run = workflows.begun_with_checkpoint("original attempt").await;
    workflows.daemon = Some(workflows.home.start());
    let before = workflows.packet(run).await;
    for (step, attempt) in [
        (Some(1), None),
        (None, Some(1)),
        (Some(2), Some(1)),
        (Some(1), Some(2)),
    ] {
        for agent in [true, false] {
            let command = Command::WorkflowComplete {
                launch_token: agent.then(|| workflows.token.as_str().to_owned()),
                workflow: run,
                expected_step: step,
                expected_attempt: attempt,
                handoff: handoff("Result for a mismatched or incomplete identity"),
                waive: Vec::new(),
            };
            let result = if agent {
                workflows.as_agent(command).await
            } else {
                workflows.as_owner(command).await
            };
            assert_eq!(
                refused(result).code,
                ErrorCode::StaleRevision,
                "agent={agent}, step={step:?}, attempt={attempt:?}"
            );
            assert_eq!(workflows.packet(run).await, before);
        }
    }
}

#[tokio::test]
async fn a_completed_steps_identity_cannot_complete_the_next_step_but_owner_override_remains() {
    let mut workflows = prepared("").await;
    let run = workflows.begun_with_checkpoint("original attempt").await;
    workflows.daemon = Some(workflows.home.start());
    let completion = |step, attempt, waive| Command::WorkflowComplete {
        launch_token: None,
        workflow: run,
        expected_step: step,
        expected_attempt: attempt,
        handoff: handoff("Owner reviewed this result"),
        waive,
    };
    // A supplied, current owner identity is accepted, without waiving any requirement.
    let completed = workflow(
        workflows
            .as_owner(completion(Some(1), Some(1), Vec::new()))
            .await,
    );
    assert_eq!(completed.steps[0].status, WorkflowStepStatusView::Done);
    assert_eq!(
        completed.next,
        WorkflowNextView::AwaitPlacement { position: 2 }
    );
    let before = workflows.packet(run).await;
    // Waivers cannot make an obsolete binding current, even for the owner.
    for agent in [true, false] {
        let mut stale = completion(Some(1), Some(1), vec!["checks".into(), "review".into()]);
        let result = if agent {
            if let Command::WorkflowComplete {
                launch_token,
                waive,
                ..
            } = &mut stale
            {
                *launch_token = workflows.token();
                waive.clear();
            }
            workflows.as_agent(stale).await
        } else {
            workflows.as_owner(stale).await
        };
        assert_eq!(refused(result).code, ErrorCode::StaleRevision);
        assert_eq!(workflows.packet(run).await, before);
    }
    // Omitting both fields remains the owner's deliberate current-step override.
    let completed = workflow(
        workflows
            .as_owner(completion(
                None,
                None,
                vec!["checks".into(), "review".into()],
            ))
            .await,
    );
    assert_eq!(completed.steps[1].status, WorkflowStepStatusView::Done);
    assert_eq!(
        completed.next,
        WorkflowNextView::AwaitPlacement { position: 3 }
    );
}
