//! #718: a session whose turn the network ended is sent one continue message once a plan-window
//! reading of its provider succeeds again, through #158's resume pass; at the real daemon
//! socket, with a fake provider and isolated tmux. A reading is recorded as the quota worker
//! records it.
#![cfg(target_os = "linux")]
mod candidate;
mod common;
mod messaging_fixture;

use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_daemon::{Daemon, QuotaPolling};
use cockpit_protocol::{
    Command, QuotaStopCauseView, QuotaStopStatusView, QuotaStopView, Reply, SessionView,
};
use cockpit_provider_api::fake::{FakeLaunch, FakeProvider};
use cockpit_provider_api::{
    LimitKey, ProviderError, ProviderName, QuotaReading, QuotaWindow, TurnOutcome,
};
use cockpit_usage::{AccountName, QuotaPolicy};
use common::{GENEROUS, unix_now};
use messaging_fixture::Home;

/// What the daemon sends a session the network stopped, word for word.
const CONTINUE: &str = "Torlek: the network connection that ended your last turn is back. \
     Continue where you stopped.";

fn failing_reading() -> Result<QuotaReading, ProviderError> {
    Err(ProviderError {
        reason: "the usage endpoint is unreachable".to_owned(),
    })
}

/// A daemon whose resume passes run every 100 ms. Its `fake` provider reads plan windows every
/// `refresh`, and every reading fails, so only the readings the test records succeed.
async fn start(home: &Home, refresh: Duration) -> Daemon {
    let provider = fake(home).reading_quota(vec![failing_reading()], quota_asks(home));
    start_with(home, provider, refresh).await
}

/// A daemon as [`start`] makes, whose `fake` provider reads no plan windows at all.
async fn start_unread(home: &Home) -> Daemon {
    start_with(home, fake(home), Duration::from_secs(3600)).await
}

fn fake(home: &Home) -> FakeProvider {
    FakeProvider::new(
        ProviderName::new("fake").unwrap(),
        home.path().join("launches"),
    )
    .reading_messages(home.path().join("messages"))
}

fn quota_asks(home: &Home) -> std::path::PathBuf {
    home.path().join("quota-asks")
}

async fn start_with(home: &Home, provider: FakeProvider, refresh: Duration) -> Daemon {
    let mut wiring = common::wiring_in(home.path(), vec![Arc::new(provider)]);
    wiring.session_tick = Duration::from_secs(3600);
    wiring.quota_resume_tick = Duration::from_millis(100);
    wiring.quota = Some(QuotaPolling {
        home: home.path().join("home"),
        refresh,
        policy: QuotaPolicy::default(),
        project_shares: std::collections::BTreeMap::new(),
    });
    home.start_wired(wiring).await
}

fn launches(home: &Home) -> Vec<FakeLaunch> {
    FakeProvider::launches(&home.path().join("launches"))
}

