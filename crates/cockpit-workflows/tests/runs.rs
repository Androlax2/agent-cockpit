//! The step state machine on a real state database: starting, beginning, checkpoints,
//! completion with requirements, release, blockers, cancellation and who may act.

mod common;

use cockpit_authorization::SessionId;
use cockpit_provider_api::{Capabilities, Capability};
use cockpit_tasks::{ItemId, Kind};
use cockpit_workflows::{
    Actor, BlockedAction, CODE_CHANGE, Completion, DESIGN_LED, HANDOFF_MAX_BYTES, Handoff,
    HandoffKind, NextAction, Refusal, RunFilter, RunStatus, StepStatus,
};
use common::{Fixture, NOW, completion, handoff, refusal};

fn statuses(run: &cockpit_workflows::Run) -> Vec<StepStatus> {
    run.steps.iter().map(|step| step.status).collect()
}

#[tokio::test]
async fn a_started_run_makes_its_first_step_ready() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    assert_eq!(run.preset, &CODE_CHANGE);
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(
        statuses(&run),
        [StepStatus::Ready, StepStatus::Pending, StepStatus::Pending]
    );
    assert_eq!(
        run.next_action(),
        NextAction::AwaitPlacement { position: 1 }
    );
}

#[tokio::test]
async fn scheduling_reads_the_ready_agent_steps() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let read = fixture.database.read().await.unwrap();
    let ready = cockpit_workflows::ready_steps(&read).await.unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!((ready[0].run, ready[0].position), (run.id, 1));
    assert_eq!(ready[0].definition.name, "implement");
}

#[tokio::test]
async fn only_the_owner_starts_a_workflow() {
    let fixture = Fixture::new().await;
    let item = fixture.item("cockpit", "Fix it", Kind::Bug).await;
    let session = fixture.session("s1", "cockpit").await;
    let write = fixture.database.write().await.unwrap();
    let started = cockpit_workflows::start(&write, &session, item.id, &CODE_CHANGE, NOW).await;
    assert_eq!(
        refusal(started),
        Refusal::OwnerOnly {
            action: "start a workflow"
        }
    );
}

#[tokio::test]
async fn an_item_follows_one_running_workflow_at_a_time() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    assert_eq!(
        refusal(fixture.start(run.item, &CODE_CHANGE).await),
        Refusal::AlreadyRunning {
            item: run.item,
            run: run.id
        }
    );
}

#[tokio::test]
async fn a_cancelled_run_lets_its_item_start_another() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let write = fixture.database.write().await.unwrap();
    let cancelled = cockpit_workflows::cancel(&write, &Actor::Owner, run.id, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
    assert_eq!(cancelled.status, RunStatus::Cancelled);
    assert_eq!(cancelled.next_action(), NextAction::Nothing);
    let again = fixture.start(run.item, &DESIGN_LED).await.unwrap();
    assert_ne!(again.id, run.id);
}

#[tokio::test]
async fn a_missing_item_is_refused() {
    let fixture = Fixture::new().await;
    assert_eq!(
        refusal(fixture.start(ItemId(99), &CODE_CHANGE).await),
        Refusal::ItemNotFound { item: ItemId(99) }
    );
}

#[tokio::test]
async fn a_done_item_starts_no_workflow() {
    let fixture = Fixture::new().await;
    let item = fixture.item("cockpit", "Fix it", Kind::Bug).await;
    let write = fixture.database.write().await.unwrap();
    cockpit_tasks::start(&write, &Actor::Owner, item.id, NOW)
        .await
        .unwrap();
    cockpit_tasks::finish(
        &write,
        &Actor::Owner,
        item.id,
        Some("Fixed in abc123."),
        NOW,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    assert_eq!(
        refusal(fixture.start(item.id, &CODE_CHANGE).await),
        Refusal::ItemNotOpen {
            item: item.id,
            status: "done"
        }
    );
}

#[tokio::test]
async fn a_session_begins_the_ready_agent_step() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    let begun = fixture.begin(&session, run.id).await.unwrap();
    let step = &begun.steps[0];
    assert_eq!(step.status, StepStatus::Active);
    assert_eq!(step.session, Some(SessionId::new("s1").unwrap()));
    assert_eq!(step.attempts, 1);
    let read = fixture.database.read().await.unwrap();
    assert_eq!(cockpit_workflows::ready_steps(&read).await.unwrap(), []);
}

#[tokio::test]
async fn a_provider_without_the_declared_capabilities_cannot_begin() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    assert_eq!(
        refusal(
            fixture
                .begin_with(&session, run.id, &Capabilities::none())
                .await
        ),
        Refusal::MissingCapabilities {
            step: "implement",
            missing: vec![Capability::Events, Capability::Outcome],
        }
    );
    assert_eq!(fixture.run(run.id).await.steps[0].status, StepStatus::Ready);
}

