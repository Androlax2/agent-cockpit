//! #62, #682: which event the notifier tells for what the daemon says, and when it tells
//! nothing. Driven through the change feed and the daemon's answers, as the runtime does.

mod common;

use cockpit_notify::{Event, Notice, Notifier};
use cockpit_protocol::{
    AgentExitView, AgentState, Command, DiskView, EntityKind, ExitStatusView, QuestionSource,
    QuotaAlertView, SessionReport, SessionStateView, SessionView, TurnOutcome,
};
use common::{
    after_snapshot, change_session, guide, loaded, network_request, report, run, session,
    set_network, set_queue, set_reports, snapshot, tombstone, upsert, waiting_run,
};

const WORKER: &str = "s00000000000a";

fn worker() -> SessionView {
    session(WORKER, 1, 1)
}

fn running() -> Notifier {
    loaded(&[worker()], vec![report(WORKER)])
}

fn events(notices: &[Notice]) -> Vec<Event> {
    notices.iter().map(|notice| notice.event).collect()
}

fn asking(question: &str, source: QuestionSource) -> SessionReport {
    SessionReport {
        state: Some(AgentState::Waiting),
        needs_input: Some(question.to_owned()),
        question_source: Some(source),
        ..report(WORKER)
    }
}

fn ended_turn(outcome: TurnOutcome) -> SessionReport {
    SessionReport {
        state: Some(AgentState::Waiting),
        outcome: Some(outcome),
        ..report(WORKER)
    }
}

const STATE_DIR_DISK: &str = "state-dir";

fn disk(free_mib: u64) -> DiskView {
    DiskView {
        free_mib,
        floor_mib: 2048,
    }
}

fn change_disk(notifier: &mut Notifier, reading: &DiskView) -> Vec<Notice> {
    notifier
        .apply_changes(&after_snapshot(vec![upsert(
            EntityKind::Disk,
            STATE_DIR_DISK,
            reading,
        )]))
        .unwrap()
}

#[test]
fn entering_low_disk_is_told_once_with_the_free_space() {
    let mut notifier = running();

    let told = change_disk(&mut notifier, &disk(900));

    assert_eq!(events(&told), [Event::Disk]);
    assert_eq!(told[0].name, "Free disk space");
    assert_eq!(
        told[0].detail.as_deref(),
        Some("900 MiB free, below the floor of 2048 MiB")
    );
}

#[test]
fn staying_low_is_not_told_again() {
    let mut notifier = running();
    change_disk(&mut notifier, &disk(900));

    let told = change_disk(&mut notifier, &disk(400));

    assert_eq!(told, []);
}

fn retract_disk(notifier: &mut Notifier) -> Vec<Notice> {
    notifier
        .apply_changes(&after_snapshot(vec![tombstone(
            EntityKind::Disk,
            STATE_DIR_DISK,
        )]))
        .unwrap()
}

#[test]
fn leaving_low_disk_tells_nothing_and_entering_it_again_tells_again() {
    let mut notifier = running();
    change_disk(&mut notifier, &disk(900));

    let left = retract_disk(&mut notifier);
    let entered_again = change_disk(&mut notifier, &disk(800));

    assert_eq!(left, []);
    assert_eq!(events(&entered_again), [Event::Disk]);
}

#[test]
fn a_disk_already_low_when_the_notifier_starts_is_not_news() {
    let mut notifier = Notifier::default();
    let mut told = notifier
        .apply_changes(&snapshot(vec![upsert(
            EntityKind::Disk,
            STATE_DIR_DISK,
            &disk(900),
        )]))
        .unwrap();
    told.extend(set_reports(&mut notifier, Vec::new()));
    told.extend(set_queue(&mut notifier, Vec::new(), Vec::new()));
    told.extend(set_network(&mut notifier, Vec::new()));

    assert_eq!(told, []);
    assert_eq!(change_disk(&mut notifier, &disk(400)), []);
}

