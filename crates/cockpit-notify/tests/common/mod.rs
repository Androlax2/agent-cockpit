//! Wire views and change-feed batches for the notifier's tests.
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use cockpit_notify::{Notice, Notifier};
use cockpit_protocol::{
    AgentState, Change, ChangeBatch, ChangeEvent, CheckoutKindView, CheckoutView, Command,
    CommitRevision, Cursor, DatabaseEpoch, EntityKind, EntityRef, EntityRevision, FeedPosition,
    GuideStateView, GuideStepView, GuideView, NetworkRequestStateView, NetworkRequestView,
    NetworkSubjectView, NetworkTargetView, Reply, RunRequestView, RunStateView, SessionReport,
    SessionStateView, SessionView, ShownText, TrustView,
};

pub fn checkout(id: i64, project: &str) -> CheckoutView {
    CheckoutView {
        id,
        project: project.to_owned(),
        kind: CheckoutKindView::Main,
        path: format!("/p/{project}"),
        incarnation: 1,
        removing: false,
        occupant: None,
        trust: TrustView::Trusted,
    }
}

pub fn session(id: &str, checkout: i64, created_at: i64) -> SessionView {
    SessionView {
        id: id.to_owned(),
        agent: "fake".to_owned(),
        checkout,
        holds_checkout: false,
        working_dir: "/p/app".to_owned(),
        conversation: Some("c-1".to_owned()),
        model: None,
        effort: None,
        budget_minutes: None,
        budget_spent: false,
        design_link: None,
        state: SessionStateView::Running,
        reason: None,
        revision: 1,
        created_at,
        changed_at: created_at,
        launch: None,
        exit: None,
        changed_start_files: Vec::new(),
        restart_pending: false,
        finished: false,
        coordinator_grants: Vec::new(),
        inbound_message: None,
        compaction_threshold: None,
        sign_in_needed: None,
        quota_stop: None,
    }
}

pub fn shown(text: &str) -> ShownText {
    ShownText {
        text: text.to_owned(),
        has_hidden_characters: false,
        non_ascii: Vec::new(),
        warnings: Vec::new(),
    }
}

pub fn run(id: i64, session: &str, command: &str) -> RunRequestView {
    RunRequestView {
        id,
        session: session.to_owned(),
        revision: 1,
        state: RunStateView::Asking,
        command: command.to_owned(),
        asked_command: None,
        cwd: "/p/app".to_owned(),
        reason: "to check".to_owned(),
        shown_command: shown(command),
        shown_cwd: shown("/p/app"),
        shown_reason: shown("to check"),
        project: Some("app".to_owned()),
        outside_project: false,
        folded_command: None,
        timeout_without_foreground: false,
        unchecked_shell: None,
        changed_build_files: None,
        digest: format!("digest-{id}-1"),
        asked_at: 1,
        expires_at: 1000,
        waiting_for_input: false,
        waiting_prompt: None,
        sudo_ask: None,
        sudo_ran_unasked: 0,
        typed_secret_redacted: 0,
        root_runs: None,
        findings: None,
        decision: None,
        result: None,
        note: None,
        already_asked: false,
        writes: Vec::new(),
        hosts: Vec::new(),
        limits: Vec::new(),
    }
}

/// A running command whose idle output ends like a prompt for a line of input.
pub fn waiting_run(id: i64, session: &str) -> RunRequestView {
    RunRequestView {
        state: RunStateView::Running,
        waiting_for_input: true,
        decision: None,
        ..run(id, session, "read -r answer")
    }
}

pub fn guide(id: i64, session: &str, ask: Option<&str>) -> GuideView {
    GuideView {
        id,
        session: session.to_owned(),
        title: shown("Set up the token"),
        cwd: "/p/app".to_owned(),
        shown_cwd: shown("/p/app"),
        project: Some("app".to_owned()),
        outside_project: false,
        state: GuideStateView::Asking,
        current_step: 0,
        steps: vec![GuideStepView {
            section: None,
            text: shown("Create a token"),
            open: Vec::new(),
            copy: Vec::new(),
            ask: ask.map(shown),
            run: None,
            answer: None,
            run_request: None,
        }],
        stuck_note: None,
        asked_at: 1,
        expires_at: 1000,
    }
}

