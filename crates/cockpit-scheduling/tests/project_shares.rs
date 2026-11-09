//! Per-project shares (#412, owner choice 1A): a project the owner caps gets no queue starts
//! on an account once its estimated part of one of the account's windows reaches its share.
//! The rest of the window stays free for every project.

mod common;

use cockpit_scheduling::{
    ActionBy, Candidate, Pacing, ProjectShare, Reason, Standing, UsageWindow,
};
use cockpit_tasks::{ItemId, Priority};
use common::{ACCOUNT, Fixture, NOW, explanation_of};

const FIVE_HOURS: u32 = 5 * 60 * 60;

/// The fixture account's five-hour window half way, 40 % used (below the even line), where
/// project `project` has a `share_percent` cap and has used `used_percent` of the window.
fn window_with(project: &str, share_percent: u8, used_percent: f64) -> UsageWindow {
    UsageWindow {
        provider: "claude".to_owned(),
        account: ACCOUNT.to_owned(),
        limit: "five-hour".to_owned(),
        scope: None,
        used_percent: 40.0,
        length_seconds: Some(FIVE_HOURS),
        resets_at: Some(NOW + 150 * 60),
        standing: Standing::Current,
        project_shares: vec![ProjectShare {
            project: project.to_owned(),
            share_percent,
            used_percent,
        }],
    }
}

/// One queued item of `priority` in project `a` and its one free session.
async fn one_item(priority: Priority) -> (Fixture, ItemId, Candidate) {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "task", priority).await;
    (fixture, item.id, session)
}

async fn is_placed(fixture: &Fixture, session: Candidate, window: UsageWindow) -> bool {
    !fixture
        .dispatch_using(NOW, &[session], vec![window])
        .await
        .placed
        .is_empty()
}

#[tokio::test]
async fn a_project_below_its_share_starts_and_one_at_it_waits() {
    for (used, expected) in [(19.9, true), (20.0, false)] {
        let (fixture, _, session) = one_item(Priority::NORMAL).await;

        let placed = is_placed(&fixture, session, window_with("a", 20, used)).await;

        assert_eq!(placed, expected, "{used} % of the window used by project a");
    }
}

#[tokio::test]
async fn a_capped_project_says_which_window_and_share_hold_it_until_the_reset() {
    let (fixture, item, session) = one_item(Priority::NORMAL).await;

    let dispatched = fixture
        .dispatch_using(NOW, &[session], vec![window_with("a", 20, 21.4)])
        .await;

    let explanation = explanation_of(&dispatched.explanations, item);
    assert_eq!(explanation.reason, Reason::UsagePaced);
    assert_eq!(
        explanation.why,
        "project a has used about 21 % of window five-hour of claude account work, at its 20 % \
         share (estimated from its replies' API-price value)"
    );
    assert_eq!(
        explanation.limited_by.as_deref(),
        Some("project a's share of window five-hour of claude account work")
    );
    assert_eq!(
        explanation.action,
        "wait: it starts once the window allows it, in about 2 h 30 min; \
         `[usage.project_shares] a` sets the share"
    );
    assert_eq!(explanation.action_by, ActionBy::Cockpit);
}

#[tokio::test]
async fn the_setting_named_for_a_dotted_project_quotes_its_key() {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "my.app").await;
    let item = fixture.queued("my.app", "task", Priority::NORMAL).await;

    let dispatched = fixture
        .dispatch_using(NOW, &[session], vec![window_with("my.app", 20, 25.0)])
        .await;

    let action = &explanation_of(&dispatched.explanations, item.id).action;
    assert!(
        action.ends_with("`[usage.project_shares] \"my.app\"` sets the share"),
        "{action}"
    );
}

#[tokio::test]
async fn high_priority_work_waits_at_its_projects_share_too() {
    let (fixture, _, session) = one_item(Priority::HIGH).await;

    assert!(!is_placed(&fixture, session, window_with("a", 20, 25.0)).await);
}

#[tokio::test]
async fn another_projects_share_caps_nothing_here() {
    let (fixture, _, session) = one_item(Priority::NORMAL).await;

    assert!(is_placed(&fixture, session, window_with("b", 20, 39.0)).await);
}

#[tokio::test]
async fn a_share_holds_with_pacing_off_since_the_owner_set_it_on_its_own() {
    let (mut fixture, _, session) = one_item(Priority::NORMAL).await;
    fixture.settings.pacing = Pacing::Off;

    assert!(!is_placed(&fixture, session, window_with("a", 20, 25.0)).await);
}

#[tokio::test]
async fn a_stale_or_model_scoped_window_caps_nothing() {
    let stale = UsageWindow {
        standing: Standing::Stale,
        ..window_with("a", 20, 25.0)
    };
    let scoped = UsageWindow {
        scope: Some("opus".to_owned()),
        ..window_with("a", 20, 25.0)
    };
    for window in [stale, scoped] {
        let (fixture, _, session) = one_item(Priority::NORMAL).await;

        assert!(
            is_placed(&fixture, session, window.clone()).await,
            "{window:?}"
        );
    }
}
