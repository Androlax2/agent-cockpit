//! #158: a quota-stopped session resumes once its plan window refills, with one continue
//! message, judged by the daemon's own plan-window readings; at the real daemon socket, with
//! a fake provider and isolated tmux. A reading is recorded as the quota worker records it.
#![cfg(target_os = "linux")]
mod candidate;
mod common;
mod messaging_fixture;

use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_daemon::{Daemon, QuotaPolling};
use cockpit_protocol::{Command, QuotaStopStatusView, QuotaStopView, Reply, SessionView};
use cockpit_provider_api::fake::{FakeLaunch, FakeProvider};
use cockpit_provider_api::{
    LimitKey, ProviderError, ProviderName, QuotaReading, QuotaWindow, TurnOutcome,
};
use cockpit_usage::{AccountName, QuotaPolicy};
use common::{GENEROUS, unix_now};
use messaging_fixture::Home;

/// What the daemon sends a resumed session, word for word.
const CONTINUE: &str = "Torlek: the plan usage limit that stopped your last turn has reset. \
     Continue where you stopped.";
const FIVE_HOURS: u32 = 5 * 60 * 60;

/// A daemon whose quota-stop passes run every 100 ms, judging readings under the default
/// policy. Its `fake` provider reads plan windows, but its one reading at start fails, so only
/// the readings the test records count.
async fn start(home: &Home) -> Daemon {
    let provider = fake(home).reading_quota(
        vec![Err(ProviderError {
            reason: "the test records the readings".to_owned(),
        })],
        home.path().join("quota-asks"),
    );
    start_with(home, provider).await
}

/// A daemon as [`start`] makes, whose `fake` provider reads no plan windows at all.
async fn start_unread(home: &Home) -> Daemon {
    start_with(home, fake(home)).await
}

/// A `fake` provider whose agent's messages land in `messages`, and that gives `rate_limit`
/// both for a spent plan and for a transient limit, as Claude Code does.
fn fake(home: &Home) -> FakeProvider {
    FakeProvider::new(
        ProviderName::new("fake").unwrap(),
        home.path().join("launches"),
    )
    .reading_messages(home.path().join("messages"))
    .limiting_on(&["rate_limit"])
}

async fn start_with(home: &Home, provider: FakeProvider) -> Daemon {
    let mut wiring = common::wiring_in(home.path(), vec![Arc::new(provider)]);
    wiring.session_tick = Duration::from_millis(100);
    wiring.quota = Some(QuotaPolling {
        home: home.path().join("home"),
        refresh: Duration::from_secs(3600),
        policy: QuotaPolicy::default(),
        project_shares: std::collections::BTreeMap::new(),
    });
    home.start_wired(wiring).await
}

fn launches(home: &Home) -> Vec<FakeLaunch> {
    FakeProvider::launches(&home.path().join("launches"))
}

