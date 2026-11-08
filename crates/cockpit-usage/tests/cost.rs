//! The cost archive: what a reply costs, how replies are counted once, and the totals per
//! account, project and day.
mod common;

use cockpit_provider_api::{ProviderName, ReplyScan, ReplyTokens, SourceCursor};
use cockpit_usage::{AccountName, Day, Error};
use common::{DAY, Fixture, MONDAY, app_project, reply, scan_of, tokens};

const OPUS: &str = "claude-opus-5-5";

fn all_kinds() -> ReplyTokens {
    ReplyTokens {
        input: 10,
        output: 20,
        cache_read: 300,
        cache_write_5m: 40,
        cache_write_1h: 5,
        is_fast: false,
    }
}

async fn only_cost_nanos(fixture: &Fixture) -> u64 {
    let costs = fixture.all_costs().await;
    assert_eq!(costs.len(), 1, "{costs:#?}");
    costs[0].cost_nanos
}

#[tokio::test]
async fn a_reply_costs_each_kind_of_token_at_its_models_rate() {
    let fixture = Fixture::new().await;
    fixture
        .archive(vec![reply("m1:r1", MONDAY, OPUS, all_kinds())])
        .await;

    // Opus 5.5: $4 input, $20 output, $0.20 cache read, $5 and $8 cache writes per million.
    let expected = 10 * 4_000 + 20 * 20_000 + 300 * 200 + 40 * 5_000 + 5 * 8_000;
    assert_eq!(only_cost_nanos(&fixture).await, expected);
}

#[tokio::test]
async fn fast_mode_doubles_a_replys_cost() {
    let fixture = Fixture::new().await;
    let fast = ReplyTokens {
        is_fast: true,
        ..all_kinds()
    };
    fixture
        .archive(vec![reply("m1:r1", MONDAY, OPUS, fast)])
        .await;

    let standard = 10 * 4_000 + 20 * 20_000 + 300 * 200 + 40 * 5_000 + 5 * 8_000;
    assert_eq!(only_cost_nanos(&fixture).await, 2 * standard);
}

#[tokio::test]
async fn a_dated_model_id_is_priced_as_its_family_and_never_as_a_shorter_one() {
    let fixture = Fixture::new().await;
    fixture
        .archive(vec![
            reply(
                "a:a",
                MONDAY,
                "claude-opus-5-5-20261101",
                tokens(1_000_000, 0),
            ),
            reply(
                "b:b",
                MONDAY,
                "claude-opus-5-5-2026-11-01",
                tokens(1_000_000, 0),
            ),
        ])
        .await;

    // Both are Opus 5.5 at $4 a million input tokens, not Opus 5 at $5.
    assert_eq!(only_cost_nanos(&fixture).await, 2 * 4_000_000_000);
}

#[tokio::test]
async fn a_long_prompt_to_haiku_5_5_is_billed_at_the_long_prompt_rates() {
    let fixture = Fixture::new().await;
    fixture
        .archive(vec![
            reply("a:a", MONDAY, "claude-haiku-5-5", tokens(100_000, 1_000)),
            reply("b:b", MONDAY, "claude-haiku-5-5", tokens(100_001, 1_000)),
        ])
        .await;

    let short = 100_000 * 100 + 1_000 * 500;
    let long = 100_001 * 500 + 1_000 * 2_500;
    assert_eq!(only_cost_nanos(&fixture).await, short + long);
}

#[tokio::test]
async fn a_model_without_a_price_is_archived_with_its_tokens_and_no_cost() {
    let fixture = Fixture::new().await;
    let recorded = fixture
        .archive(vec![
            reply("a:a", MONDAY, OPUS, tokens(1, 1)),
            reply("b:b", MONDAY, "claude-unheard-of-9", tokens(500, 700)),
        ])
        .await;

    let total = &fixture.all_costs().await[0];

    assert_eq!((recorded.added, recorded.unpriced), (2, 1));
    assert_eq!((total.replies, total.unpriced_replies), (2, 1));
    assert_eq!((total.input, total.output), (501, 701));
    assert_eq!(total.cost_nanos, 4_000 + 20_000);
}

