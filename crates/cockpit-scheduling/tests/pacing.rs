//! Usage-aware pacing (#412): queue starts on an account follow an even line through its
//! plan windows to their reset, keep the owner's reserve, and say why they wait.

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use cockpit_scheduling::{ActionBy, Candidate, Pacing, Presence, Reason, Standing, UsageWindow};
use cockpit_tasks::{ItemId, Priority};
use common::{ACCOUNT, Fixture, NOW, explanation_of};

const FIVE_HOURS: u32 = 5 * 60 * 60;

/// The five-hour window of the fixture account, opened `elapsed_minutes` before [`NOW`].
fn window(used_percent: f64, elapsed_minutes: i64) -> UsageWindow {
    UsageWindow {
        provider: "claude".to_owned(),
        account: ACCOUNT.to_owned(),
        limit: "five-hour".to_owned(),
        scope: None,
        used_percent,
        length_seconds: Some(FIVE_HOURS),
        resets_at: Some(NOW - elapsed_minutes * 60 + i64::from(FIVE_HOURS)),
        standing: Standing::Current,
    }
}

/// One queued item of `priority` and one free session of project `a`.
async fn one_item(priority: Priority) -> (Fixture, ItemId, Candidate) {
    let fixture = Fixture::with_budget(4).await;
    let session = fixture.session("a-1", "a").await;
    let item = fixture.queued("a", "task", priority).await;
    (fixture, item.id, session)
}

async fn is_placed(priority: Priority, usage: Vec<UsageWindow>) -> bool {
    let (fixture, _, session) = one_item(priority).await;
    !fixture
        .dispatch_using(NOW, &[session], usage)
        .await
        .placed
        .is_empty()
}

#[tokio::test]
async fn work_starts_while_the_window_is_below_its_line_and_waits_past_it() {
    // The default line: elapsed share × 90 % + 15 %, capped at 90 %; low priority gets no
    // burst, high priority only stops at the reserve.
    let cases = [
        (Priority::NORMAL, 0, 14.0, true),
        (Priority::NORMAL, 0, 15.0, false),
        (Priority::NORMAL, 150, 59.0, true),
        (Priority::NORMAL, 150, 61.0, false),
        (Priority::NORMAL, 295, 89.0, true),
        (Priority::LOW, 150, 44.0, true),
        (Priority::LOW, 150, 46.0, false),
        (Priority::HIGH, 0, 80.0, true),
        (Priority::HIGH, 290, 90.0, false),
    ];
    for (priority, elapsed, used, expected) in cases {
        assert_eq!(
            is_placed(priority, vec![window(used, elapsed)]).await,
            expected,
            "priority {priority:?}, {elapsed} min into the window, {used} % used"
        );
    }
}

#[tokio::test]
async fn a_paced_item_says_which_window_holds_it_and_when_it_starts() {
    let (fixture, item, session) = one_item(Priority::NORMAL).await;

    // Half way, 70 % used: the line (0.5 × 90 + 15 = 60 %) reaches 70 % at 55/90 of the
    // window, 33 min 20 s from now.
    let dispatched = fixture
        .dispatch_using(NOW, &[session], vec![window(70.0, 150)])
        .await;

    assert!(dispatched.placed.is_empty(), "{:?}", dispatched.placed);
    let explanation = explanation_of(&dispatched.explanations, item);
    assert_eq!(explanation.reason, Reason::UsagePaced);
    assert_eq!(
        explanation.why,
        "window five-hour of claude account work is 70 % used, ahead of its even pace to the \
         reset (60 % now)"
    );
    assert_eq!(
        explanation.limited_by.as_deref(),
        Some("window five-hour of claude account work")
    );
    assert!(
        explanation
            .action
            .starts_with("wait: it starts once the window allows it, in about 34 min"),
        "{}",
        explanation.action
    );
    assert_eq!(explanation.action_by, ActionBy::Cockpit);
}

#[tokio::test]
async fn each_paced_session_is_noted_with_its_window() {
    let (fixture, item, session) = one_item(Priority::NORMAL).await;

    let dispatched = fixture
        .dispatch_using(NOW, &[session], vec![window(70.0, 150)])
        .await;

    let notes = &explanation_of(&dispatched.explanations, item).sessions;
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].session.as_str(), "a-1");
    assert!(notes[0].why.contains("70 % used"), "{}", notes[0].why);
}

