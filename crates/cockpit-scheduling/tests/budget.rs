//! Per-item budgets (#412, owner choice 2A): once an item's sessions have worked it for its
//! budget's agent minutes, the queue places none of it, neither the item again nor its run's
//! next step, and asks the owner. Work already placed runs on.

mod common;

use std::collections::BTreeMap;

use cockpit_scheduling::{ActionBy, Reason, Unit};
use cockpit_tasks::Priority;
use cockpit_workflows::RESEARCH;
use common::{Fixture, NOW, explanation_of};

const HOUR: u64 = 60 * 60;

#[tokio::test]
async fn an_item_under_its_budget_is_placed_and_one_at_it_waits() {
    for (seconds, expected) in [(HOUR - 1, true), (HOUR, false)] {
        let fixture = Fixture::with_budget(4).await;
        let session = fixture.session("a-1", "a").await;
        let item = fixture.queued("a", "task", Priority::NORMAL).await;
        fixture.budget(item.id, 60).await;

        let dispatched = fixture
            .dispatch_after(NOW, &[session], BTreeMap::from([(item.id, seconds)]))
            .await;

        assert_eq!(
            !dispatched.placed.is_empty(),
            expected,
            "{seconds} s worked"
        );
    }
}

#[tokio::test]
async fn an_item_at_its_budget_asks_the_owner_for_more_time() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.budget(item.id, 60).await;

    let dispatched = fixture
        .dispatch_after(NOW, &[session], BTreeMap::from([(item.id, HOUR + 7 * 60)]))
        .await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::OverBudget);
    assert_eq!(
        explanation.why,
        "its sessions have worked it about 67 min, its 60 min budget"
    );
    assert_eq!(
        explanation.action,
        format!(
            "give it 30 min more to go on: `torlek todo budget {} 97`",
            item.id.0
        )
    );
    assert_eq!(explanation.action_by, ActionBy::Owner);
    assert!(!explanation.reason.is_queued());
}

#[tokio::test]
async fn the_offer_of_more_time_starts_from_the_time_worked_past_the_budget() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.budget(item.id, 60).await;

    let dispatched = fixture
        .dispatch_after(NOW, &[session], BTreeMap::from([(item.id, 200 * 60)]))
        .await;

    assert_eq!(
        explanation_of(&dispatched.explanations, item.id).action,
        format!(
            "give it 30 min more to go on: `torlek todo budget {} 230`",
            item.id.0
        )
    );
}

#[tokio::test]
async fn a_runs_next_step_waits_once_its_item_is_at_its_budget() {
    let fixture = Fixture::with_budget(4).await;
    let item = fixture.item("a", "question", Priority::NORMAL).await;
    let run = fixture.run(item.id, &RESEARCH).await;
    fixture.budget(item.id, 30).await;
    let session = fixture.session("a-1", "a").await;

    let dispatched = fixture
        .dispatch_after(NOW, &[session], BTreeMap::from([(item.id, 30 * 60)]))
        .await;

    assert!(dispatched.placed.is_empty());
    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::OverBudget);
    assert_eq!(
        explanation.unit,
        Unit::Step {
            run: run.id,
            position: 1,
            name: "investigate",
        }
    );
}

#[tokio::test]
async fn an_item_without_a_budget_is_placed_however_long_it_was_worked() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;

    let dispatched = fixture
        .dispatch_after(NOW, &[session], BTreeMap::from([(item.id, 100 * HOUR)]))
        .await;

    assert_eq!(dispatched.placed.len(), 1);
}

#[tokio::test]
async fn an_unqueued_item_past_its_budget_still_says_it_is_not_queued() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.item("a", "task", Priority::NORMAL).await;
    fixture.budget(item.id, 60).await;

    let dispatched = fixture
        .dispatch_after(NOW, &[session], BTreeMap::from([(item.id, 2 * HOUR)]))
        .await;

    assert_eq!(
        explanation_of(&dispatched.explanations, item.id).reason,
        Reason::NotQueued
    );
}

#[tokio::test]
async fn an_over_budget_explanation_survives_a_reopen() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.budget(item.id, 60).await;
    fixture
        .dispatch_after(NOW, &[session], BTreeMap::from([(item.id, 2 * HOUR)]))
        .await;

    let fixture = fixture.reopen().await;

    let explanations = fixture.explanations().await;
    assert_eq!(
        explanation_of(&explanations, item.id).reason,
        Reason::OverBudget
    );
}