#[tokio::test]
async fn replies_without_a_cost_are_priced_later_when_the_table_knows_their_model() {
    let fixture = Fixture::new().await;
    fixture
        .archive(vec![reply("a:a", MONDAY, OPUS, tokens(1_000, 0))])
        .await;
    let write = fixture.database.write().await.unwrap();
    write
        .execute("UPDATE usage_cost_replies SET cost_nanos = NULL", ())
        .await
        .unwrap();
    let priced = cockpit_usage::price_unpriced(&write).await.unwrap();
    let again = cockpit_usage::price_unpriced(&write).await.unwrap();
    write.commit().await.unwrap();

    assert_eq!((priced, again), (1, 0));
    assert_eq!(only_cost_nanos(&fixture).await, 4_000_000);
}

#[tokio::test]
async fn totals_are_kept_per_account_project_and_day() {
    let fixture = Fixture::new().await;
    fixture
        .archive_for(
            "work",
            "session-1",
            &scan_of(vec![
                reply("a:a", MONDAY, OPUS, tokens(1_000, 0)),
                reply("b:b", MONDAY + DAY, OPUS, tokens(1_000, 0)),
            ]),
        )
        .await
        .unwrap();
    fixture
        .archive_for(
            "private",
            "session-2",
            &scan_of(vec![reply("c:c", MONDAY, OPUS, tokens(2_000, 0))]),
        )
        .await
        .unwrap();
    fixture
        .archive_in_project(
            "work",
            "session-1",
            &scan_of(vec![reply("d:d", MONDAY, OPUS, tokens(3_000, 0))]),
            &|_| "tools".to_owned(),
        )
        .await
        .unwrap();

    let rows: Vec<(String, String, String, String, u64, u64)> = fixture
        .all_costs()
        .await
        .into_iter()
        .map(|row| {
            (
                row.day.to_string(),
                row.account.to_string(),
                row.project,
                row.provider.to_string(),
                row.replies,
                row.cost_nanos,
            )
        })
        .collect();
    let row = |day: &str, account: &str, project: &str, nanos: u64| {
        (
            day.to_owned(),
            account.to_owned(),
            project.to_owned(),
            "claude".to_owned(),
            1,
            nanos,
        )
    };
    assert_eq!(
        rows,
        [
            row("2026-11-02", "private", "app", 8_000_000),
            row("2026-11-02", "work", "app", 4_000_000),
            row("2026-11-02", "work", "tools", 12_000_000),
            row("2026-11-03", "work", "app", 4_000_000),
        ]
    );
}

#[tokio::test]
async fn a_day_is_a_utc_day_the_way_providers_bill() {
    let fixture = Fixture::new().await;
    fixture
        .archive(vec![
            reply("a:a", MONDAY - 1, OPUS, tokens(1, 0)),
            reply("b:b", MONDAY, OPUS, tokens(1, 0)),
            reply("c:c", MONDAY + DAY - 1, OPUS, tokens(1, 0)),
            reply("d:d", MONDAY + DAY, OPUS, tokens(1, 0)),
        ])
        .await;

    let days: Vec<(String, u64)> = fixture
        .all_costs()
        .await
        .into_iter()
        .map(|row| (row.day.to_string(), row.replies))
        .collect();

    assert_eq!(
        days,
        [
            ("2026-11-01".to_owned(), 1),
            ("2026-11-02".to_owned(), 2),
            ("2026-11-03".to_owned(), 1)
        ]
    );
}

#[tokio::test]
async fn the_range_of_days_includes_both_ends() {
    let fixture = Fixture::new().await;
    fixture
        .archive(
            (0..5)
                .map(|offset| {
                    reply(
                        &format!("{offset}:x"),
                        MONDAY + offset * DAY,
                        OPUS,
                        tokens(1, 0),
                    )
                })
                .collect(),
        )
        .await;

    let days: Vec<String> = fixture
        .costs_between("2026-11-03", "2026-11-05")
        .await
        .into_iter()
        .map(|row| row.day.to_string())
        .collect();

    assert_eq!(days, ["2026-11-03", "2026-11-04", "2026-11-05"]);
}