#[tokio::test]
async fn a_session_of_another_project_does_not_see_the_run() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let stranger = fixture.session("s2", "dotfiles").await;
    assert_eq!(
        refusal(fixture.begin(&stranger, run.id).await),
        Refusal::NotFound { run: run.id }
    );
    let read = fixture.database.read().await.unwrap();
    let listed = cockpit_workflows::runs(&read, &stranger, &RunFilter::default())
        .await
        .unwrap();
    assert_eq!(listed, []);
}

#[tokio::test]
async fn a_session_without_its_change_state_grant_is_refused() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let ungranted = Actor::Session {
        session: SessionId::new("s9").unwrap(),
        project: Some("cockpit".to_owned()),
    };
    assert!(matches!(
        refusal(fixture.begin(&ungranted, run.id).await),
        Refusal::NotAuthorized(_)
    ));
}

#[tokio::test]
async fn a_session_does_one_step_at_a_time() {
    let fixture = Fixture::new().await;
    let first = fixture.code_change("cockpit").await;
    let second = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, first.id).await.unwrap();
    assert_eq!(
        refusal(fixture.begin(&session, second.id).await),
        Refusal::AlreadyDoing {
            run: first.id,
            position: 1
        }
    );
}

#[tokio::test]
async fn an_active_step_is_not_begun_twice() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let first = fixture.session("s1", "cockpit").await;
    let second = fixture.session("s2", "cockpit").await;
    fixture.begin(&first, run.id).await.unwrap();
    assert_eq!(
        refusal(fixture.begin(&second, run.id).await),
        Refusal::WrongStep {
            run: run.id,
            position: 1,
            status: StepStatus::Active,
            action: "be begun"
        }
    );
}

#[tokio::test]
async fn only_the_session_doing_the_step_records_its_checkpoint() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let doer = fixture.session("s1", "cockpit").await;
    let other = fixture.session("s2", "cockpit").await;
    fixture.begin(&doer, run.id).await.unwrap();
    assert_eq!(
        refusal(fixture.checkpoint(&other, run.id, "Half way").await),
        Refusal::NotDoing { run: run.id }
    );
    let recorded = fixture
        .checkpoint(&doer, run.id, "Form validation rewritten; tests next")
        .await
        .unwrap();
    let checkpoint = recorded.steps[0].checkpoint.clone().unwrap();
    assert_eq!(checkpoint.note, "Form validation rewritten; tests next");
    assert_eq!(checkpoint.revision, 1);
}

#[tokio::test]
async fn completing_a_step_makes_the_next_one_ready() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    let completed = fixture
        .complete(&session, run.id, completion("Rewrote the form validation"))
        .await
        .unwrap();
    assert_eq!(
        statuses(&completed),
        [StepStatus::Done, StepStatus::Ready, StepStatus::Pending]
    );
    assert_eq!(completed.steps[0].session, None);
}

#[tokio::test]
async fn a_step_with_requirements_waits_for_their_evidence() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    fixture
        .complete(&session, run.id, completion("Rewrote the form validation"))
        .await
        .unwrap();
    fixture.begin(&session, run.id).await.unwrap();
    let only_checks = Completion {
        satisfied: vec!["checks".to_owned()],
        ..completion("Ran the checks and the review")
    };
    assert_eq!(
        refusal(fixture.complete(&session, run.id, only_checks).await),
        Refusal::RequirementsUnmet {
            step: "check",
            missing: vec!["review"]
        }
    );
}

#[tokio::test]
async fn satisfied_requirements_complete_the_step() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    fixture
        .complete(&session, run.id, completion("Rewrote the form validation"))
        .await
        .unwrap();
    fixture.begin(&session, run.id).await.unwrap();
    let both = Completion {
        satisfied: vec!["review".to_owned(), "checks".to_owned()],
        ..completion("Ran the checks and the review")
    };
    let completed = fixture.complete(&session, run.id, both).await.unwrap();
    assert_eq!(completed.steps[1].status, StepStatus::Done);
    let read = fixture.database.read().await.unwrap();
    let handoffs = cockpit_workflows::handoffs(&read, &Actor::Owner, run.id)
        .await
        .unwrap();
    assert_eq!(handoffs[1].satisfied, ["review", "checks"]);
}

#[tokio::test]
async fn a_requirement_the_step_does_not_declare_is_refused() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    let misspelt = Completion {
        satisfied: vec!["reveiw".to_owned()],
        ..completion("Rewrote the form validation")
    };
    assert!(matches!(
        refusal(fixture.complete(&session, run.id, misspelt).await),
        Refusal::InvalidRequest {
            field: "satisfied",
            ..
        }
    ));
}