/// Reports `report` as the agent of `launch`, through the agent channel's own record.
async fn report_as(home: &Home, launch: &FakeLaunch, report: cockpit_sessions::Report) {
    let database = home.database.lock().unwrap().clone().unwrap();
    let write = database.write().await.unwrap();
    cockpit_sessions::report(
        &write,
        &cockpit_agent_channel::ancestry(launch.pid as u32),
        Some(&launch.token),
        report,
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
}

fn ended(outcome: TurnOutcome) -> cockpit_sessions::Report {
    cockpit_sessions::Report {
        state: Some(cockpit_sessions::AgentState::Waiting),
        outcome: Some(outcome),
        ..Default::default()
    }
}

fn waiting() -> cockpit_sessions::Report {
    cockpit_sessions::Report {
        state: Some(cockpit_sessions::AgentState::Waiting),
        ..Default::default()
    }
}

fn network_lost() -> cockpit_sessions::Report {
    ended(TurnOutcome::NetworkLost {
        reason: "connection_failed".to_owned(),
    })
}

/// Records a successful reading of the owner's own sign-in, taken now. Waits past the second
/// first, so the reading is strictly newer than whatever came before it.
async fn record_reading(home: &Home) {
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let window = QuotaWindow::new(
        LimitKey::new("five-hour").unwrap(),
        None,
        Some(5 * 60 * 60),
        3.0,
        Some(unix_now() + 3600),
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

/// The session's stop once `check` holds for it.
async fn stop_once(
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

/// A running session of `fake` whose agent waits at its prompt, and its launch.
async fn waiting_session(
    home: &Home,
    client: &Connection,
    index: usize,
) -> (SessionView, FakeLaunch) {
    let root = home.path().join("project");
    let session = home.session(client, &root, "fake").await;
    let launch = common::once_logged(index + 1, || launches(home)).await[index].clone();
    assert_eq!(launch.session, session.id);
    report_as(home, &launch, waiting()).await;
    (session, launch)
}

/// A running session of `fake` whose last turn the network ended.
async fn network_stopped_session(home: &Home, client: &Connection, index: usize) -> SessionView {
    let (session, launch) = waiting_session(home, client, index).await;
    report_as(home, &launch, network_lost()).await;
    session
}

#[tokio::test]
async fn a_turn_the_network_ended_is_sent_one_continue_once_a_reading_reaches_its_provider() {
    let home = Home::new();
    let _daemon = start(&home, Duration::from_secs(3600)).await;
    let client = home.connect().await;
    home.project(&client).await;
    let session = network_stopped_session(&home, &client, 0).await;
    let stop = stop_once(&client, &session.id, |_| true).await;
    assert_eq!(stop.cause, QuotaStopCauseView::Network);
    assert_eq!(stop.status, QuotaStopStatusView::Waiting);
    common::still_logged(0, || messages(&home)).await;

    record_reading(&home).await;
    assert_eq!(
        common::once_logged(1, || messages(&home)).await,
        vec![(session.id.clone(), CONTINUE.to_owned())]
    );
    record_reading(&home).await;
    common::still_logged(1, || messages(&home)).await;
    let stop = show(&client, &session.id).await.quota_stop.unwrap();
    assert!(
        matches!(stop.status, QuotaStopStatusView::Resumed { .. }),
        "{stop:?}"
    );
}

#[tokio::test]
async fn a_reading_taken_before_the_network_ended_the_turn_does_not_resume_it() {
    let home = Home::new();
    let _daemon = start(&home, Duration::from_secs(3600)).await;
    let client = home.connect().await;
    home.project(&client).await;
    let (session, launch) = waiting_session(&home, &client, 0).await;
    record_reading(&home).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;

    report_as(&home, &launch, network_lost()).await;

    common::still_logged(0, || messages(&home)).await;
    let stop = show(&client, &session.id).await.quota_stop.unwrap();
    assert_eq!(stop.status, QuotaStopStatusView::Waiting);
}

#[tokio::test]
async fn a_failed_turn_is_never_resumed() {
    let home = Home::new();
    let _daemon = start(&home, Duration::from_secs(3600)).await;
    let client = home.connect().await;
    home.project(&client).await;
    let (session, launch) = waiting_session(&home, &client, 0).await;

    report_as(
        &home,
        &launch,
        ended(TurnOutcome::Failed {
            reason: Some("server_error".to_owned()),
        }),
    )
    .await;
    record_reading(&home).await;

    common::still_logged(0, || messages(&home)).await;
    assert_eq!(show(&client, &session.id).await.quota_stop, None);
}

#[tokio::test]
async fn the_coordinator_is_resumed_before_the_workers() {
    let home = Home::new();
    let _daemon = start(&home, Duration::from_secs(3600)).await;
    let client = home.connect().await;
    home.project(&client).await;
    let worker = network_stopped_session(&home, &client, 0).await;
    let coordinator = network_stopped_session(&home, &client, 1).await;
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

    record_reading(&home).await;

    assert_eq!(
        common::still_logged(2, || messages(&home)).await,
        vec![
            (coordinator.id.clone(), CONTINUE.to_owned()),
            (worker.id.clone(), CONTINUE.to_owned())
        ]
    );
}

#[tokio::test]
async fn a_provider_whose_windows_are_never_read_hands_the_session_to_the_owner() {
    let home = Home::new();
    let _daemon = start_unread(&home).await;
    let client = home.connect().await;
    home.project(&client).await;
    let session = network_stopped_session(&home, &client, 0).await;

    let stop = stop_once(&client, &session.id, |stop| {
        matches!(stop.status, QuotaStopStatusView::Dropped { .. })
    })
    .await;
    let QuotaStopStatusView::Dropped { reason, .. } = stop.status else {
        unreachable!()
    };
    assert!(
        reason.contains("cannot tell when the network is back"),
        "{reason}"
    );
    common::still_logged(0, || messages(&home)).await;
}

#[tokio::test]
async fn a_session_asleep_when_the_network_is_back_is_woken_then_sent_its_continue() {
    let home = Home::new();
    let _daemon = start(&home, Duration::from_secs(3600)).await;
    let client = home.connect().await;
    home.project(&client).await;
    let session = network_stopped_session(&home, &client, 0).await;
    client
        .request(
            Command::SessionSleep {
                session: session.id.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();

    record_reading(&home).await;
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
        Some("woken once its network was back")
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

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_reading_is_retried_at_each_refresh_while_the_network_stopped_a_session() {
    let refresh = Duration::from_millis(500);
    let home = Home::new();
    let _daemon = start(&home, refresh).await;
    let client = home.connect().await;
    home.project(&client).await;
    network_stopped_session(&home, &client, 0).await;
    let asked_before = FakeProvider::quota_asks(&quota_asks(&home)).len();

    let asks = common::once_logged(asked_before + 3, || {
        FakeProvider::quota_asks(&quota_asks(&home))
    })
    .await;

    let waited: Vec<u128> = asks[asked_before..]
        .windows(2)
        .map(|pair| pair[1].0 - pair[0].0)
        .collect();
    assert!(
        waited.iter().all(|wait| *wait < 2 * refresh.as_millis()),
        "{waited:?}"
    );
}

fn millis_now() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_the_network_ended_during_a_long_backoff_is_read_for_within_a_refresh() {
    let refresh = Duration::from_millis(500);
    let home = Home::new();
    let _daemon = start(&home, refresh).await;
    let client = home.connect().await;
    home.project(&client).await;
    let (_, launch) = waiting_session(&home, &client, 0).await;
    // After three failed readings the next one waits four refreshes.
    let asked = common::once_logged(3, || FakeProvider::quota_asks(&quota_asks(&home)))
        .await
        .len();

    report_as(&home, &launch, network_lost()).await;
    let stopped_at = millis_now();

    let asks =
        common::once_logged(asked + 1, || FakeProvider::quota_asks(&quota_asks(&home))).await;
    let waited = asks[asked].0.saturating_sub(stopped_at);
    assert!(waited < 2 * refresh.as_millis(), "{waited} ms");
}

#[tokio::test]
async fn a_turn_the_network_ends_again_soon_after_its_resume_goes_to_the_owner() {
    let home = Home::new();
    let _daemon = start(&home, Duration::from_secs(3600)).await;
    let client = home.connect().await;
    home.project(&client).await;
    let (session, launch) = waiting_session(&home, &client, 0).await;
    report_as(&home, &launch, network_lost()).await;
    record_reading(&home).await;
    common::once_logged(1, || messages(&home)).await;

    report_as(&home, &launch, network_lost()).await;

    let stop = stop_once(&client, &session.id, |stop| {
        matches!(stop.status, QuotaStopStatusView::Dropped { .. })
    })
    .await;
    let QuotaStopStatusView::Dropped { reason, .. } = stop.status else {
        unreachable!()
    };
    assert!(reason.contains("again soon after"), "{reason}");
    record_reading(&home).await;
    common::still_logged(1, || messages(&home)).await;
}