#[tokio::test]
async fn a_reply_seen_again_is_counted_once() {
    let fixture = Fixture::new().await;
    let replay = scan_of(vec![reply("m1:r1", MONDAY, OPUS, tokens(1_000, 0))]);
    fixture.archive_for("main", "owner", &replay).await.unwrap();

    let again = fixture.archive_for("main", "owner", &replay).await.unwrap();

    assert_eq!((again.added, again.raised), (0, 0));
    assert_eq!(only_cost_nanos(&fixture).await, 4_000_000);
}

#[tokio::test]
async fn a_replayed_reply_stays_with_the_account_that_archived_it_first() {
    let fixture = Fixture::new().await;
    let replay = scan_of(vec![reply("m1:r1", MONDAY, OPUS, tokens(1_000, 0))]);
    fixture
        .archive_for("first", "session-1", &replay)
        .await
        .unwrap();

    fixture
        .archive_for("second", "session-1", &replay)
        .await
        .unwrap();

    let costs = fixture.all_costs().await;
    assert_eq!(costs.len(), 1, "{costs:#?}");
    assert_eq!(costs[0].account.as_str(), "first");
}

#[tokio::test]
async fn a_message_written_in_pieces_is_the_fullest_piece_counted_once() {
    let fixture = Fixture::new().await;
    fixture
        .archive(vec![
            reply("m1:r1", MONDAY, OPUS, tokens(1_000, 5)),
            reply("m1:r1", MONDAY, OPUS, tokens(1_000, 400)),
            reply("m1:r1", MONDAY, OPUS, tokens(1_000, 90)),
        ])
        .await;

    let total = fixture.all_costs().await.remove(0);

    assert_eq!((total.replies, total.output), (1, 400));
}

#[tokio::test]
async fn a_later_scan_raises_an_archived_reply_to_its_fuller_counts_and_never_lowers_them() {
    let fixture = Fixture::new().await;
    fixture
        .archive(vec![reply("m1:r1", MONDAY, OPUS, tokens(1_000, 5))])
        .await;

    let raised = fixture
        .archive(vec![reply("m1:r1", MONDAY, OPUS, tokens(1_000, 400))])
        .await;
    let lowered = fixture
        .archive(vec![reply("m1:r1", MONDAY, OPUS, tokens(1_000, 90))])
        .await;

    assert_eq!((raised.raised, lowered.raised), (1, 0));
    assert_eq!(
        only_cost_nanos(&fixture).await,
        1_000 * 4_000 + 400 * 20_000
    );
}

#[tokio::test]
async fn a_reply_with_an_impossible_count_is_refused_and_the_rest_kept() {
    let fixture = Fixture::new().await;
    let huge = reply("a:a", MONDAY, OPUS, tokens(u64::MAX / 2, 0));
    let sound = reply("b:b", MONDAY, OPUS, tokens(1, 0));
    let scan = scan_of(vec![huge, sound.clone()]);
    let recorded = fixture.archive_for("main", "owner", &scan).await.unwrap();
    assert_eq!((recorded.added, recorded.rejected), (1, 1));
    assert_eq!(fixture.all_costs().await[0].replies, 1);
}

#[tokio::test]
async fn a_reply_whose_project_has_no_plain_name_is_archived_under_unknown() {
    let fixture = Fixture::new().await;
    let long_name = "x".repeat(300);

    for (key, name) in [
        ("a:a", String::new()),
        ("b:b", long_name),
        ("c:c", "bad\nname".to_owned()),
    ] {
        let recorded = fixture
            .archive_in_project(
                "main",
                "owner",
                &scan_of(vec![reply(key, MONDAY, OPUS, tokens(1, 0))]),
                &|_| name.clone(),
            )
            .await
            .unwrap();
        assert_eq!((recorded.added, recorded.rejected), (1, 0));
    }

    let costs = fixture.all_costs().await;
    assert_eq!(costs.len(), 1);
    assert_eq!(
        (costs[0].project.as_str(), costs[0].replies),
        ("unknown", 3)
    );
}

