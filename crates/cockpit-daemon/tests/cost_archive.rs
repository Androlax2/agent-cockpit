//! Issue #699 through the daemon: the replies in the owner's home are archived once, at the
//! provider's API prices, and the owner reads them per UTC day with `usage-cost`. The fixture
//! is a synthetic week shaped like Claude Code's transcripts, with the bills worked out by
//! hand (nano-dollars: dollars per million tokens times 1000 per token); no test reads the
//! owner's own transcripts.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cockpit_claude::ClaudeCode;
use cockpit_client::{ClientError, Connection};
use cockpit_daemon::{CostScanning, Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{Command, CostRowView, ErrorCode, Reply};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{ModelReply, ProviderName, ReplyTokens};
use cockpit_usage::Day;
use common::GENEROUS;

const SCAN_INTERVAL: Duration = Duration::from_millis(100);
const DAY_SECONDS: i64 = 86_400;
const TRANSCRIPT: &str = ".claude/projects/-work-app/aaaa.jsonl";
const RESUMED_TRANSCRIPT: &str = ".claude/projects/-work-app/bbbb.jsonl";

fn options_in(dir: &Path) -> Options {
    Options {
        runtime_dir: dir.join("run/cockpit"),
        state_dir: dir.join("state/cockpit"),
        limits: Limits::default(),
    }
}

/// UTC midnight of the day five days before today: the fixture's first day, so the week is
/// inside `usage-cost --days 7` whenever the test runs.
fn first_day_start() -> i64 {
    let today_start = Day::containing(common::unix_now()).first_second();
    today_start - 5 * DAY_SECONDS
}

fn day_text(days_after_first: i64) -> String {
    Day::containing(first_day_start() + days_after_first * DAY_SECONDS).to_string()
}

/// `2026-11-02T10:00:00.000Z` for the second of the fixture week.
fn utc_stamp(days_after_first: i64, seconds_into_day: i64) -> String {
    format!(
        "{}T{:02}:{:02}:{:02}.000Z",
        day_text(days_after_first),
        seconds_into_day / 3600,
        seconds_into_day % 3600 / 60,
        seconds_into_day % 60
    )
}

fn reply_line(stamp: &str, message: &str, model: &str, usage: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{stamp}","cwd":"/work/app","sessionId":"aaaa","requestId":"req_{message}","message":{{"id":"msg_{message}","model":"{model}","role":"assistant","usage":{usage}}}}}"#
    )
}

/// Claude Code writes one line per content block of a reply, each with the whole reply's usage.
fn block_lines(line: &str) -> String {
    format!("{line}\n{line}\n")
}

