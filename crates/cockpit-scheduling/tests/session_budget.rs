//! Per-session budgets (#412, owner answer A): once a session's turns since it started reach
//! the agent minutes the owner gave it, the queue places nothing more on it and asks the
//! owner. Work already placed runs on; another session of the project still takes work.

mod common;

use std::collections::BTreeMap;

use cockpit_scheduling::{ActionBy, Candidate, Economy, Presence, Reason, SessionBudget};
use cockpit_tasks::Priority;
use common::{Fixture, NOW, explanation_of};

const HOUR: u64 = 60 * 60;

fn budgeted(session: Candidate, minutes: u32, worked_seconds: u64) -> Candidate {
    Candidate {
        budget: Some(SessionBudget {
            minutes,
            worked_seconds,
        }),
        ..session
    }
}

#[tokio::test]
async fn a_session_under_its_budget_takes_work_and_one_at_it_does_not() {
    for (worked_seconds, is_placed) in [(HOUR - 1, true), (HOUR, false)] {
        let fixture = Fixture::with_budget(4).await;
        let session = budgeted(fixture.session("a-1", "a").await, 60, worked_seconds);
        fixture.queued("a", "task", Priority::NORMAL).await;

        let dispatched = fixture.dispatch_at(NOW, &[session]).await;

        assert_eq!(
            !dispatched.placed.is_empty(),
            is_placed,
            "{worked_seconds} s worked"
        );
    }
}

#[tokio::test]
async fn work_waits_for_the_owner_when_its_only_session_spent_its_budget() {
    let fixture = Fixture::with_budget(4).await;
    let session = budgeted(fixture.session("a-1", "a").await, 60, HOUR + 7 * 60);
    let item = fixture.queued("a", "task", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::SessionOverBudget);
    assert_eq!(
        explanation.why,
        "session a-1 has worked about 67 min since it started, its 60 min budget"
    );
    assert_eq!(
        explanation.action,
        "give session a-1 30 min more: `torlek session budget a-1 97`"
    );
    assert_eq!(explanation.action_by, ActionBy::Owner);
    assert!(explanation.reason.is_queued());
}

#[tokio::test]
async fn work_goes_to_another_session_of_the_project_within_its_budget() {
    let fixture = Fixture::with_budget(4).await;
    let spent = budgeted(fixture.session("a-1", "a").await, 60, 2 * HOUR);
    let within = budgeted(fixture.session("a-2", "a").await, 60, 0);
    fixture.queued("a", "task", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[spent, within]).await;

    assert_eq!(dispatched.placed.len(), 1, "{:?}", dispatched.explanations);
    assert_eq!(dispatched.placed[0].session.as_str(), "a-2");
}

#[tokio::test]
async fn work_waits_for_a_busy_session_rather_than_the_owner() {
    let fixture = Fixture::with_budget(4).await;
    let spent = budgeted(fixture.session("a-1", "a").await, 60, 2 * HOUR);
    let busy = Candidate {
        presence: Presence::Working,
        ..fixture.session("a-2", "a").await
    };
    let item = fixture.queued("a", "task", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[spent, busy]).await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::NoSessionFree);
    let notes: Vec<_> = explanation
        .sessions
        .iter()
        .map(|note| (note.session.as_str(), note.why.as_str()))
        .collect();
    assert_eq!(
        notes,
        [
            ("a-2", "its agent is working"),
            (
                "a-1",
                "has worked about 120 min since it started, its 60 min budget"
            ),
        ]
    );
}

#[tokio::test]
async fn several_spent_sessions_are_each_noted() {
    let fixture = Fixture::with_budget(4).await;
    let first = budgeted(fixture.session("a-1", "a").await, 60, 2 * HOUR);
    let second = budgeted(fixture.session("a-2", "a").await, 30, HOUR);
    let item = fixture.queued("a", "task", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[first, second]).await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::SessionOverBudget);
    assert_eq!(
        explanation.why,
        "the 2 sessions of project a have each worked their budget of agent minutes"
    );
    assert_eq!(explanation.sessions.len(), 2);
}

#[tokio::test]
async fn a_session_that_cannot_free_on_its_own_leaves_the_spent_budget_to_the_owner() {
    let fixture = Fixture::with_budget(4).await;
    let spent = budgeted(fixture.session("a-1", "a").await, 60, 2 * HOUR);
    let asleep = Candidate {
        presence: Presence::Inactive {
            state: "asleep".to_owned(),
        },
        ..fixture.session("a-2", "a").await
    };
    let item = fixture.queued("a", "task", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[spent, asleep]).await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::SessionOverBudget);
    assert_eq!(explanation.sessions.len(), 2);
}

#[tokio::test]
async fn an_economy_item_whose_economy_session_spent_its_budget_asks_the_owner() {
    let fixture = Fixture::with_budget(4).await;
    let full = fixture.session("a-1", "a").await;
    let economy = Candidate {
        effort: Some("low".to_owned()),
        ..budgeted(fixture.session("a-2", "a").await, 60, 2 * HOUR)
    };
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.mark_economy(item.id).await;

    let dispatched = fixture
        .dispatch_economy(
            NOW,
            &[full, economy],
            BTreeMap::from([(
                "a".to_owned(),
                Economy {
                    model: None,
                    effort: Some("low".to_owned()),
                },
            )]),
        )
        .await;

    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::SessionOverBudget);
    assert!(
        explanation.action.contains("torlek session budget a-2 150"),
        "{}",
        explanation.action
    );
}

#[tokio::test]
async fn the_offer_of_more_time_goes_to_the_session_least_past_its_budget() {
    let fixture = Fixture::with_budget(4).await;
    let far = budgeted(fixture.session("a-1", "a").await, 60, 200 * 60);
    let near = budgeted(fixture.session("a-2", "a").await, 60, 61 * 60);
    let item = fixture.queued("a", "task", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[far, near]).await;

    assert_eq!(
        explanation_of(&dispatched.explanations, item.id).action,
        "give session a-2 30 min more: `torlek session budget a-2 91`"
    );
}

#[tokio::test]
async fn a_session_over_budget_explanation_survives_a_reopen() {
    let fixture = Fixture::with_budget(4).await;
    let session = budgeted(fixture.session("a-1", "a").await, 60, 2 * HOUR);
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.dispatch_at(NOW, &[session]).await;

    let fixture = fixture.reopen().await;

    assert_eq!(
        explanation_of(&fixture.explanations().await, item.id).reason,
        Reason::SessionOverBudget
    );
}

#[tokio::test]
async fn the_owner_hands_work_to_a_session_past_its_budget() {
    let fixture = Fixture::with_budget(4).await;
    let session = budgeted(fixture.session("a-1", "a").await, 60, 2 * HOUR);
    let item = fixture.item("a", "task", Priority::NORMAL).await;
    let write = fixture.database.write().await.unwrap();

    let placed = cockpit_scheduling::handoff(
        &write,
        &cockpit_scheduling::Handoff {
            item: item.id,
            intent_revision: item.intent_revision,
            state_revision: item.state_revision,
            session: session.session.clone(),
        },
        &cockpit_scheduling::Inputs {
            candidates: vec![session],
            viewed: None,
            usage: Vec::new(),
            agent_seconds: BTreeMap::new(),
            economy: BTreeMap::new(),
            now: NOW + 1,
        },
        &fixture.settings,
        &fixture.policy,
        &[],
    )
    .await;

    assert_eq!(placed.unwrap().item, item.id);
}
