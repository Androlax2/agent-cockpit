//! Time with the agent: what turn records make of it, per day and per project.
mod common;

use cockpit_usage::{DayTime, Error, ProjectTime, Turn};
use common::{Fixture, clock};

const MINUTE: i64 = 60;

fn project_seconds(time: &DayTime) -> Vec<(&str, u64)> {
    time.projects
        .iter()
        .map(|project| (project.project.as_str(), project.seconds))
        .collect()
}

async fn only_day(fixture: &Fixture, day: &str) -> DayTime {
    let mut times = fixture.time_between(day, day).await.unwrap();
    assert_eq!(times.len(), 1, "{times:#?}");
    times.remove(0)
}

#[tokio::test]
async fn a_turn_is_the_time_it_ran() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 10, 0), clock(0, 10, 20))
        .await;

    let monday = only_day(&fixture, "2026-11-02").await;

    assert_eq!(monday.seconds, 20 * 60);
    assert_eq!(project_seconds(&monday), [("app", 20 * 60)]);
}

#[tokio::test]
async fn turns_less_than_five_minutes_apart_are_one_stretch_and_the_gap_counts() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 10, 0), clock(0, 10, 10))
        .await;
    fixture
        .turn("s1", "app", clock(0, 10, 14), clock(0, 10, 20))
        .await;

    let monday = only_day(&fixture, "2026-11-02").await;

    assert_eq!(monday.seconds, 20 * 60);
}

#[tokio::test]
async fn turns_exactly_five_minutes_apart_are_not_joined() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 10, 0), clock(0, 10, 10))
        .await;
    fixture
        .turn("s1", "app", clock(0, 10, 15), clock(0, 10, 20))
        .await;

    let monday = only_day(&fixture, "2026-11-02").await;

    assert_eq!(monday.seconds, 15 * 60);
}

#[tokio::test]
async fn overlapping_sessions_are_counted_once_never_summed() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 10, 0), clock(0, 10, 30))
        .await;
    fixture
        .turn("s2", "app", clock(0, 10, 10), clock(0, 10, 40))
        .await;
    fixture
        .turn("s3", "docs", clock(0, 10, 20), clock(0, 10, 50))
        .await;

    let monday = only_day(&fixture, "2026-11-02").await;

    // app: 10:00-10:40; docs: 10:20-10:50; together 10:00-10:50, not the sum of the two.
    assert_eq!(monday.seconds, 50 * 60);
    assert_eq!(
        project_seconds(&monday),
        [("app", 40 * 60), ("docs", 30 * 60)]
    );
}

#[tokio::test]
async fn a_stretch_over_midnight_is_split_between_the_two_days() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 23, 50), clock(1, 0, 20))
        .await;

    let times = fixture
        .time_between("2026-11-02", "2026-11-03")
        .await
        .unwrap();

    assert_eq!(
        times.iter().map(|time| time.seconds).collect::<Vec<_>>(),
        [10 * 60, 20 * 60]
    );
}

#[tokio::test]
async fn a_gap_joined_across_the_ranges_edge_counts_inside_it() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 23, 50), clock(0, 23, 58))
        .await;
    fixture
        .turn("s1", "app", clock(1, 0, 1), clock(1, 0, 5))
        .await;

    let tuesday_alone = only_day(&fixture, "2026-11-03").await;
    let both_days = fixture
        .time_between("2026-11-02", "2026-11-03")
        .await
        .unwrap();

    // 23:58 to 00:01 is three minutes: joined, so Tuesday has 00:00-00:05 either way.
    assert_eq!(tuesday_alone.seconds, 5 * MINUTE as u64);
    assert_eq!(both_days[1], tuesday_alone);
}