fn append(home: &Path, relative: &str, text: &str) {
    let path = home.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

/// The week: priced replies on three days, a reply repeated by its content blocks and again by
/// a resumed conversation, a reply across the UTC midnight, a fast-mode reply, a stamp with a
/// time-zone offset, and a reply of a model with no price.
fn write_week(home: &Path) {
    let opus_5_5 = reply_line(
        &utc_stamp(0, 10 * 3600),
        "1",
        "claude-opus-5-5",
        r#"{"input_tokens":1000,"output_tokens":2000,"cache_read_input_tokens":50000,"cache_creation_input_tokens":10000,"cache_creation":{"ephemeral_5m_input_tokens":4000,"ephemeral_1h_input_tokens":6000}}"#,
    );
    let sonnet = reply_line(
        &utc_stamp(1, 86_399),
        "2",
        "claude-sonnet-5-5",
        r#"{"input_tokens":500,"output_tokens":1500}"#,
    );
    let fast_opus = reply_line(
        &utc_stamp(2, 1),
        "3",
        "claude-opus-4-6",
        r#"{"input_tokens":200,"output_tokens":400,"cache_read_input_tokens":1000,"cache_creation_input_tokens":100,"speed":"fast"}"#,
    );
    let unpriced = reply_line(
        &utc_stamp(2, 3600),
        "4",
        "claude-from-the-future-9",
        r#"{"input_tokens":10,"output_tokens":10}"#,
    );
    let offset_stamp = format!("{}T14:00:00.000+02:00", day_text(2));
    let haiku = reply_line(
        &offset_stamp,
        "5",
        "claude-haiku-4-5",
        r#"{"input_tokens":100,"output_tokens":100}"#,
    );
    for line in [&opus_5_5, &sonnet, &fast_opus, &unpriced, &haiku] {
        append(home, TRANSCRIPT, &block_lines(line));
    }
    append(home, RESUMED_TRANSCRIPT, &block_lines(&opus_5_5));
}

struct Archiving {
    dir: tempfile::TempDir,
    home: PathBuf,
}

impl Archiving {
    fn new() -> Archiving {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        Archiving { dir, home }
    }

    async fn try_start(
        &self,
        provider: Arc<dyn cockpit_provider_api::Provider>,
    ) -> Result<Daemon, String> {
        let mut wiring = common::wiring_in(self.dir.path(), vec![provider]);
        wiring.cost = Some(CostScanning {
            home: self.home.clone(),
            interval: SCAN_INTERVAL,
        });
        let environment = wiring.environment.clone();
        Instance::acquire(options_in(self.dir.path()))
            .map_err(|error| format!("{error:?}"))?
            .start(wiring, move |capabilities| {
                Arc::new(Routes::new(capabilities, environment))
            })
            .await
            .map_err(|error| format!("{error:?}"))
    }

    async fn start(&self, provider: Arc<dyn cockpit_provider_api::Provider>) -> Daemon {
        self.try_start(provider).await.unwrap()
    }

    fn claude() -> Arc<dyn cockpit_provider_api::Provider> {
        Arc::new(ClaudeCode::new("/usr/bin/claude".into(), None, None))
    }

    async fn start_claude(&self) -> Daemon {
        self.start(Archiving::claude()).await
    }

    /// The daemon again after the first was dropped: its tasks release the state database as
    /// the runtime next polls them, so the new start waits for that.
    async fn restart_claude(&self) -> Daemon {
        let deadline = tokio::time::Instant::now() + GENEROUS;
        loop {
            match self.try_start(Archiving::claude()).await {
                Ok(daemon) => return daemon,
                Err(error) => assert!(
                    tokio::time::Instant::now() < deadline,
                    "the daemon never restarted: {error}"
                ),
            }
            tokio::time::sleep(SCAN_INTERVAL).await;
        }
    }
}

async fn cost_rows(daemon: &Daemon, days: u32) -> Vec<CostRowView> {
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();
    match client
        .request(Command::UsageCost { days }, GENEROUS)
        .await
        .unwrap()
    {
        Reply::UsageCost { cost } => cost.rows,
        reply => panic!("{reply:?}"),
    }
}

/// The rows once `is_ready` holds of them; panics when they never do.
async fn rows_until(
    daemon: &Daemon,
    is_ready: impl Fn(&[CostRowView]) -> bool,
) -> Vec<CostRowView> {
    let deadline = tokio::time::Instant::now() + GENEROUS;
    loop {
        let rows = cost_rows(daemon, 7).await;
        if is_ready(&rows) {
            return rows;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the archive never got there: {rows:#?}"
        );
        tokio::time::sleep(SCAN_INTERVAL).await;
    }
}

fn row(
    day: i64,
    replies: u64,
    unpriced_replies: u64,
    tokens: [u64; 4],
    cost_nanos: u64,
) -> CostRowView {
    let [
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
    ] = tokens;
    CostRowView {
        day: day_text(day),
        provider: "claude".to_owned(),
        account: "default".to_owned(),
        project: "app".to_owned(),
        replies,
        unpriced_replies,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cost_nanos,
    }
}

/// What the week bills, worked out apart from the code:
/// day 0: 1000 x 4000 + 2000 x 20000 + 50000 x 200 + 4000 x 5000 + 6000 x 8000 = 122 000 000;
/// day 1: 500 x 2000 + 1500 x 10000 = 16 000 000 (it ends one second before the UTC midnight);
/// day 2: fast, twice (200 x 5000 + 400 x 25000 + 1000 x 500 + 100 x 6250) = 24 250 000, plus
/// 100 x 1000 + 100 x 5000 = 600 000 for the reply stamped 14:00 at +02:00, plus a reply with
/// no price that counts its tokens and no cost.
fn billed_week() -> Vec<CostRowView> {
    vec![
        row(0, 1, 0, [1000, 2000, 50_000, 10_000], 122_000_000),
        row(1, 1, 0, [500, 1500, 0, 0], 16_000_000),
        row(2, 3, 1, [310, 510, 1000, 100], 24_850_000),
    ]
}

#[tokio::test]
async fn a_week_of_transcripts_is_archived_to_the_cent_by_utc_day() {
    let archiving = Archiving::new();
    write_week(&archiving.home);
    let daemon = archiving.start_claude().await;

    let rows = rows_until(&daemon, |rows| rows.len() == 3).await;

    assert_eq!(rows, billed_week());
}

#[tokio::test]
async fn replies_added_later_are_archived_without_counting_the_rest_again() {
    let archiving = Archiving::new();
    write_week(&archiving.home);
    let daemon = archiving.start_claude().await;
    rows_until(&daemon, |rows| rows.len() == 3).await;
    let later = reply_line(
        &utc_stamp(3, 3600),
        "6",
        "claude-sonnet-5-5",
        r#"{"input_tokens":1,"output_tokens":1}"#,
    );

    append(&archiving.home, TRANSCRIPT, &block_lines(&later));
    let rows = rows_until(&daemon, |rows| rows.len() == 4).await;

    let mut expected = billed_week();
    expected.push(row(3, 1, 0, [1, 1, 0, 0], 12_000));
    assert_eq!(rows, expected);
}

#[tokio::test]
async fn a_restart_does_not_count_the_archived_replies_again() {
    let archiving = Archiving::new();
    write_week(&archiving.home);
    let first = archiving.start_claude().await;
    rows_until(&first, |rows| rows.len() == 3).await;
    drop(first);

    let second = archiving.restart_claude().await;
    tokio::time::sleep(SCAN_INTERVAL * 5).await;

    assert_eq!(cost_rows(&second, 7).await, billed_week());
}

#[tokio::test]
async fn a_home_is_read_on_from_where_the_last_scan_ended() {
    let archiving = Archiving::new();
    let log = archiving.dir.path().join("reply-scans");
    let tokens = ReplyTokens {
        input: 1,
        output: 1,
        ..ReplyTokens::default()
    };
    let replies = (1..=2)
        .map(|number| {
            ModelReply::new(
                format!("reply-{number}"),
                first_day_start() + 3600,
                "claude-opus-5-5".to_owned(),
                Some("/work/app".to_owned()),
                tokens,
            )
            .unwrap()
        })
        .collect();
    let provider = FakeProvider::new(
        ProviderName::new("fake").unwrap(),
        archiving.dir.path().join("launches"),
    )
    .reading_replies(vec![(archiving.home.clone(), replies)], log.clone());
    let daemon = archiving.start(Arc::new(provider)).await;

    rows_until(&daemon, |rows| rows.iter().any(|row| row.replies == 2)).await;
    let scans = loop {
        let scans = FakeProvider::reply_scans(&log);
        if scans.len() >= 3 {
            break scans;
        }
        tokio::time::sleep(SCAN_INTERVAL).await;
    };

    let offsets: Vec<u64> = scans.iter().map(|(_, offset)| *offset).collect();
    assert_eq!(offsets[0], 0);
    assert!(
        offsets[1..].iter().all(|offset| *offset == 2),
        "{offsets:?}"
    );
}

#[tokio::test]
async fn a_day_count_outside_the_archive_range_is_refused() {
    let archiving = Archiving::new();
    let daemon = archiving.start_claude().await;
    let client = Connection::open(daemon.socket_path(), GENEROUS)
        .await
        .unwrap();

    for days in [0, 367] {
        let outcome = client.request(Command::UsageCost { days }, GENEROUS).await;
        assert!(
            matches!(&outcome, Err(ClientError::Daemon(error)) if error.code == ErrorCode::InvalidArgument),
            "{days}: {outcome:?}"
        );
    }
}
