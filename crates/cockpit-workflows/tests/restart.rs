//! Issue #39's acceptance: a restart mid-step resumes at the checkpoint, and the task packet
//! is rebuilt from durable records alone.

mod common;

use cockpit_tasks::{Edit, Kind};
use cockpit_workflows::{Actor, NextAction, StepStatus};
use common::{Fixture, NOW, completion, handoff};

#[tokio::test]
async fn a_restart_mid_step_resumes_at_the_checkpoint() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    fixture
        .complete(&session, run.id, completion("Rewrote the form validation"))
        .await
        .unwrap();
    fixture.begin(&session, run.id).await.unwrap();
    fixture
        .checkpoint(&session, run.id, "Checks pass; review half done")
        .await
        .unwrap();
    let before = fixture.packet(&session, run.id).await;

    let fixture = fixture.reopen().await;

    let resumed = fixture.run(run.id).await;
    let step = resumed.current_step().unwrap();
    assert_eq!((step.position, step.status), (2, StepStatus::Active));
    assert_eq!(
        step.checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.note.as_str()),
        Some("Checks pass; review half done")
    );
    assert_eq!(fixture.packet(&session, run.id).await, before);
}

#[tokio::test]
async fn the_next_session_resumes_a_released_step_from_its_checkpoint_after_a_restart() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let first = fixture.session("s1", "cockpit").await;
    fixture.begin(&first, run.id).await.unwrap();
    fixture
        .checkpoint(&first, run.id, "Validation rewritten; tests next")
        .await
        .unwrap();
    fixture
        .release(&first, run.id, Some(handoff("Rewrote the validation")))
        .await
        .unwrap();

    let fixture = fixture.reopen().await;
    let second = fixture.session("s2", "cockpit").await;
    fixture.begin(&second, run.id).await.unwrap();

    let packet = fixture.packet(&second, run.id).await;
    let step = packet.step.unwrap();
    assert_eq!((step.position, step.attempt), (1, 2));
    assert_eq!(
        step.checkpoint.map(|checkpoint| checkpoint.note),
        Some("Validation rewritten; tests next".to_owned())
    );
    assert_eq!(packet.handoffs.len(), 1);
    assert_eq!(packet.handoffs[0].handoff.changed, "Rewrote the validation");
}

#[tokio::test]
async fn the_packet_carries_the_goal_scope_and_step() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    let session = fixture.session("s1", "cockpit").await;
    fixture.begin(&session, run.id).await.unwrap();
    let packet = fixture.packet(&session, run.id).await;
    assert_eq!(packet.goal.title, "Fix the login form");
    assert_eq!(packet.goal.notes, "Notes for Fix the login form.");
    assert_eq!(packet.project, "cockpit");
    let step = packet.step.unwrap();
    assert_eq!(step.definition.name, "implement");
    assert_eq!(step.session.unwrap().as_str(), "s1");
    assert!(matches!(
        packet.next_action,
        NextAction::InProgress { position: 1, .. }
    ));
}

#[tokio::test]
async fn the_packet_says_when_the_items_intent_changed_since_the_run_started() {
    let fixture = Fixture::new().await;
    let run = fixture.code_change("cockpit").await;
    assert!(
        !fixture
            .packet(&Actor::Owner, run.id)
            .await
            .goal
            .is_intent_changed()
    );
    let write = fixture.database.write().await.unwrap();
    cockpit_tasks::edit(
        &write,
        &Actor::Owner,
        run.item,
        None,
        Edit {
            notes: Some("Also the signup form.".to_owned()),
            ..Edit::default()
        },
        NOW,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    assert!(
        fixture
            .packet(&Actor::Owner, run.id)
            .await
            .goal
            .is_intent_changed()
    );
}

#[tokio::test]
async fn the_packet_passes_on_what_the_awaited_items_decided() {
    let fixture = Fixture::new().await;
    let decided = fixture
        .item("cockpit", "Pick the form library", Kind::Research)
        .await;
    let run = fixture.code_change("cockpit").await;
    let write = fixture.database.write().await.unwrap();
    cockpit_tasks::wait(&write, &Actor::Owner, run.item, vec![decided.id], NOW)
        .await
        .unwrap();
    cockpit_tasks::start(&write, &Actor::Owner, decided.id, NOW)
        .await
        .unwrap();
    cockpit_tasks::finish(
        &write,
        &Actor::Owner,
        decided.id,
        Some("Use the built-in one."),
        NOW,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    let packet = fixture.packet(&Actor::Owner, run.id).await;
    assert_eq!(
        packet
            .decisions
            .iter()
            .map(|wait| wait.summary.as_deref())
            .collect::<Vec<_>>(),
        [Some("Use the built-in one.")]
    );
}