#[test]
fn the_low_disk_is_no_session_in_the_status_file() {
    let mut notifier = running();
    change_disk(&mut notifier, &disk(900));

    let status = notifier.status(true, &Default::default());

    assert_eq!(status.sessions.len(), 1);
    assert_eq!(status.needs_you, 0);
}

const FIVE_HOUR: &str = "claude/default/five-hour";

fn quota_alert(threshold: u8, used_percent: u32, fired_at: i64) -> QuotaAlertView {
    QuotaAlertView {
        provider: "claude".to_owned(),
        account: "default".to_owned(),
        limit: "five-hour".to_owned(),
        scope: None,
        threshold,
        used_percent,
        resets_at: Some(1_800_018_000),
        fired_at,
    }
}

fn change_quota_alert(notifier: &mut Notifier, alert: &QuotaAlertView) -> Vec<Notice> {
    notifier
        .apply_changes(&after_snapshot(vec![upsert(
            EntityKind::QuotaAlert,
            FIVE_HOUR,
            alert,
        )]))
        .unwrap()
}

#[test]
fn a_window_passing_a_threshold_is_told_with_the_window_and_its_use() {
    let mut notifier = running();

    let told = change_quota_alert(&mut notifier, &quota_alert(80, 83, 1_800_000_000));

    assert_eq!(events(&told), [Event::Usage]);
    assert_eq!(told[0].name, "claude five-hour window");
    assert_eq!(
        told[0].detail.as_deref(),
        Some("83% used, alert at 80%, account default")
    );
}

#[test]
fn the_next_threshold_of_the_same_window_is_told_again() {
    let mut notifier = running();
    change_quota_alert(&mut notifier, &quota_alert(80, 83, 1_800_000_000));

    let told = change_quota_alert(&mut notifier, &quota_alert(95, 96, 1_800_000_600));

    assert_eq!(events(&told), [Event::Usage]);
    assert_eq!(
        told[0].detail.as_deref(),
        Some("96% used, alert at 95%, account default")
    );
}

#[test]
fn the_same_threshold_in_the_next_window_is_told_again() {
    let mut notifier = running();
    change_quota_alert(&mut notifier, &quota_alert(80, 83, 1_800_000_000));

    let next_window = QuotaAlertView {
        resets_at: Some(1_800_036_000),
        ..quota_alert(80, 81, 1_800_020_000)
    };
    let told = change_quota_alert(&mut notifier, &next_window);

    assert_eq!(events(&told), [Event::Usage]);
}

#[test]
fn a_quota_alert_already_on_the_feed_when_the_notifier_starts_is_not_news() {
    let mut notifier = Notifier::default();
    let mut told = notifier
        .apply_changes(&snapshot(vec![upsert(
            EntityKind::QuotaAlert,
            FIVE_HOUR,
            &quota_alert(80, 83, 1_800_000_000),
        )]))
        .unwrap();
    told.extend(set_reports(&mut notifier, Vec::new()));
    told.extend(set_queue(&mut notifier, Vec::new(), Vec::new()));
    told.extend(set_network(&mut notifier, Vec::new()));

    assert_eq!(told, []);
}

#[test]
fn a_quota_alert_is_no_session_in_the_status_file() {
    let mut notifier = running();
    change_quota_alert(&mut notifier, &quota_alert(80, 83, 1_800_000_000));

    let status = notifier.status(true, &Default::default());

    assert_eq!(status.sessions.len(), 1);
    assert_eq!(status.needs_you, 0);
}

#[test]
fn a_session_the_task_list_says_finished_is_told_once() {
    let mut notifier = running();
    let finished = SessionView {
        finished: true,
        ..worker()
    };

    assert_eq!(
        events(&change_session(&mut notifier, &finished)),
        [Event::Finished]
    );
    assert_eq!(change_session(&mut notifier, &finished), []);
}

#[test]
fn a_finish_the_feed_and_the_report_both_show_is_told_once() {
    let mut notifier = running();

    let from_feed = change_session(
        &mut notifier,
        &SessionView {
            finished: true,
            ..worker()
        },
    );
    let from_report = set_reports(&mut notifier, vec![ended_turn(TurnOutcome::Completed)]);

    assert_eq!(events(&from_feed), [Event::Finished]);
    assert_eq!(events(&from_report), [Event::Turn]);
}