/// Reports `report` as the agent of `launch`, through the agent channel's own record,
/// counting the `rate_limit` failures [`fake`] gives as the daemon does.
async fn report_as(home: &Home, launch: &FakeLaunch, report: cockpit_sessions::Report) {
    let database = home.database.lock().unwrap().clone().unwrap();
    let write = database.write().await.unwrap();
    let providers: [Arc<dyn cockpit_provider_api::Provider>; 1] = [Arc::new(fake(home))];
    cockpit_sessions::report_counting_limits(
        &write,
        &cockpit_agent_channel::ancestry(launch.pid as u32),
        Some(&launch.token),
        report,
        &cockpit_sessions::TransientLimits::of(&providers),
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
}

fn waiting() -> cockpit_sessions::Report {
    cockpit_sessions::Report {
        state: Some(cockpit_sessions::AgentState::Waiting),
        ..Default::default()
    }
}

fn quota_stopped() -> cockpit_sessions::Report {
    cockpit_sessions::Report {
        state: Some(cockpit_sessions::AgentState::Waiting),
        outcome: Some(TurnOutcome::QuotaStopped),
        ..Default::default()
    }
}

/// Records a reading of the owner's own sign-in, taken now: its five-hour window used
/// `used_percent`, starting over at `resets_at`. Waits past the second first, so the reading
/// is strictly newer than whatever came before it.
async fn record_reading(home: &Home, used_percent: f64, resets_at: i64) {
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let window = QuotaWindow::new(
        LimitKey::new("five-hour").unwrap(),
        None,
        Some(FIVE_HOURS),
        used_percent,
        Some(resets_at),
    )
    .unwrap();
    let reading = QuotaReading::new(Some("max".to_owned()), vec![window], false).unwrap();
    let database = home.database.lock().unwrap().clone().unwrap();
    let write = database.write().await.unwrap();
    cockpit_usage::record_reading(
        &write,
        &ProviderName::new("fake").unwrap(),
        &AccountName::new("default").unwrap(),
        &reading,
        &QuotaPolicy::default(),
        unix_now(),
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
}

async fn show(client: &Connection, session: &str) -> SessionView {
    let reply = client
        .request(Command::SessionList { ended: true }, GENEROUS)
        .await
        .unwrap();
    let Reply::Sessions { sessions } = reply else {
        panic!("{reply:?}")
    };
    sessions
        .into_iter()
        .find(|listed| listed.id == session)
        .unwrap()
}

/// The session's quota stop once `check` holds for it.
async fn quota_stop_once(
    client: &Connection,
    session: &str,
    check: impl Fn(&QuotaStopView) -> bool,
) -> QuotaStopView {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let stop = show(client, session).await.quota_stop;
        if let Some(stop) = stop.as_ref().filter(|stop| check(stop)) {
            return (**stop).clone();
        }
        assert!(Instant::now() < deadline, "{session}: {stop:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn messages(home: &Home) -> Vec<(String, String)> {
    FakeProvider::messages(&home.path().join("messages"))
}

/// A running session of `fake`, its agent waiting at its prompt, then quota-stopped.
async fn quota_stopped_session(home: &Home, client: &Connection, index: usize) -> SessionView {
    let root = home.path().join("project");
    let session = home.session(client, &root, "fake").await;
    let launch = common::once_logged(index + 1, || launches(home)).await[index].clone();
    assert_eq!(launch.session, session.id);
    report_as(home, &launch, waiting()).await;
    report_as(home, &launch, quota_stopped()).await;
    session
}

#[tokio::test]
async fn a_quota_stopped_session_is_sent_one_continue_once_its_plan_window_refills() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let session = quota_stopped_session(&home, &client, 0).await;
    let resets_at = unix_now() + 3600;

    record_reading(&home, 100.0, resets_at).await;
    let stop = quota_stop_once(&client, &session.id, |stop| stop.resets_at.is_some()).await;
    assert_eq!(stop.resets_at, Some(resets_at));
    assert_eq!(stop.status, QuotaStopStatusView::Waiting);
    common::still_logged(0, || messages(&home)).await;

    record_reading(&home, 3.0, unix_now() + i64::from(FIVE_HOURS)).await;
    assert_eq!(
        common::still_logged(1, || messages(&home)).await,
        vec![(session.id.clone(), CONTINUE.to_owned())]
    );
    let stop = show(&client, &session.id).await.quota_stop.unwrap();
    assert!(
        matches!(stop.status, QuotaStopStatusView::Resumed { .. }),
        "{stop:?}"
    );
}

#[tokio::test]
async fn a_reading_taken_before_the_stop_neither_confirms_nor_resumes_it() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let root = home.path().join("project");
    let session = home.session(&client, &root, "fake").await;
    let launch = common::once_logged(1, || launches(&home)).await[0].clone();
    report_as(&home, &launch, waiting()).await;

    record_reading(&home, 3.0, unix_now() + 3600).await;
    report_as(&home, &launch, quota_stopped()).await;

    common::still_logged(0, || messages(&home)).await;
    let stop = show(&client, &session.id).await.quota_stop.unwrap();
    assert_eq!(stop.status, QuotaStopStatusView::Waiting);
    assert_eq!(stop.resets_at, None);
}

#[tokio::test]
async fn a_spent_reading_just_before_the_stop_confirms_it_so_the_next_room_resumes_it() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let root = home.path().join("project");
    let session = home.session(&client, &root, "fake").await;
    let launch = common::once_logged(1, || launches(&home)).await[0].clone();
    report_as(&home, &launch, waiting()).await;
    let resets_at = unix_now() + 60;
    record_reading(&home, 100.0, resets_at).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    report_as(&home, &launch, quota_stopped()).await;
    let stop = quota_stop_once(&client, &session.id, |stop| stop.resets_at.is_some()).await;
    assert_eq!(stop.resets_at, Some(resets_at));

    record_reading(&home, 2.0, unix_now() + i64::from(FIVE_HOURS)).await;

    assert_eq!(
        common::once_logged(1, || messages(&home)).await,
        vec![(session.id.clone(), CONTINUE.to_owned())]
    );
}

