//! Economy (#412, owner choice A): an item the owner marks economy goes only to a session of
//! its project started with each of the project's economy model and effort that is set. With
//! none it waits, and
//! the explanation says what to start; the queue never starts or relaunches a session.

mod common;

use std::collections::BTreeMap;

use cockpit_scheduling::{ActionBy, Candidate, Economy, Reason};
use cockpit_tasks::Priority;
use common::{Fixture, NOW, explanation_of};

fn haiku_low() -> BTreeMap<String, Economy> {
    BTreeMap::from([(
        "a".to_owned(),
        Economy {
            model: Some("haiku".to_owned()),
            effort: Some("low".to_owned()),
        },
    )])
}

fn started_with(session: Candidate, model: Option<&str>, effort: Option<&str>) -> Candidate {
    Candidate {
        model: model.map(str::to_owned),
        effort: effort.map(str::to_owned),
        ..session
    }
}

#[tokio::test]
async fn an_economy_item_goes_to_the_session_started_with_the_economy_model_and_effort() {
    let fixture = Fixture::with_budget(4).await;
    let full = started_with(fixture.session("a-1", "a").await, None, None);
    let economy = started_with(
        fixture.session("a-2", "a").await,
        Some("haiku"),
        Some("low"),
    );
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.mark_economy(item.id).await;

    let dispatched = fixture
        .dispatch_economy(NOW, &[full, economy], haiku_low())
        .await;

    assert_eq!(dispatched.placed.len(), 1, "{:?}", dispatched.explanations);
    assert_eq!(dispatched.placed[0].session.as_str(), "a-2");
}

#[tokio::test]
async fn an_economy_item_waits_while_no_session_runs_the_economy() {
    let fixture = Fixture::with_budget(4).await;
    let other_effort = started_with(
        fixture.session("a-1", "a").await,
        Some("haiku"),
        Some("high"),
    );
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.mark_economy(item.id).await;

    let dispatched = fixture
        .dispatch_economy(NOW, &[other_effort], haiku_low())
        .await;

    assert!(dispatched.placed.is_empty());
    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::Economy);
    assert_eq!(
        explanation.why,
        "it runs economy, and no session of project a was started with model haiku, effort low"
    );
    assert_eq!(
        explanation.action,
        "start one: `torlek session new DIR --model haiku --effort low`"
    );
    assert_eq!(explanation.action_by, ActionBy::Owner);
    assert_eq!(explanation.sessions.len(), 1);
    assert_eq!(
        explanation.sessions[0].why,
        "started with model haiku, effort high, not the economy"
    );
    assert!(explanation.reason.is_queued());
}

#[tokio::test]
async fn an_economy_item_waits_while_its_project_has_no_economy() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.mark_economy(item.id).await;

    let dispatched = fixture
        .dispatch_economy(NOW, &[session], BTreeMap::new())
        .await;

    assert!(dispatched.placed.is_empty());
    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::Economy);
    assert_eq!(
        explanation.why,
        "it runs economy, and project a has no economy model or effort"
    );
    assert_eq!(
        explanation.action,
        format!(
            "set them in `[usage.economy.a]` of the owner's config, or run it as usual: \
             `torlek todo economy {} off`",
            item.id.0
        )
    );
    assert_eq!(explanation.action_by, ActionBy::Owner);
}

#[tokio::test]
async fn an_economy_of_effort_alone_takes_any_model() {
    let fixture = Fixture::with_budget(4).await;
    let session = started_with(fixture.session("a-1", "a").await, Some("opus"), Some("low"));
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.mark_economy(item.id).await;
    let economy = BTreeMap::from([(
        "a".to_owned(),
        Economy {
            model: None,
            effort: Some("low".to_owned()),
        },
    )]);

    let dispatched = fixture.dispatch_economy(NOW, &[session], economy).await;

    assert_eq!(dispatched.placed.len(), 1, "{:?}", dispatched.explanations);
}

#[tokio::test]
async fn an_unmarked_item_goes_to_any_session_of_its_project() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    fixture.queued("a", "task", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_economy(NOW, &[session], haiku_low()).await;

    assert_eq!(dispatched.placed.len(), 1);
}

#[tokio::test]
async fn an_economy_explanation_survives_a_reopen() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    fixture.mark_economy(item.id).await;
    fixture.dispatch_economy(NOW, &[session], haiku_low()).await;

    let fixture = fixture.reopen().await;

    assert_eq!(
        explanation_of(&fixture.explanations().await, item.id).reason,
        Reason::Economy
    );
}

#[tokio::test]
async fn the_owner_hands_an_economy_item_to_any_session() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.item("a", "task", Priority::NORMAL).await;
    fixture.mark_economy(item.id).await;
    let write = fixture.database.write().await.unwrap();
    let item = cockpit_tasks::item(&write, &cockpit_tasks::Actor::Owner, item.id)
        .await
        .unwrap();

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
            economy: haiku_low(),
            now: NOW + 1,
        },
        &fixture.settings,
        &fixture.policy,
        &[],
    )
    .await;

    assert_eq!(placed.unwrap().item, item.id);
}