#[test]
fn a_completed_turn_alone_is_a_turn_and_never_finished() {
    let mut notifier = running();

    let told = set_reports(&mut notifier, vec![ended_turn(TurnOutcome::Completed)]);

    assert_eq!(events(&told), [Event::Turn]);
}

#[test]
fn the_task_item_done_after_a_turn_tells_finished_once() {
    let mut notifier = running();
    set_reports(&mut notifier, vec![ended_turn(TurnOutcome::Completed)]);

    let told = change_session(
        &mut notifier,
        &SessionView {
            finished: true,
            ..worker()
        },
    );

    assert_eq!(events(&told), [Event::Finished]);
}

#[test]
fn each_turn_outcome_is_its_own_event() {
    for (outcome, event) in [
        (TurnOutcome::Completed, Event::Turn),
        (TurnOutcome::Failed { reason: None }, Event::Failure),
        (TurnOutcome::QuotaStopped, Event::Quota),
    ] {
        let mut notifier = running();

        let told = set_reports(&mut notifier, vec![ended_turn(outcome.clone())]);

        assert_eq!(events(&told), [event], "{outcome:?}");
    }
}

#[test]
fn the_agents_own_question_is_a_question_with_its_text() {
    let mut notifier = running();

    let told = set_reports(
        &mut notifier,
        vec![asking("Which branch?", QuestionSource::Agent)],
    );

    assert_eq!(events(&told), [Event::Question]);
    assert_eq!(told[0].detail.as_deref(), Some("Which branch?"));
}

#[test]
fn a_providers_permission_dialog_is_a_hold() {
    let mut notifier = running();

    let told = set_reports(
        &mut notifier,
        vec![asking("Allow Bash(rm -rf build)?", QuestionSource::Dialog)],
    );

    assert_eq!(events(&told), [Event::Hold]);
}

#[test]
fn a_question_a_coordinator_holds_is_not_told() {
    let mut notifier = running();

    let told = set_reports(
        &mut notifier,
        vec![SessionReport {
            held_by: Some("s00000000000c".to_owned()),
            ..asking("Which branch?", QuestionSource::Agent)
        }],
    );

    assert_eq!(told, []);
}

#[test]
fn a_command_waiting_for_approval_is_a_hold_naming_the_command() {
    let mut notifier = running();

    let told = set_queue(&mut notifier, vec![run(1, WORKER, "make test")], Vec::new());

    assert_eq!(events(&told), [Event::Hold]);
    assert_eq!(told[0].detail.as_deref(), Some("make test"));
}

#[test]
fn a_refused_network_host_is_a_hold_naming_the_host() {
    let mut notifier = running();

    let told = set_network(&mut notifier, vec![network_request(4, WORKER)]);

    assert_eq!(events(&told), [Event::Hold]);
    assert_eq!(told[0].detail.as_deref(), Some("unknown.example.com:443"));
}

#[test]
fn a_guide_or_a_command_waiting_for_input_is_a_question() {
    let mut notifier = running();

    let told = set_queue(
        &mut notifier,
        vec![waiting_run(2, WORKER)],
        vec![guide(3, WORKER, None)],
    );

    assert_eq!(events(&told), [Event::Question, Event::Question]);
}

#[test]
fn a_second_approval_while_the_first_is_open_is_told_again() {
    let mut notifier = running();
    set_queue(&mut notifier, vec![run(1, WORKER, "make test")], Vec::new());

    let told = set_queue(
        &mut notifier,
        vec![run(1, WORKER, "make test"), run(2, WORKER, "make lint")],
        Vec::new(),
    );

    assert_eq!(told.len(), 1);
    assert_eq!(told[0].detail.as_deref(), Some("make lint"));
}