#[tokio::test]
async fn a_quota_stop_whose_windows_show_room_is_handed_to_the_owner() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let session = quota_stopped_session(&home, &client, 0).await;

    record_reading(&home, 40.0, unix_now() + 3600).await;
    let stop = quota_stop_once(&client, &session.id, |stop| {
        matches!(stop.status, QuotaStopStatusView::Dropped { .. })
    })
    .await;
    let QuotaStopStatusView::Dropped { reason, .. } = stop.status else {
        unreachable!()
    };
    assert!(reason.contains("plan windows show room"), "{reason}");
    let reports = agent_reports(&client).await;
    let report = reports
        .iter()
        .find(|report| report.session == session.id)
        .unwrap();
    assert_eq!(report.needs_input.as_deref(), Some(reason.as_str()));
    common::still_logged(0, || messages(&home)).await;
}

#[tokio::test]
async fn a_provider_whose_windows_are_never_read_hands_the_stop_to_the_owner() {
    let home = Home::new();
    let _daemon = start_unread(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let session = quota_stopped_session(&home, &client, 0).await;

    let stop = quota_stop_once(&client, &session.id, |stop| {
        matches!(stop.status, QuotaStopStatusView::Dropped { .. })
    })
    .await;
    let QuotaStopStatusView::Dropped { reason, .. } = stop.status else {
        unreachable!()
    };
    assert!(
        reason.contains("account default, whose plan windows Torlek does not read"),
        "{reason}"
    );
    common::still_logged(0, || messages(&home)).await;
}

#[tokio::test]
async fn an_agent_that_works_again_before_the_reset_is_not_resumed() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let session = quota_stopped_session(&home, &client, 0).await;
    record_reading(&home, 100.0, unix_now() + 3600).await;
    quota_stop_once(&client, &session.id, |stop| stop.resets_at.is_some()).await;

    let launch = launches(&home)[0].clone();
    report_as(
        &home,
        &launch,
        cockpit_sessions::Report {
            state: Some(cockpit_sessions::AgentState::Working),
            ..Default::default()
        },
    )
    .await;
    record_reading(&home, 3.0, unix_now() + i64::from(FIVE_HOURS)).await;

    let stop = quota_stop_once(&client, &session.id, |stop| {
        matches!(stop.status, QuotaStopStatusView::Dropped { .. })
    })
    .await;
    assert!(
        matches!(&stop.status, QuotaStopStatusView::Dropped { reason, .. } if reason.contains("worked again")),
        "{stop:?}"
    );
    common::still_logged(0, || messages(&home)).await;
}

#[tokio::test]
async fn a_session_asleep_at_the_reset_is_woken_then_sent_its_continue() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let session = quota_stopped_session(&home, &client, 0).await;
    record_reading(&home, 100.0, unix_now() + 3600).await;
    quota_stop_once(&client, &session.id, |stop| stop.resets_at.is_some()).await;
    client
        .request(
            Command::SessionSleep {
                session: session.id.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();

    record_reading(&home, 3.0, unix_now() + i64::from(FIVE_HOURS)).await;
    let woken = common::once_logged(2, || launches(&home)).await[1].clone();
    assert_eq!(woken.session, session.id);
    assert_eq!(woken.mode, "resume");
    common::still_logged(0, || messages(&home)).await;

    report_as(&home, &woken, waiting()).await;
    assert_eq!(
        common::still_logged(1, || messages(&home)).await,
        vec![(session.id.clone(), CONTINUE.to_owned())]
    );
    assert_eq!(
        latest_launching_reason(&home, &session.id).await.as_deref(),
        Some("woken at its plan's reset")
    );
}

/// Why the session last launched, from its transition history.
async fn latest_launching_reason(home: &Home, session: &str) -> Option<String> {
    let database = home.database.lock().unwrap().clone().unwrap();
    let read = database.read().await.unwrap();
    let mut rows = read
        .query(
            "SELECT reason FROM session_transitions
             WHERE session_id = ?1 AND to_state = 'launching' ORDER BY id DESC LIMIT 1",
            [session],
        )
        .await
        .unwrap();
    rows.next().await.unwrap().unwrap().get(0).unwrap()
}

#[tokio::test]
async fn the_coordinator_is_resumed_before_the_workers() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let worker = quota_stopped_session(&home, &client, 0).await;
    let coordinator = quota_stopped_session(&home, &client, 1).await;
    let granted = client
        .request(
            Command::SessionGrant {
                session: coordinator.id.clone(),
                permission: "message-sessions".to_owned(),
                all_projects: true,
                without_confirmation: false,
                per_hour: None,
                max_active: None,
            },
            GENEROUS,
        )
        .await
        .unwrap();
    assert!(
        matches!(granted, Reply::CoordinatorGrants { .. }),
        "{granted:?}"
    );
    record_reading(&home, 100.0, unix_now() + 3600).await;
    for session in [&worker.id, &coordinator.id] {
        quota_stop_once(&client, session, |stop| stop.resets_at.is_some()).await;
    }

    record_reading(&home, 3.0, unix_now() + i64::from(FIVE_HOURS)).await;

    assert_eq!(
        common::still_logged(2, || messages(&home)).await,
        vec![
            (coordinator.id.clone(), CONTINUE.to_owned()),
            (worker.id.clone(), CONTINUE.to_owned())
        ]
    );
}