#[tokio::test]
async fn only_the_owner_waives_a_requirement() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    fixture
        .complete(&session, run.id, completion("Rewrote the form validation"))
        .await
        .unwrap();
    fixture.begin(&session, run.id).await.unwrap();
    let waiving = Completion {
        satisfied: vec!["checks".to_owned()],
        waived: vec!["review".to_owned()],
        ..completion("Ran the checks")
    };
    assert_eq!(
        refusal(fixture.complete(&session, run.id, waiving.clone()).await),
        Refusal::OwnerOnly {
            action: "waive a requirement"
        }
    );
    let completed = fixture
        .complete(&Actor::Owner, run.id, waiving)
        .await
        .unwrap();
    assert_eq!(completed.steps[1].status, StepStatus::Done);
    let read = fixture.database.read().await.unwrap();
    let handoffs = cockpit_workflows::handoffs(&read, &Actor::Owner, run.id)
        .await
        .unwrap();
    assert_eq!(
        (handoffs[1].session.clone(), handoffs[1].waived.clone()),
        (None, vec!["review".to_owned()])
    );
}

#[tokio::test]
async fn the_last_step_delivers_the_run() {
    let fixture = Fixture::new().await;
    let item = fixture
        .item("cockpit", "Compare queues", Kind::Research)
        .await;
    let run = fixture
        .start(
            item.id,
            cockpit_workflows::select_preset(None, item.kind).unwrap(),
        )
        .await
        .unwrap();
    let session = fixture.session("s1", "cockpit").await;
    for step in ["investigate", "synthesize", "deliver"] {
        fixture.begin(&session, run.id).await.unwrap();
        fixture
            .complete(&session, run.id, completion(&format!("Did {step}")))
            .await
            .unwrap();
    }
    let delivered = fixture.run(run.id).await;
    assert_eq!(delivered.status, RunStatus::Delivered);
    assert_eq!(delivered.current_step(), None);
}

#[tokio::test]
async fn a_handoff_that_only_says_done_is_refused() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    assert!(matches!(
        refusal(
            fixture
                .complete(&session, run.id, completion("Done."))
                .await
        ),
        Refusal::InvalidRequest {
            field: "changed",
            ..
        }
    ));
}

#[tokio::test]
async fn a_handoff_without_the_checked_result_is_refused() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    let unchecked = Completion {
        handoff: Handoff {
            checked: "  ".to_owned(),
            ..handoff("Rewrote the form validation")
        },
        ..Completion::default()
    };
    assert!(matches!(
        refusal(fixture.complete(&session, run.id, unchecked).await),
        Refusal::InvalidRequest {
            field: "checked",
            ..
        }
    ));
}

#[tokio::test]
async fn a_handoff_over_two_kib_of_prose_is_refused() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    let pasted = completion(&"x".repeat(HANDOFF_MAX_BYTES));
    assert!(matches!(
        refusal(fixture.complete(&session, run.id, pasted).await),
        Refusal::InvalidRequest {
            field: "handoff",
            ..
        }
    ));
}

#[tokio::test]
async fn a_released_step_is_ready_again_with_its_checkpoint() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    fixture
        .checkpoint(&session, run.id, "Validation rewritten; tests next")
        .await
        .unwrap();
    let released = fixture
        .release(&session, run.id, Some(handoff("Rewrote the validation")))
        .await
        .unwrap();
    let step = &released.steps[0];
    assert_eq!(
        (step.status, step.session.clone()),
        (StepStatus::Ready, None)
    );
    assert_eq!(
        step.checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.note.as_str()),
        Some("Validation rewritten; tests next")
    );
    let read = fixture.database.read().await.unwrap();
    let handoffs = cockpit_workflows::handoffs(&read, &Actor::Owner, run.id)
        .await
        .unwrap();
    assert_eq!(handoffs[0].kind, HandoffKind::Continued);
}

#[tokio::test]
async fn a_session_releasing_its_step_must_hand_off() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    assert!(matches!(
        refusal(fixture.release(&session, run.id, None).await),
        Refusal::InvalidRequest {
            field: "handoff",
            ..
        }
    ));
}

#[tokio::test]
async fn the_owner_releases_a_session_that_cannot_hand_off() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    let released = fixture.release(&Actor::Owner, run.id, None).await.unwrap();
    assert_eq!(released.steps[0].status, StepStatus::Ready);
    let next = fixture.session("s2", "cockpit").await;
    let begun = fixture.begin(&next, run.id).await.unwrap();
    assert_eq!(begun.steps[0].attempts, 2);
}

#[tokio::test]
async fn a_blocked_run_is_not_placed_or_completed() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    let write = fixture.database.write().await.unwrap();
    let blocked = cockpit_workflows::block(
        &write,
        &session,
        run.id,
        BlockedAction::ApproveOperation,
        "Approve `npm install` in the project",
        NOW,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    assert_eq!(
        blocked.next_action(),
        NextAction::Unblock {
            action: BlockedAction::ApproveOperation
        }
    );
    assert_eq!(
        refusal(
            fixture
                .complete(&session, run.id, completion("Rewrote the validation"))
                .await
        ),
        Refusal::Blocked { run: run.id }
    );
}

