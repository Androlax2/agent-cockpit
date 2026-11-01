//! Which session takes waiting work: one of its project, free, authorized, idle past the
//! window, not viewed, of the right role and provider; and when a placement ends.

mod common;

use cockpit_provider_api::Capabilities;
use cockpit_scheduling::{Candidate, Presence, Reason, Role, Unit};
use cockpit_tasks::{Item, Priority, Status};
use cockpit_workflows::RESEARCH;
use common::{Fixture, NOW, explanation_of, idle};

async fn one_item() -> (Fixture, Item) {
    let fixture = Fixture::with_budget(4).await;
    let item = fixture.queued("a", "task", Priority::NORMAL).await;
    (fixture, item)
}

/// Why the only session of project `a` does not take its queued item.
async fn refusal_of(candidate: Candidate, viewed: Option<&str>) -> String {
    let (fixture, item) = one_item().await;
    let session = fixture.session("a-1", "a").await;
    let candidate = Candidate {
        session: session.session,
        ..candidate
    };

    let dispatched = fixture.dispatch_viewing(NOW, &[candidate], viewed).await;

    assert!(dispatched.placed.is_empty(), "{:?}", dispatched.placed);
    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::NoSessionFree);
    assert_eq!(explanation.sessions.len(), 1);
    explanation.sessions[0].why.clone()
}

#[tokio::test]
async fn an_idle_session_of_the_project_takes_the_queued_item() {
    let (fixture, item) = one_item().await;
    let session = fixture.session("a-1", "a").await;

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    assert_eq!(dispatched.placed.len(), 1);
    assert_eq!(dispatched.placed[0].item, item.id);
    assert_eq!(dispatched.placed[0].session.as_str(), "a-1");
    let read = fixture.database.read().await.unwrap();
    let started = cockpit_tasks::item(&read, &cockpit_tasks::Actor::Owner, item.id)
        .await
        .unwrap();
    assert_eq!(started.status, Status::Doing);
    assert_eq!(started.doing_session.unwrap().as_str(), "a-1");
}

#[tokio::test]
async fn a_session_of_another_project_never_takes_it() {
    let (fixture, item) = one_item().await;
    let elsewhere = fixture.session("b-1", "b").await;

    let dispatched = fixture.dispatch_at(NOW, &[elsewhere]).await;

    assert!(dispatched.placed.is_empty());
    assert_eq!(
        explanation_of(&dispatched.explanations, item.id).reason,
        Reason::NoSession
    );
}

#[tokio::test]
async fn the_session_waiting_longest_takes_it() {
    let (fixture, item) = one_item().await;
    let recent = Candidate {
        presence: Presence::Waiting { since: NOW - 600 },
        ..fixture.session("a-1", "a").await
    };
    let longest = Candidate {
        presence: Presence::Waiting { since: NOW - 7200 },
        ..fixture.session("a-2", "a").await
    };

    let dispatched = fixture.dispatch_at(NOW, &[recent, longest]).await;

    assert_eq!(dispatched.placed[0].item, item.id);
    assert_eq!(dispatched.placed[0].session.as_str(), "a-2");
}

#[tokio::test]
async fn each_unready_presence_names_why_the_session_cannot_take_it() {
    let cases = [
        (
            Presence::Inactive {
                state: "asleep".to_owned(),
            },
            "it is asleep",
        ),
        (
            Presence::Unreported,
            "its agent has not reported since its launch",
        ),
        (Presence::Working, "its agent is working"),
        (Presence::NeedsInput, "it waits for your answer"),
        (
            Presence::Waiting { since: NOW - 60 },
            "idle window: 240 s left before it gets new work",
        ),
    ];
    for (presence, expected) in cases {
        let candidate = Candidate {
            presence: presence.clone(),
            ..idle("a-1", "a")
        };
        assert_eq!(refusal_of(candidate, None).await, expected, "{presence:?}");
    }
}

#[tokio::test]
async fn the_session_the_owner_views_gets_no_work() {
    let why = refusal_of(idle("a-1", "a"), Some("a-1")).await;

    assert_eq!(why, "you are viewing it");
}

#[tokio::test]
async fn a_session_holding_a_role_takes_only_that_roles_steps() {
    let candidate = Candidate {
        role: Some(Role::Designer),
        ..idle("a-1", "a")
    };

    let why = refusal_of(candidate, None).await;

    assert_eq!(
        why,
        "it holds the designer role and takes only that role's steps"
    );
}

