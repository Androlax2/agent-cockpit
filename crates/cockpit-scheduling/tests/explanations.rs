//! Every open item that waits has one record naming why, what limits it and what moves it;
//! the records are rewritten each pass and survive a restart.

mod common;

use cockpit_resource_admission::{Demand, Policy};
use cockpit_scheduling::{ActionBy, Reason, Unit};
use cockpit_tasks::{Actor, Priority};
use cockpit_workflows::DESIGN_LED;
use common::{Fixture, NOW, TURN, explanation_of};

#[tokio::test]
async fn a_paused_queue_holds_queued_work_until_the_owner_resumes_it() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.pause_queue().await;

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    assert!(dispatched.placed.is_empty());
    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::QueuePaused);
    assert_eq!(
        explanation.action,
        "resume the queue: `cockpit resources resume-queue`"
    );
    assert_eq!(explanation.action_by, ActionBy::Owner);
}

#[tokio::test]
async fn a_turn_larger_than_the_budget_is_infeasible_and_names_the_limit() {
    let mut fixture = Fixture::with_budget(4).await;
    fixture.policy = Policy::for_budget(Demand {
        cpu_millis: TURN.cpu_millis / 2,
        memory_mib: TURN.memory_mib * 4,
    });
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::Infeasible);
    assert_eq!(
        explanation.limited_by.as_deref(),
        Some("cpu in the shared budget")
    );
    assert_eq!(explanation.action_by, ActionBy::Owner);
}

#[tokio::test]
async fn work_waiting_for_room_names_the_resource_and_is_moved_by_the_cockpit() {
    let fixture = Fixture::with_budget(1).await;
    let first = fixture.session("a-1", "a").await;
    let second = fixture.session("a-2", "a").await;
    fixture.queued("a", "first", Priority::NORMAL).await;
    let waiting = fixture.queued("a", "second", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[first, second]).await;

    let explanation = explanation_of(&dispatched.explanations, waiting.id);
    assert_eq!(explanation.reason, Reason::NoRoom);
    assert_eq!(
        explanation.limited_by.as_deref(),
        Some("cpu in the shared budget")
    );
    assert!(
        explanation.why.ends_with(", after the placed agent turns"),
        "{}",
        explanation.why
    );
    assert_eq!(explanation.action_by, ActionBy::Cockpit);
}

#[tokio::test]
async fn a_project_without_sessions_asks_the_owner_to_start_one() {
    let fixture = Fixture::with_budget(1).await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[]).await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::NoSession);
    assert_eq!(
        explanation.action,
        "start a session in project a: `cockpit session new DIR`"
    );
}

#[tokio::test]
async fn an_unqueued_item_is_explained_with_the_command_that_queues_it() {
    let fixture = Fixture::with_budget(1).await;
    let item = fixture.item("a", "task", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[]).await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::NotQueued);
    assert_eq!(
        explanation.action,
        format!("queue it: `cockpit todo queue {}`", item.id.0)
    );
    assert_eq!(explanation.waiting_since, None);
}

#[tokio::test]
async fn an_item_waiting_on_an_unfinished_one_names_it() {
    let fixture = Fixture::with_budget(1).await;
    let first = fixture.item("a", "first", Priority::NORMAL).await;
    let then = fixture.queued("a", "then", Priority::NORMAL).await;
    let write = fixture.database.write().await.unwrap();
    cockpit_tasks::wait(&write, &Actor::Owner, then.id, vec![first.id], NOW)
        .await
        .unwrap();
    write.commit().await.unwrap();

    let dispatched = fixture.dispatch_at(NOW, &[]).await;

    let explanation = explanation_of(&dispatched.explanations, then.id);
    assert_eq!(explanation.reason, Reason::WaitsOnItems);
    assert_eq!(
        explanation.why,
        format!("it waits on item {}, not done yet", first.id.0)
    );
}

#[tokio::test]
async fn an_item_whose_run_waits_on_the_owners_step_says_so() {
    let fixture = Fixture::with_budget(1).await;
    let item = fixture.item("a", "screen", Priority::NORMAL).await;
    let run = fixture.run(item.id, &DESIGN_LED).await;

    let dispatched = fixture.dispatch_at(NOW, &[]).await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::OwnerStep);
    assert_eq!(
        explanation.unit,
        Unit::Step {
            run: run.id,
            position: 1,
            name: "design-approval",
        }
    );
}

#[tokio::test]
async fn queued_work_is_explained_before_work_waiting_outside_the_queue() {
    let fixture = Fixture::with_budget(1).await;
    let outside = fixture.item("a", "unqueued", Priority::HIGHEST).await;
    let queued = fixture.queued("a", "queued", Priority::LOW).await;

    let dispatched = fixture.dispatch_at(NOW, &[]).await;

    let items: Vec<_> = dispatched
        .explanations
        .iter()
        .map(|explanation| explanation.item)
        .collect();
    assert_eq!(items, [queued.id, outside.id]);
}

#[tokio::test]
async fn placed_work_has_no_explanation() {
    let fixture = Fixture::with_budget(1).await;
    let session = fixture.session("a-1", "a").await;
    fixture.queued("a", "task", Priority::NORMAL).await;

    fixture.dispatch_at(NOW, &[session]).await;

    assert!(fixture.explanations().await.is_empty());
}

#[tokio::test]
async fn the_records_read_back_as_the_pass_wrote_them_after_a_reopen() {
    let fixture = Fixture::with_budget(1).await;
    let first = fixture.session("a-1", "a").await;
    let second = fixture.session("a-2", "a").await;
    fixture.queued("a", "placed", Priority::NORMAL).await;
    fixture
        .queued("a", "waits for room", Priority::NORMAL)
        .await;
    fixture.item("a", "unqueued", Priority::NORMAL).await;
    let written = fixture
        .dispatch_at(NOW, &[first, second])
        .await
        .explanations;

    let fixture = fixture.reopen().await;

    assert_eq!(written.len(), 2);
    assert_eq!(fixture.explanations().await, written);
}

#[tokio::test]
async fn the_time_a_unit_entered_the_queue_survives_a_reopen() {
    let fixture = Fixture::with_budget(1).await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.dispatch_at(NOW, &[]).await;
    let fixture = fixture.reopen().await;

    let dispatched = fixture.dispatch_at(NOW + 3600, &[]).await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.waiting_since, Some(NOW));
    assert_eq!(explanation.next_evaluation_at, NOW + 3600 + 30);
}