#[tokio::test]
async fn a_day_without_turns_is_there_with_no_time() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(1, 9, 0), clock(1, 9, 10))
        .await;

    let times = fixture
        .time_between("2026-11-02", "2026-11-04")
        .await
        .unwrap();

    assert_eq!(
        times.iter().map(|time| time.seconds).collect::<Vec<_>>(),
        [0, 10 * 60, 0]
    );
    assert_eq!(times[0].projects, Vec::<ProjectTime>::new());
}

#[tokio::test]
async fn a_turn_recorded_again_counts_once() {
    let fixture = Fixture::new().await;

    let first = fixture
        .turn("s1", "app", clock(0, 10, 0), clock(0, 10, 10))
        .await;
    let replay = fixture
        .turn("s1", "app", clock(0, 10, 0), clock(0, 10, 10))
        .await;

    assert!((first, replay) == (true, false));
    assert_eq!(only_day(&fixture, "2026-11-02").await.seconds, 10 * 60);
}

#[tokio::test]
async fn a_turn_that_ends_before_it_starts_is_refused() {
    let result = Turn::new("s1", "app", clock(0, 10, 10), clock(0, 10, 0));

    assert!(matches!(result, Err(Error::Invalid { field: "turn", .. })));
}

#[tokio::test]
async fn a_turn_without_a_session_is_refused() {
    let result = Turn::new("", "app", clock(0, 10, 0), clock(0, 10, 10));

    assert!(matches!(
        result,
        Err(Error::Invalid {
            field: "session",
            ..
        })
    ));
}

#[tokio::test]
async fn a_turn_in_a_project_without_a_plain_name_is_time_in_unknown() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "bad\nname", clock(0, 10, 0), clock(0, 10, 10))
        .await;

    let monday = only_day(&fixture, "2026-11-02").await;

    assert_eq!(project_seconds(&monday), [("unknown", 10 * 60)]);
}

#[tokio::test]
async fn a_range_that_ends_before_it_starts_is_refused() {
    let fixture = Fixture::new().await;

    let result = fixture.time_between("2026-11-03", "2026-11-02").await;

    assert!(matches!(result, Err(Error::Invalid { field: "days", .. })));
}

#[tokio::test]
async fn a_range_longer_than_a_leap_year_is_refused() {
    let fixture = Fixture::new().await;

    let result = fixture.time_between("2026-01-01", "2027-01-02").await;

    assert!(matches!(result, Err(Error::Invalid { field: "days", .. })));
}

#[tokio::test]
async fn a_turn_recorded_again_with_a_later_end_takes_the_end() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 10, 0), clock(0, 10, 0))
        .await;

    let replay = fixture
        .turn("s1", "app", clock(0, 10, 0), clock(0, 10, 20))
        .await;
    let earlier_end = fixture
        .turn("s1", "app", clock(0, 10, 0), clock(0, 10, 5))
        .await;

    assert!((replay, earlier_end) == (false, false));
    assert_eq!(only_day(&fixture, "2026-11-02").await.seconds, 20 * 60);
}

#[tokio::test]
async fn a_turn_longer_than_a_week_is_refused() {
    let result = Turn::new("s1", "app", clock(0, 10, 0), clock(8, 10, 0));

    assert!(matches!(result, Err(Error::Invalid { field: "turn", .. })));
}

#[tokio::test]
async fn a_turn_before_the_year_2000_is_refused() {
    let result = Turn::new("s1", "app", 0, 60);

    assert!(matches!(
        result,
        Err(Error::Invalid {
            field: "turn start",
            ..
        })
    ));
}

#[tokio::test]
async fn a_turn_that_began_and_ended_is_time_from_its_start_to_its_end() {
    let fixture = Fixture::new().await;

    let opened = fixture.began("s1", "app", clock(0, 10, 0)).await;
    let ended = fixture.ended("s1", clock(0, 10, 20)).await.unwrap();

    assert!((opened, ended) == (true, true));
    assert_eq!(only_day(&fixture, "2026-11-02").await.seconds, 20 * 60);
    assert!(fixture.open_turns().await.is_empty());
}