#[tokio::test]
async fn a_session_without_the_change_state_grant_gets_no_work() {
    let (fixture, item) = one_item().await;

    let dispatched = fixture.dispatch_at(NOW, &[idle("a-1", "a")]).await;

    assert!(dispatched.placed.is_empty());
    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(
        explanation.sessions[0].why,
        "it may not change its own state: the owner grants it change-state over itself"
    );
}

#[tokio::test]
async fn a_session_doing_an_item_takes_no_second_one() {
    let (fixture, first) = one_item().await;
    let session = fixture.session("a-1", "a").await;
    fixture
        .dispatch_at(NOW, std::slice::from_ref(&session))
        .await;
    let second = fixture.queued("a", "second", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW + 60, &[session]).await;

    assert!(dispatched.placed.is_empty());
    let explanation = explanation_of(&dispatched.explanations, second.id);
    assert_eq!(
        explanation.sessions[0].why,
        format!("it is doing item {}", first.id.0)
    );
}

#[tokio::test]
async fn one_session_takes_one_unit_per_pass() {
    let (fixture, first) = one_item().await;
    let second = fixture.queued("a", "second", Priority::NORMAL).await;
    let session = fixture.session("a-1", "a").await;

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    assert_eq!(dispatched.placed.len(), 1);
    assert_eq!(
        explanation_of(&dispatched.explanations, second.id).sessions[0].why,
        format!("it takes item {} in this pass", first.id.0)
    );
}

#[tokio::test]
async fn a_ready_agent_step_is_begun_by_the_session() {
    let fixture = Fixture::with_budget(4).await;
    let item = fixture.item("a", "question", Priority::NORMAL).await;
    let run = fixture.run(item.id, &RESEARCH).await;
    let session = fixture.session("a-1", "a").await;

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    assert_eq!(
        dispatched.placed[0].unit,
        Unit::Step {
            run: run.id,
            position: 1,
            name: "investigate",
        }
    );
    let read = fixture.database.read().await.unwrap();
    let begun = cockpit_workflows::run(&read, &cockpit_workflows::Actor::Owner, run.id)
        .await
        .unwrap();
    assert_eq!(begun.steps[0].session.as_ref().unwrap().as_str(), "a-1");
}

#[tokio::test]
async fn a_session_whose_provider_lacks_the_steps_capabilities_does_not_take_it() {
    let fixture = Fixture::with_budget(4).await;
    let item = fixture.item("a", "question", Priority::NORMAL).await;
    fixture.run(item.id, &RESEARCH).await;
    let session = Candidate {
        capabilities: Capabilities::none(),
        ..fixture.session("a-1", "a").await
    };

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    assert!(dispatched.placed.is_empty());
    let explanation = explanation_of(&dispatched.explanations, item.id);
    assert_eq!(explanation.reason, Reason::NoSessionFree);
    assert!(
        explanation.sessions[0].why.contains("events"),
        "{}",
        explanation.sessions[0].why
    );
}

#[tokio::test]
async fn a_placement_ends_when_its_item_is_finished() {
    let (fixture, item) = one_item().await;
    let session = fixture.session("a-1", "a").await;
    let placed = fixture
        .dispatch_at(NOW, std::slice::from_ref(&session))
        .await
        .placed;
    fixture.finish("a-1", "a", item.id).await;

    let dispatched = fixture.dispatch_at(NOW + 60, &[session]).await;

    assert_eq!(dispatched.ended, [placed[0].id]);
    let read = fixture.database.read().await.unwrap();
    assert!(
        cockpit_scheduling::placements(&read)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_placement_ends_when_its_session_is_gone() {
    let (fixture, _) = one_item().await;
    let session = fixture.session("a-1", "a").await;
    let placed = fixture.dispatch_at(NOW, &[session]).await.placed;

    let dispatched = fixture.dispatch_at(NOW + 60, &[]).await;

    assert_eq!(dispatched.ended, [placed[0].id]);
}

#[tokio::test]
async fn a_placement_lasts_while_its_session_does_the_work() {
    let (fixture, _) = one_item().await;
    let session = fixture.session("a-1", "a").await;
    let placed = fixture
        .dispatch_at(NOW, std::slice::from_ref(&session))
        .await
        .placed;

    let dispatched = fixture.dispatch_at(NOW + 60, &[session]).await;

    assert!(dispatched.ended.is_empty());
    let read = fixture.database.read().await.unwrap();
    assert_eq!(cockpit_scheduling::placements(&read).await.unwrap(), placed);
}