#[tokio::test]
async fn a_blocked_ready_step_leaves_the_ready_list_until_unblocked() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let write = fixture.database.write().await.unwrap();
    cockpit_workflows::block(
        &write,
        &Actor::Owner,
        run.id,
        BlockedAction::WaitForCapacity,
        "Wait for the weekly window",
        NOW,
    )
    .await
    .unwrap();
    assert_eq!(cockpit_workflows::ready_steps(&write).await.unwrap(), []);
    cockpit_workflows::unblock(&write, &Actor::Owner, run.id, NOW)
        .await
        .unwrap();
    assert_eq!(
        cockpit_workflows::ready_steps(&write).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn a_session_not_doing_the_step_cannot_block_the_run() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    let write = fixture.database.write().await.unwrap();
    let blocked = cockpit_workflows::block(
        &write,
        &session,
        run.id,
        BlockedAction::AnswerQuestion,
        "Which form?",
        NOW,
    )
    .await;
    assert_eq!(refusal(blocked), Refusal::NotDoing { run: run.id });
}

#[tokio::test]
async fn only_the_owner_completes_an_owner_step() {
    let fixture = Fixture::new().await;
    let item = fixture
        .item("cockpit", "New settings screen", Kind::Feature)
        .await;
    let run = fixture.start(item.id, &DESIGN_LED).await.unwrap();
    assert_eq!(
        run.next_action(),
        NextAction::OwnerCompletes { position: 1 }
    );
    let session = fixture.session("s1", "cockpit").await;
    assert_eq!(
        refusal(fixture.begin(&session, run.id).await),
        Refusal::WrongStep {
            run: run.id,
            position: 1,
            status: StepStatus::Ready,
            action: "be begun by a session"
        }
    );
    assert_eq!(
        refusal(
            fixture
                .complete(&session, run.id, completion("Approved frame 12"))
                .await
        ),
        Refusal::OwnerOnly {
            action: "complete an owner step"
        }
    );
    let approved = fixture
        .complete(&Actor::Owner, run.id, completion("Approved frame 12"))
        .await
        .unwrap();
    assert_eq!(
        approved.next_action(),
        NextAction::AwaitPlacement { position: 2 }
    );
}

#[tokio::test]
async fn only_the_owner_cancels() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    let write = fixture.database.write().await.unwrap();
    assert_eq!(
        refusal(cockpit_workflows::cancel(&write, &session, run.id, NOW).await),
        Refusal::OwnerOnly {
            action: "cancel a workflow"
        }
    );
}

#[tokio::test]
async fn a_cancelled_run_takes_no_more_steps() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let write = fixture.database.write().await.unwrap();
    cockpit_workflows::cancel(&write, &Actor::Owner, run.id, NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();
    let session = fixture.session("s1", "cockpit").await;
    assert_eq!(
        refusal(fixture.begin(&session, run.id).await),
        Refusal::NotRunning {
            run: run.id,
            status: RunStatus::Cancelled
        }
    );
}

#[tokio::test]
async fn a_blocked_runs_ready_step_cannot_be_begun() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    let write = fixture.database.write().await.unwrap();
    cockpit_workflows::block(
        &write,
        &Actor::Owner,
        run.id,
        BlockedAction::ApproveOperation,
        "Approve `npm install` in the project",
        NOW,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();

    assert_eq!(
        refusal(fixture.begin(&session, run.id).await),
        Refusal::Blocked { run: run.id }
    );
}

#[tokio::test]
async fn a_step_of_a_finished_item_cannot_be_begun() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    let write = fixture.database.write().await.unwrap();
    cockpit_tasks::finish(&write, &Actor::Owner, run.item, Some("done by hand"), NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    assert_eq!(
        refusal(fixture.begin(&session, run.id).await),
        Refusal::ItemNotOpen {
            item: run.item,
            status: "done",
        }
    );
}

#[tokio::test]
async fn a_session_lists_only_its_projects_workflows_whatever_project_it_names() {
    let fixture = Fixture::new().await;
    fixture.code_change("cockpit").await;
    let other = fixture.code_change("other").await;
    let session = fixture.session("s1", "cockpit").await;
    let read = fixture.database.read().await.unwrap();
    let asked_other = RunFilter {
        project: Some("other".to_owned()),
        include_closed: false,
    };

    let listed = cockpit_workflows::runs(&read, &session, &asked_other)
        .await
        .unwrap();

    assert!(listed.iter().all(|run| run.id != other.id), "{listed:?}");
    assert!(listed.is_empty());
}