#[tokio::test]
async fn working_again_while_a_turn_is_open_keeps_its_start() {
    let fixture = Fixture::new().await;
    fixture.began("s1", "app", clock(0, 10, 0)).await;

    let reopened = fixture.began("s1", "app", clock(0, 10, 7)).await;
    fixture.ended("s1", clock(0, 10, 20)).await.unwrap();

    assert!(!reopened);
    assert_eq!(only_day(&fixture, "2026-11-02").await.seconds, 20 * 60);
}

#[tokio::test]
async fn ending_a_session_with_no_open_turn_records_nothing() {
    let fixture = Fixture::new().await;

    let ended = fixture.ended("s1", clock(0, 10, 20)).await.unwrap();

    assert!(!ended);
    assert_eq!(only_day(&fixture, "2026-11-02").await.seconds, 0);
}

#[tokio::test]
async fn an_open_turn_is_listed_with_its_project_until_it_ends() {
    let fixture = Fixture::new().await;
    fixture.began("s1", "app", clock(0, 10, 0)).await;

    let open = fixture.open_turns().await;

    assert_eq!(open.len(), 1);
    assert_eq!(
        (
            open[0].session.as_str(),
            open[0].project.as_str(),
            open[0].started_at
        ),
        ("s1", "app", clock(0, 10, 0))
    );
}

#[tokio::test]
async fn an_end_before_the_start_is_the_start() {
    let fixture = Fixture::new().await;
    fixture.began("s1", "app", clock(0, 10, 0)).await;

    fixture.ended("s1", clock(0, 9, 0)).await.unwrap();

    assert_eq!(only_day(&fixture, "2026-11-02").await.seconds, 0);
    assert!(fixture.open_turns().await.is_empty());
}

#[tokio::test]
async fn an_open_turn_over_a_week_is_refused_and_dropped() {
    let fixture = Fixture::new().await;
    fixture.began("s1", "app", clock(0, 10, 0)).await;

    let result = fixture.ended("s1", clock(9, 10, 0)).await;

    assert!(matches!(result, Err(Error::Invalid { field: "turn", .. })));
    assert!(fixture.open_turns().await.is_empty());
}

async fn turn_seconds(fixture: &Fixture, session: &str, from: i64, until: i64) -> u64 {
    let read = fixture.database.read().await.unwrap();
    cockpit_usage::turn_seconds(&read, session, from, until)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_sessions_turn_seconds_in_a_span_count_only_the_part_of_each_turn_inside_it() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 9, 50), clock(0, 10, 10))
        .await;
    fixture
        .turn("s1", "app", clock(0, 10, 12), clock(0, 10, 14))
        .await;
    fixture
        .turn("s1", "app", clock(0, 10, 55), clock(0, 11, 5))
        .await;
    fixture
        .turn("s2", "app", clock(0, 10, 20), clock(0, 10, 30))
        .await;

    let seconds = turn_seconds(&fixture, "s1", clock(0, 10, 0), clock(0, 11, 0)).await;

    assert_eq!(seconds, (10 + 2 + 5) * MINUTE as u64);
}

#[tokio::test]
async fn the_turn_under_way_counts_until_the_end_of_the_span() {
    let fixture = Fixture::new().await;
    fixture.began("s1", "app", clock(0, 10, 0)).await;

    let seconds = turn_seconds(&fixture, "s1", clock(0, 9, 0), clock(0, 10, 25)).await;

    assert_eq!(seconds, 25 * MINUTE as u64);
}

#[tokio::test]
async fn an_empty_span_has_no_turn_seconds() {
    let fixture = Fixture::new().await;
    fixture
        .turn("s1", "app", clock(0, 10, 0), clock(0, 10, 20))
        .await;

    let seconds = turn_seconds(&fixture, "s1", clock(0, 10, 10), clock(0, 10, 10)).await;

    assert_eq!(seconds, 0);
}
