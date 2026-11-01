//! Projects share the budget turn by turn, whatever their session count; inside a project the
//! highest aged priority goes first; a large unit that waited long gathers capacity; a
//! project nothing finished in yet is charged startup headroom.

mod common;

use cockpit_scheduling::Reason;
use cockpit_tasks::Priority;
use common::{Fixture, NOW, TURN, explanation_of, placed_projects};

/// The acceptance fixture: project `a` fans out to three sessions, `b` and `c` have one
/// each, every project has three queued items and the budget fits three agent turns.
async fn three_projects() -> (Fixture, Vec<cockpit_scheduling::Candidate>) {
    let fixture = Fixture::with_budget(3).await;
    let mut candidates = Vec::new();
    for (project, sessions) in [("a", 3), ("b", 1), ("c", 1)] {
        for number in 1..=sessions {
            candidates.push(
                fixture
                    .session(&format!("{project}-{number}"), project)
                    .await,
            );
        }
        for number in 1..=3 {
            fixture
                .queued(
                    project,
                    &format!("{project} task {number}"),
                    Priority::NORMAL,
                )
                .await;
        }
    }
    (fixture, candidates)
}

#[tokio::test]
async fn three_projects_sharing_the_budget_each_get_one_turn_whatever_their_session_count() {
    let (fixture, candidates) = three_projects().await;

    let dispatched = fixture.dispatch_at(NOW, &candidates).await;

    assert_eq!(placed_projects(&dispatched), ["a", "b", "c"]);
}

#[tokio::test]
async fn work_left_over_after_the_fair_share_waits_for_room() {
    let (fixture, candidates) = three_projects().await;

    let dispatched = fixture.dispatch_at(NOW, &candidates).await;

    let reasons: Vec<(&str, Reason)> = dispatched
        .explanations
        .iter()
        .map(|explanation| (explanation.project.as_str(), explanation.reason))
        .collect();
    assert_eq!(
        reasons,
        [
            ("a", Reason::NoRoom),
            ("a", Reason::NoRoom),
            ("b", Reason::NoSessionFree),
            ("b", Reason::NoSessionFree),
            ("c", Reason::NoSessionFree),
            ("c", Reason::NoSessionFree),
        ]
    );
}

#[tokio::test]
async fn released_capacity_goes_to_the_project_with_the_fewest_placed_turns() {
    let (fixture, candidates) = three_projects().await;
    let first = fixture.dispatch_at(NOW, &candidates).await;
    let b_placement = first
        .placed
        .iter()
        .find(|placement| placement.project == "b")
        .unwrap();
    fixture.finish("b-1", "b", b_placement.item).await;

    let second = fixture.dispatch_at(NOW + 60, &candidates).await;

    assert_eq!(second.ended, [b_placement.id]);
    assert_eq!(placed_projects(&second), ["b"]);
}

#[tokio::test]
async fn the_same_state_gives_the_same_placements() {
    let (left, left_candidates) = three_projects().await;
    let (right, right_candidates) = three_projects().await;

    let left = left.dispatch_at(NOW, &left_candidates).await;
    let right = right.dispatch_at(NOW, &right_candidates).await;

    let placed = |dispatched: &cockpit_scheduling::Dispatched| {
        dispatched
            .placed
            .iter()
            .map(|placement| (placement.item, placement.session.as_str().to_owned()))
            .collect::<Vec<_>>()
    };
    assert_eq!(placed(&left), placed(&right));
    assert_eq!(left.explanations, right.explanations);
}

#[tokio::test]
async fn the_higher_priority_goes_first_inside_a_project() {
    let fixture = Fixture::with_budget(1).await;
    let session = fixture.session("a-1", "a").await;
    fixture.queued("a", "routine", Priority::NORMAL).await;
    let urgent = fixture.queued("a", "urgent", Priority::HIGH).await;

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    assert_eq!(dispatched.placed[0].item, urgent.id);
}

#[tokio::test]
async fn a_low_priority_item_that_waited_an_hour_passes_a_new_high_priority_one() {
    let fixture = Fixture::with_budget(1).await;
    let old = fixture.queued("a", "old chore", Priority::LOW).await;
    fixture.dispatch_at(NOW, &[]).await;
    fixture.queued("a", "new feature", Priority::HIGH).await;
    let session = fixture.session("a-1", "a").await;

    let dispatched = fixture.dispatch_at(NOW + 3600, &[session]).await;

    assert_eq!(dispatched.placed[0].item, old.id);
}