#[tokio::test]
async fn a_paced_explanation_survives_a_reopen() {
    let (fixture, item, session) = one_item(Priority::NORMAL).await;
    fixture
        .dispatch_using(NOW, &[session], vec![window(70.0, 150)])
        .await;

    let fixture = fixture.reopen().await;

    let explanations = fixture.explanations().await;
    assert_eq!(
        explanation_of(&explanations, item).reason,
        Reason::UsagePaced
    );
}

#[tokio::test]
async fn at_the_reserve_work_waits_until_the_window_resets() {
    let (fixture, item, session) = one_item(Priority::HIGHEST).await;

    let dispatched = fixture
        .dispatch_using(NOW, &[session], vec![window(92.0, 240)])
        .await;

    let explanation = explanation_of(&dispatched.explanations, item);
    assert_eq!(explanation.reason, Reason::UsagePaced);
    assert!(
        explanation
            .why
            .ends_with("at the 90 % the queue may use; the rest is kept for your own sessions"),
        "{}",
        explanation.why
    );
    assert!(
        explanation.action.contains("in about 1 h 0 min"),
        "{}",
        explanation.action
    );
}

#[tokio::test]
async fn a_blocked_account_holds_even_the_highest_priority() {
    let blocked = UsageWindow {
        standing: Standing::Blocked,
        ..window(10.0, 60)
    };
    let (fixture, item, session) = one_item(Priority::HIGHEST).await;

    let dispatched = fixture.dispatch_using(NOW, &[session], vec![blocked]).await;

    let explanation = explanation_of(&dispatched.explanations, item);
    assert_eq!(explanation.reason, Reason::UsagePaced);
    assert_eq!(
        explanation.action,
        "wait: it starts once a reading shows the account usable again"
    );
}

#[tokio::test]
async fn a_blocked_window_scoped_to_one_model_still_holds_the_account() {
    let blocked = UsageWindow {
        scope: Some("opus".to_owned()),
        standing: Standing::Blocked,
        ..window(10.0, 60)
    };

    assert!(!is_placed(Priority::HIGHEST, vec![blocked]).await);
}

#[tokio::test]
async fn windows_that_cannot_pace_this_session_leave_it_free() {
    let cases = [
        (
            "a stale reading",
            UsageWindow {
                standing: Standing::Stale,
                ..window(99.0, 60)
            },
        ),
        (
            "a window scoped to one model",
            UsageWindow {
                scope: Some("opus".to_owned()),
                ..window(99.0, 60)
            },
        ),
        (
            "another account's window",
            UsageWindow {
                account: "personal".to_owned(),
                ..window(99.0, 60)
            },
        ),
        (
            "another provider's window",
            UsageWindow {
                provider: "codex".to_owned(),
                ..window(99.0, 60)
            },
        ),
    ];
    for (case, usage) in cases {
        assert!(is_placed(Priority::NORMAL, vec![usage]).await, "{case}");
    }
}

#[tokio::test]
async fn a_session_on_no_known_account_is_not_paced() {
    let (fixture, _, session) = one_item(Priority::NORMAL).await;
    let session = Candidate {
        account: None,
        ..session
    };

    let dispatched = fixture
        .dispatch_using(NOW, &[session], vec![window(99.0, 60)])
        .await;

    assert_eq!(dispatched.placed.len(), 1);
}

#[tokio::test]
async fn the_owner_turns_pacing_off() {
    let (mut fixture, _, session) = one_item(Priority::NORMAL).await;
    fixture.settings.pacing = Pacing::Off;

    let dispatched = fixture
        .dispatch_using(NOW, &[session], vec![window(99.0, 60)])
        .await;

    assert_eq!(dispatched.placed.len(), 1);
}

#[tokio::test]
async fn a_session_on_an_account_with_room_takes_the_work_of_a_paced_one() {
    let (fixture, _, paced) = one_item(Priority::NORMAL).await;
    let other = Candidate {
        account: Some("personal".to_owned()),
        ..fixture.session("a-2", "a").await
    };

    let dispatched = fixture
        .dispatch_using(NOW, &[paced, other], vec![window(70.0, 150)])
        .await;

    assert_eq!(dispatched.placed.len(), 1);
    assert_eq!(dispatched.placed[0].session.as_str(), "a-2");
}