#[test]
fn a_session_that_failed_is_told_with_its_reason() {
    let mut notifier = running();

    let told = change_session(
        &mut notifier,
        &SessionView {
            state: SessionStateView::Failed,
            reason: Some("the launch failed".to_owned()),
            ..worker()
        },
    );

    assert_eq!(events(&told), [Event::Failure]);
    assert_eq!(told[0].detail.as_deref(), Some("the launch failed"));
}

#[test]
fn an_agent_that_exited_by_itself_is_a_failure() {
    let mut notifier = running();

    let told = change_session(
        &mut notifier,
        &SessionView {
            state: SessionStateView::Ended,
            exit: Some(AgentExitView {
                status: ExitStatusView::Code { code: 1 },
                output: vec!["panic".to_owned()],
            }),
            ..worker()
        },
    );

    assert_eq!(events(&told), [Event::Failure]);
}

#[test]
fn a_session_the_owner_ended_is_not_told() {
    let mut notifier = running();

    let told = change_session(
        &mut notifier,
        &SessionView {
            state: SessionStateView::Ended,
            ..worker()
        },
    );

    assert_eq!(told, []);
}

#[test]
fn a_session_taken_off_the_feed_is_not_told() {
    let mut notifier = running();

    let told = notifier
        .apply_changes(&after_snapshot(vec![tombstone(
            EntityKind::Session,
            WORKER,
        )]))
        .unwrap();

    assert_eq!(told, []);
}

#[test]
fn events_that_land_together_come_in_one_answer() {
    let mut notifier = running();

    let told = set_queue(
        &mut notifier,
        vec![run(1, WORKER, "make test")],
        vec![guide(2, WORKER, None)],
    );

    assert_eq!(events(&told), [Event::Hold, Event::Question]);
}

#[test]
fn what_already_needed_the_owner_at_start_is_not_told() {
    let mut notifier = loaded(
        &[SessionView {
            finished: true,
            ..worker()
        }],
        vec![asking("Which branch?", QuestionSource::Agent)],
    );

    let refreshed = set_reports(
        &mut notifier,
        vec![asking("Which branch?", QuestionSource::Agent)],
    );

    assert_eq!(refreshed, []);
}

#[test]
fn a_report_request_that_failed_does_not_keep_the_notifier_silent() {
    let mut notifier = Notifier::default();
    notifier
        .apply_changes(&common::snapshot(vec![common::upsert(
            EntityKind::Session,
            WORKER,
            &worker(),
        )]))
        .unwrap();
    notifier.answer(&Command::AgentReports, Err("timed out".to_owned()));
    set_queue(&mut notifier, Vec::new(), Vec::new());
    set_network(&mut notifier, Vec::new());

    let told = set_reports(
        &mut notifier,
        vec![asking("Which branch?", QuestionSource::Agent)],
    );

    assert_eq!(events(&told), [Event::Question]);
}

#[test]
fn a_notice_names_the_session_by_its_reported_label() {
    let mut notifier = running();

    let told = set_reports(
        &mut notifier,
        vec![SessionReport {
            label: Some("fix login".to_owned()),
            ..ended_turn(TurnOutcome::Completed)
        }],
    );

    assert_eq!(told[0].name, "fix login");
}

/// `torlek notify` passes the owner's settings by the names cockpit-config declares; the
/// client may not depend on the config crate, so its source is the contract.
#[test]
fn the_owner_settings_name_the_same_events_in_the_same_order() {
    let settings = include_str!("../../cockpit-config/src/notify.rs");
    let names: Vec<String> = Event::ALL
        .iter()
        .map(|event| format!("{:?}", event.name()))
        .collect();
    let declaration = format!(
        "pub const NOTIFY_EVENTS: [&str; {}] = [{}];",
        names.len(),
        names.join(", ")
    );
    // rustfmt wraps a long array over lines and ends it with a comma.
    let without_layout = |text: &str| {
        text.split_whitespace()
            .collect::<String>()
            .replace(",]", "]")
    };

    assert!(
        without_layout(settings).contains(&without_layout(&declaration)),
        "expected {declaration}"
    );
}