#[tokio::test]
async fn aging_adds_one_level_per_fifteen_minutes_up_to_four() {
    let fixture = Fixture::with_budget(1).await;
    let item = fixture.queued("a", "chore", Priority::LOW).await;

    for (waited, boost) in [(0, 0), (15 * 60, 1), (45 * 60, 3), (10 * 3600, 4)] {
        let dispatched = fixture.dispatch_at(NOW + waited, &[]).await;
        let explanation = explanation_of(&dispatched.explanations, item.id);
        assert_eq!(explanation.aging_boost, boost, "after {waited} s");
        assert_eq!(explanation.waiting_since, Some(NOW));
    }
}

#[tokio::test]
async fn an_unfamiliar_project_is_charged_startup_headroom() {
    let mut fixture = Fixture::with_budget(2).await;
    fixture.settings.startup_headroom_percent = 50;
    let session = fixture.session("a-1", "a").await;
    fixture.queued("a", "first", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW, &[session]).await;

    assert_eq!(
        dispatched.placed[0].demand.cpu_millis,
        TURN.cpu_millis * 3 / 2
    );
    assert_eq!(
        dispatched.placed[0].demand.memory_mib,
        TURN.memory_mib * 3 / 2
    );
}

#[tokio::test]
async fn startup_headroom_ends_once_a_placement_finished_in_the_project() {
    let mut fixture = Fixture::with_budget(2).await;
    fixture.settings.startup_headroom_percent = 50;
    let first_session = fixture.session("a-1", "a").await;
    let second_session = fixture.session("a-2", "a").await;
    let first = fixture.queued("a", "first", Priority::NORMAL).await;
    fixture.dispatch_at(NOW, std::slice::from_ref(&first_session)).await;
    fixture.finish("a-1", "a", first.id).await;
    fixture.queued("a", "second", Priority::NORMAL).await;
    fixture.queued("a", "third", Priority::NORMAL).await;

    let dispatched = fixture
        .dispatch_at(NOW + 60, &[first_session, second_session])
        .await;

    let demands: Vec<_> = dispatched
        .placed
        .iter()
        .map(|placement| placement.demand)
        .collect();
    assert_eq!(demands, [TURN, TURN]);
}

/// Project `small` is familiar and holds one turn of a two-turn budget; project `big`,
/// queued at `NOW + 120`, is unfamiliar, so its turn (with 50 % headroom) does not fit in
/// the turn left.
async fn big_waits_beside_small() -> (Fixture, Vec<cockpit_scheduling::Candidate>, i64) {
    let mut fixture = Fixture::with_budget(2).await;
    fixture.settings.startup_headroom_percent = 50;
    let candidates = vec![
        fixture.session("small-1", "small").await,
        fixture.session("small-2", "small").await,
        fixture.session("big-1", "big").await,
    ];
    let warmup = fixture.queued("small", "warm-up", Priority::NORMAL).await;
    fixture.dispatch_at(NOW, &candidates[..1]).await;
    fixture.finish("small-1", "small", warmup.id).await;
    fixture.queued("small", "running", Priority::NORMAL).await;
    let running = fixture.dispatch_at(NOW + 60, &candidates).await;
    assert_eq!(placed_projects(&running), ["small"]);
    let big = fixture.queued("big", "large job", Priority::NORMAL).await;
    let dispatched = fixture.dispatch_at(NOW + 120, &candidates).await;
    assert!(dispatched.placed.is_empty(), "{:?}", dispatched.placed);
    assert_eq!(
        explanation_of(&dispatched.explanations, big.id).reason,
        Reason::NoRoom
    );
    (fixture, candidates, big.id.0)
}

#[tokio::test]
async fn smaller_work_passes_a_large_unit_that_waited_less_than_thirty_minutes() {
    let (fixture, candidates, _) = big_waits_beside_small().await;
    fixture.queued("small", "small job", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW + 180, &candidates).await;

    assert_eq!(placed_projects(&dispatched), ["small"]);
}

#[tokio::test]
async fn a_large_unit_that_waited_thirty_minutes_holds_later_work_behind_it() {
    let (fixture, candidates, big) = big_waits_beside_small().await;
    let small = fixture.queued("small", "small job", Priority::NORMAL).await;

    let dispatched = fixture.dispatch_at(NOW + 120 + 30 * 60, &candidates).await;

    assert!(dispatched.placed.is_empty(), "{:?}", dispatched.placed);
    let held = explanation_of(&dispatched.explanations, small.id);
    assert_eq!(held.reason, Reason::Behind);
    assert_eq!(held.behind.map(|item| item.0), Some(big));
    let gathering = explanation_of(&dispatched.explanations, cockpit_tasks::ItemId(big));
    assert!(
        gathering.why.contains("gather capacity"),
        "{}",
        gathering.why
    );
}