/// The recorded feed's sessions: four sessions of project `a`, each spending 10 % of the
/// window per hour while it works; full speed spends the window in half its length.
const SESSIONS: usize = 4;
const PERCENT_PER_SESSION_HOUR: f64 = 10.0;
/// Between two passes, and between two readings of the window.
const TICK: i64 = 5 * 60;
/// How long an agent works on one item.
const WORK: i64 = 10 * 60;

/// What a simulated window showed: its share used at each pass, and the share used at each
/// queue start.
struct Feed {
    used_at: Vec<(i64, f64)>,
    used_at_start: Vec<(i64, f64)>,
}

/// Runs the queue from a window's opening at [`NOW`] to its reset, one pass per [`TICK`]:
/// sessions finish their items after [`WORK`], the window grows with the sessions working,
/// and each pass reads it as a current reading.
async fn simulate(pacing: Pacing) -> Feed {
    let mut fixture = Fixture::with_budget(SESSIONS as u64).await;
    fixture.settings.pacing = pacing;
    fixture.settings.idle_window = Duration::ZERO;
    let mut candidates = Vec::new();
    for number in 1..=SESSIONS {
        candidates.push(fixture.session(&format!("a-{number}"), "a").await);
    }
    for number in 1..=120 {
        fixture
            .queued("a", &format!("task {number}"), Priority::NORMAL)
            .await;
    }
    let resets_at = NOW + i64::from(FIVE_HOURS);
    let mut working: BTreeMap<String, (ItemId, i64)> = BTreeMap::new();
    let mut used = 0.0;
    let mut feed = Feed {
        used_at: Vec::new(),
        used_at_start: Vec::new(),
    };
    let mut now = NOW;
    while now < resets_at {
        for candidate in &mut candidates {
            let session = candidate.session.as_str().to_owned();
            if let Some(&(item, done_at)) = working.get(&session)
                && done_at <= now
            {
                fixture.finish(&session, "a", item).await;
                working.remove(&session);
                candidate.presence = Presence::Waiting { since: now };
            }
        }
        let reading = UsageWindow {
            used_percent: used,
            resets_at: Some(resets_at),
            ..window(0.0, 0)
        };
        let dispatched = fixture
            .dispatch_using(now, &candidates, vec![reading])
            .await;
        for placement in &dispatched.placed {
            let session = placement.session.as_str().to_owned();
            working.insert(session.clone(), (placement.item, now + WORK));
            let candidate = candidates
                .iter_mut()
                .find(|candidate| candidate.session.as_str() == session)
                .unwrap();
            candidate.presence = Presence::Working;
            feed.used_at_start.push((now, used));
        }
        feed.used_at.push((now, used));
        used += working.len() as f64 * PERCENT_PER_SESSION_HOUR * TICK as f64 / 3600.0;
        now += TICK;
    }
    feed.used_at.push((now, used));
    feed
}

#[tokio::test]
async fn at_full_speed_the_recorded_feed_spends_the_window_by_half_its_length() {
    let feed = simulate(Pacing::Off).await;

    let half = NOW + i64::from(FIVE_HOURS) / 2;
    let spent_at = feed
        .used_at
        .iter()
        .find(|(_, used)| *used >= 100.0 - 1e-9)
        .map(|(at, _)| *at);
    assert!(
        spent_at.is_some_and(|at| at <= half),
        "spent at {spent_at:?}, half way at {half}"
    );
}

#[tokio::test]
async fn under_the_default_pace_the_window_lasts_until_its_reset() {
    let feed = simulate(Pacing::DEFAULT).await;

    let (_, used_at_reset) = *feed.used_at.last().unwrap();
    assert!(used_at_reset < 100.0, "{used_at_reset} % used at the reset");
}

#[tokio::test]
async fn under_the_default_pace_work_keeps_starting_until_the_last_hour() {
    let feed = simulate(Pacing::DEFAULT).await;

    let last_hour = NOW + i64::from(FIVE_HOURS) - 3600;
    assert!(
        feed.used_at_start.iter().any(|(at, _)| *at >= last_hour),
        "{:?}",
        feed.used_at_start
    );
}

#[tokio::test]
async fn under_the_default_pace_the_queue_starts_nothing_in_the_owners_reserve() {
    let feed = simulate(Pacing::DEFAULT).await;

    assert!(
        feed.used_at_start.iter().all(|(_, used)| *used < 90.0),
        "{:?}",
        feed.used_at_start
    );
}