fn failed(reason: &str) -> cockpit_sessions::Report {
    cockpit_sessions::Report {
        state: Some(cockpit_sessions::AgentState::Waiting),
        outcome: Some(TurnOutcome::Failed {
            reason: Some(reason.to_owned()),
        }),
        ..Default::default()
    }
}

fn completed() -> cockpit_sessions::Report {
    cockpit_sessions::Report {
        state: Some(cockpit_sessions::AgentState::Waiting),
        outcome: Some(TurnOutcome::Completed),
        ..Default::default()
    }
}

/// A session whose agent waits at its prompt, and its launch.
async fn waiting_session(home: &Home, client: &Connection) -> (SessionView, FakeLaunch) {
    let root = home.path().join("project");
    let session = home.session(client, &root, "fake").await;
    let launch = common::once_logged(1, || launches(home)).await[0].clone();
    report_as(home, &launch, waiting()).await;
    (session, launch)
}

#[tokio::test]
async fn three_rate_limit_failures_within_half_an_hour_are_recorded_as_a_quota_stop() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let (session, launch) = waiting_session(&home, &client).await;
    for _ in 0..2 {
        report_as(&home, &launch, failed("rate_limit")).await;
    }
    assert_eq!(show(&client, &session.id).await.quota_stop, None);

    report_as(&home, &launch, failed("rate_limit")).await;

    let stop = show(&client, &session.id).await.quota_stop.unwrap();
    assert_eq!(
        stop.inferred_from.as_deref(),
        Some("3 `rate_limit` failed turns within 30 minutes")
    );
    let reports = agent_reports(&client).await;
    let report = reports
        .iter()
        .find(|report| report.session == session.id)
        .unwrap();
    assert_eq!(
        report.outcome,
        Some(cockpit_protocol::TurnOutcome::QuotaStopped)
    );
}

#[tokio::test]
async fn a_completed_turn_between_rate_limit_failures_starts_the_count_over() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let (session, launch) = waiting_session(&home, &client).await;

    for report in [
        failed("rate_limit"),
        failed("rate_limit"),
        completed(),
        failed("rate_limit"),
        failed("rate_limit"),
    ] {
        report_as(&home, &launch, report).await;
    }

    assert_eq!(show(&client, &session.id).await.quota_stop, None);
}

#[tokio::test]
async fn a_failure_the_provider_does_not_give_for_limits_is_never_counted() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let (session, launch) = waiting_session(&home, &client).await;

    for _ in 0..3 {
        report_as(&home, &launch, failed("overloaded")).await;
    }

    assert_eq!(show(&client, &session.id).await.quota_stop, None);
}

#[tokio::test]
async fn rate_limit_failures_older_than_half_an_hour_do_not_count() {
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let (session, launch) = waiting_session(&home, &client).await;
    report_as(&home, &launch, failed("rate_limit")).await;
    report_as(&home, &launch, failed("rate_limit")).await;
    let database = home.database.lock().unwrap().clone().unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "UPDATE session_limit_failures SET failed_at = failed_at - 1801",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    report_as(&home, &launch, failed("rate_limit")).await;

    assert_eq!(show(&client, &session.id).await.quota_stop, None);
}

#[tokio::test]
async fn an_inferred_quota_stop_whose_windows_show_room_tells_the_owner_what_it_was_inferred_from()
{
    let home = Home::new();
    let _daemon = start(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let (session, launch) = waiting_session(&home, &client).await;
    for _ in 0..3 {
        report_as(&home, &launch, failed("rate_limit")).await;
    }

    record_reading(&home, 40.0, unix_now() + 3600).await;
    let stop = quota_stop_once(&client, &session.id, |stop| {
        matches!(stop.status, QuotaStopStatusView::Dropped { .. })
    })
    .await;

    let QuotaStopStatusView::Dropped { reason, .. } = stop.status else {
        unreachable!()
    };
    assert!(
        reason.starts_with(
            "quota stop inferred from 3 `rate_limit` failed turns within 30 minutes, but its \
             plan windows show room"
        ),
        "{reason}"
    );
}

async fn agent_reports(client: &Connection) -> Vec<cockpit_protocol::SessionReport> {
    let reply = client
        .request(Command::AgentReports, GENEROUS)
        .await
        .unwrap();
    let Reply::AgentReports { reports } = reply else {
        panic!("{reply:?}")
    };
    reports
}