/// A change carrying `view` as the current state of `kind` `id`.
pub fn upsert(kind: EntityKind, id: &str, view: &impl serde::Serialize) -> ChangeEvent {
    let serde_json::Value::Object(state) = serde_json::to_value(view).unwrap() else {
        panic!("a view is a JSON object");
    };
    ChangeEvent {
        entity: EntityRef {
            kind,
            id: id.to_owned(),
        },
        entity_revision: EntityRevision(1),
        change: Change::Upsert { state },
    }
}

/// A change taking `kind` `id` off the feed.
pub fn tombstone(kind: EntityKind, id: &str) -> ChangeEvent {
    ChangeEvent {
        entity: EntityRef {
            kind,
            id: id.to_owned(),
        },
        entity_revision: EntityRevision(2),
        change: Change::Tombstone,
    }
}

fn feed_cursor(revision: u64) -> Cursor {
    Cursor {
        epoch: DatabaseEpoch("e1".to_owned()),
        revision: CommitRevision(revision),
    }
}

/// A snapshot batch of `changes`.
pub fn snapshot(changes: Vec<ChangeEvent>) -> ChangeBatch {
    ChangeBatch {
        follows: FeedPosition::Snapshot,
        cursor: feed_cursor(7),
        changes,
    }
}

/// A batch of `changes` following the snapshot of [`snapshot`].
pub fn after_snapshot(changes: Vec<ChangeEvent>) -> ChangeBatch {
    ChangeBatch {
        follows: FeedPosition::Cursor(feed_cursor(7)),
        cursor: feed_cursor(8),
        changes,
    }
}

pub fn report(session: &str) -> SessionReport {
    SessionReport {
        compaction: None,
        session: session.to_owned(),
        state: Some(AgentState::Working),
        label: None,
        doing: None,
        needs_input: None,
        outcome: None,
        question_source: None,
        covered_question: None,
        held_by: None,
        reported_at: 1,
    }
}

pub fn network_request(id: i64, session: &str) -> NetworkRequestView {
    NetworkRequestView {
        id,
        subject: NetworkSubjectView::Session {
            id: session.to_owned(),
        },
        project: "app".to_owned(),
        asks: NetworkTargetView::Host {
            host: "unknown.example.com".to_owned(),
            port: 443,
        },
        target: "unknown.example.com:443".to_owned(),
        state: NetworkRequestStateView::Asking,
        asked_at: 1_000,
        decision: None,
        grant: None,
        shown_reason: None,
    }
}

/// A notifier that loaded the daemon's state at start: `sessions` from the feed, `reports`,
/// and an empty queue and network list. What it started on is never news.
pub fn loaded(sessions: &[SessionView], reports: Vec<SessionReport>) -> Notifier {
    let mut notifier = Notifier::default();
    let changes = sessions
        .iter()
        .map(|view| upsert(EntityKind::Session, &view.id, view))
        .collect();
    let mut told = notifier.apply_changes(&snapshot(changes)).unwrap();
    told.extend(notifier.answer(&Command::AgentReports, Ok(Reply::AgentReports { reports })));
    told.extend(set_queue(&mut notifier, Vec::new(), Vec::new()));
    told.extend(set_network(&mut notifier, Vec::new()));
    assert_eq!(told, Vec::new(), "the start state is not news");
    notifier
}

pub fn set_reports(notifier: &mut Notifier, reports: Vec<SessionReport>) -> Vec<Notice> {
    notifier.answer(&Command::AgentReports, Ok(Reply::AgentReports { reports }))
}

pub fn set_queue(
    notifier: &mut Notifier,
    runs: Vec<RunRequestView>,
    guides: Vec<GuideView>,
) -> Vec<Notice> {
    notifier.answer(
        &Command::ApprovalQueue,
        Ok(Reply::ApprovalQueue { runs, guides }),
    )
}

pub fn set_network(notifier: &mut Notifier, requests: Vec<NetworkRequestView>) -> Vec<Notice> {
    notifier.answer(
        &Command::NetworkList,
        Ok(Reply::NetworkRequests { requests }),
    )
}

pub fn change_session(notifier: &mut Notifier, view: &SessionView) -> Vec<Notice> {
    notifier
        .apply_changes(&after_snapshot(vec![upsert(
            EntityKind::Session,
            &view.id,
            view,
        )]))
        .unwrap()
}