#[tokio::test]
async fn a_scan_moves_its_home_s_cursors_in_the_same_transaction_as_its_replies() {
    let fixture = Fixture::new().await;
    let scan = ReplyScan {
        replies: vec![reply("m1:r1", MONDAY, OPUS, tokens(1, 1))],
        cursors: vec![
            SourceCursor::new(".claude/projects/a/1.jsonl", 900).unwrap(),
            SourceCursor::new(".claude/projects/a/2.jsonl", 40).unwrap(),
        ],
        ..ReplyScan::default()
    };
    fixture
        .archive_for("main", "session-1", &scan)
        .await
        .unwrap();
    let fixture = fixture.reopened().await;

    let read = fixture.database.read().await.unwrap();
    let provider = ProviderName::new("claude").unwrap();
    let kept = cockpit_usage::cursors(&read, &provider, "session-1")
        .await
        .unwrap();
    let elsewhere = cockpit_usage::cursors(&read, &provider, "session-2")
        .await
        .unwrap();

    assert_eq!(kept, scan.cursors);
    assert!(elsewhere.is_empty());
}

#[tokio::test]
async fn a_later_scan_replaces_a_sources_cursor_and_a_forgotten_home_keeps_its_replies() {
    let fixture = Fixture::new().await;
    let cursor = |offset| ReplyScan {
        cursors: vec![SourceCursor::new("a.jsonl", offset).unwrap()],
        ..scan_of(vec![reply(
            &format!("{offset}:x"),
            MONDAY,
            OPUS,
            tokens(1, 1),
        )])
    };
    fixture
        .archive_for("main", "session-1", &cursor(10))
        .await
        .unwrap();
    fixture
        .archive_for("main", "session-1", &cursor(25))
        .await
        .unwrap();
    let provider = ProviderName::new("claude").unwrap();
    let moved = cockpit_usage::cursors(
        &fixture.database.read().await.unwrap(),
        &provider,
        "session-1",
    )
    .await
    .unwrap();
    let write = fixture.database.write().await.unwrap();
    cockpit_usage::forget_home(&write, &provider, "session-1")
        .await
        .unwrap();
    write.commit().await.unwrap();
    let forgotten = cockpit_usage::cursors(
        &fixture.database.read().await.unwrap(),
        &provider,
        "session-1",
    )
    .await
    .unwrap();

    assert_eq!(moved, [SourceCursor::new("a.jsonl", 25).unwrap()]);
    assert!(forgotten.is_empty());
    assert_eq!(fixture.all_costs().await[0].replies, 2);
}

#[tokio::test]
async fn an_empty_home_scope_is_refused() {
    let fixture = Fixture::new().await;

    let error = fixture
        .archive_for("main", "", &scan_of(vec![]))
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            Error::Invalid {
                field: "home scope",
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn a_day_reads_and_writes_as_a_utc_date() {
    assert_eq!(Day::containing(MONDAY).to_string(), "2026-11-02");
    assert_eq!(Day::containing(MONDAY - 1).to_string(), "2026-11-01");
    assert_eq!(
        Day::parse("2026-11-02").unwrap(),
        Day::containing(MONDAY + 3600)
    );
    assert_eq!(Day::parse("2026-11-02").unwrap().first_second(), MONDAY);
    assert_eq!(
        Day::parse("2026-03-01").unwrap().minus_days(1).to_string(),
        "2026-02-28"
    );
    assert_eq!(
        Day::parse("2028-03-01").unwrap().minus_days(1).to_string(),
        "2028-02-29"
    );
}

#[test]
fn text_that_is_not_a_real_date_is_not_a_day() {
    for text in [
        "2026-13-01",
        "2026-02-30",
        "2026-11-0",
        "yesterday",
        "26-11-02",
        "1999-12-31",
        "2026/11/02",
    ] {
        assert!(Day::parse(text).is_err(), "{text}");
    }
    let _ = (app_project(None), AccountName::new("main"));
}
