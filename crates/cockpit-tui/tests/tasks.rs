//! #44's basic task actions through App input, replies and rendered output.
mod common;
use cockpit_protocol::{Command, Reply, TaskView};
use cockpit_tui::{App, Key};
use common::{answer, app_with, press, screen_text, typing};
use std::time::Instant;

const LIST: Command = Command::TaskList {
    launch_token: None,
    project: None,
    all: false,
};
fn task() -> TaskView {
    serde_json::from_value(serde_json::json!({
        "id": 7, "project": "app", "title": "Fix login", "notes": "Keep Unicode: café",
        "kind": "bug", "priority": 3, "status": "todo", "is_blocker_report": false,
        "is_queued": false, "waits": [], "images": [], "intent_revision": 2, "state_revision": 3,
        "created_at": 1, "changed_at": 2, "next": {"kind": "queue"}
    }))
    .unwrap()
}
fn opened(now: Instant) -> App {
    let mut app = app_with(&["app"], vec![], now);
    let step = app.key(Key::Char('T'), now);
    assert_eq!(step.send, vec![LIST]);
    answer(
        &mut app,
        LIST,
        Reply::Tasks {
            tasks: vec![task()],
        },
        now,
    );
    app
}
#[test]
fn task_list_renders_item_status_project_and_queue_action() {
    let now = Instant::now();
    let mut app = opened(now);
    let text = screen_text(&app, now);
    assert!(
        text.contains("Fix login") && text.contains("app") && text.contains("todo"),
        "{text}"
    );
    assert_eq!(
        app.key(Key::Char('Q'), now).send,
        vec![Command::TaskQueue {
            item: 7,
            queued: true
        }]
    );
    assert!(
        app.key(Key::Char('Q'), now).send.is_empty(),
        "do not duplicate a pending mutation"
    );
}
#[test]
fn edit_keeps_its_revision_and_text_when_refresh_and_refusal_arrive() {
    let now = Instant::now();
    let mut app = opened(now);
    app.key(Key::Char('e'), now);
    typing(&mut app, " safely", now);
    let mut newer = task();
    newer.intent_revision = 9;
    newer.title = "Changed elsewhere".into();
    answer(&mut app, LIST, Reply::Tasks { tasks: vec![newer] }, now);
    let sent = app.key(Key::Enter, now).send;
    assert_eq!(
        sent,
        vec![Command::TaskEdit {
            item: 7,
            intent_revision: Some(2),
            title: Some("Fix login safely".into()),
            notes: Some("Keep Unicode: café".into()),
            item_kind: Some(cockpit_protocol::TaskKindView::Bug)
        }]
    );
    app.answer(
        &sent[0],
        Err("stale revision; reopen to edit current intent".into()),
        now,
    );
    let text = screen_text(&app, now);
    assert!(
        text.contains("Fix login safely") && text.contains("stale revision"),
        "{text}"
    );
}
#[test]
fn add_sends_project_title_notes_and_kind_and_waits_for_success() {
    let now = Instant::now();
    let mut app = opened(now);
    app.key(Key::Char('a'), now);
    typing(&mut app, "New task", now);
    press(&mut app, &[Key::Tab], now);
    typing(&mut app, "A note", now);
    let sent = app.key(Key::Enter, now).send;
    assert!(
        matches!(&sent[..], [Command::TaskAdd { project: Some(project), title, notes, .. }] if project == "app" && title == "New task" && notes == "A note")
    );
    assert!(app.key(Key::Enter, now).send.is_empty());
    app.answer(&sent[0], Err("cannot save".into()), now);
    assert!(screen_text(&app, now).contains("New task"));
}

#[test]
fn handoff_keeps_the_displayed_task_and_session_launch_until_cancelled() {
    let now = Instant::now();
    let mut session = common::session("s0123456789ab", 1, 1);
    session.launch = Some(cockpit_protocol::LaunchView {
        number: 4,
        kind: cockpit_protocol::LaunchKindView::Start,
        launched_at: 1,
    });
    let mut app = app_with(&["app"], vec![session.clone()], now);
    app.key(Key::Char('T'), now);
    answer(
        &mut app,
        LIST,
        Reply::Tasks {
            tasks: vec![task()],
        },
        now,
    );
    app.key(Key::Char('h'), now);
    session.launch.as_mut().unwrap().number = 8;
    session.revision += 1;
    app.apply_changes(&common::after_snapshot(vec![common::upsert(
        cockpit_protocol::EntityKind::Session,
        &session.id,
        &session,
    )]))
    .unwrap();
    let mut newer = task();
    newer.intent_revision = 10;
    answer(&mut app, LIST, Reply::Tasks { tasks: vec![newer] }, now);
    let sent = app.key(Key::Enter, now).send;
    assert_eq!(
        sent,
        vec![Command::TaskHandoff {
            item: 7,
            intent_revision: 2,
            state_revision: 3,
            session: session.id.clone(),
            expected_launch: 4
        }]
    );
    app.answer(&sent[0], Err("selected launch is stale".into()), now);
    assert_eq!(
        app.key(Key::Enter, now).send,
        sent,
        "refusal does not retarget stale intent"
    );
    app.answer(&sent[0], Err("selected launch is stale".into()), now);
    press(&mut app, &[Key::Esc, Key::Char('h')], now);
    assert!(matches!(
        &app.key(Key::Enter, now).send[..],
        [Command::TaskHandoff {
            intent_revision: 10,
            expected_launch: 8,
            ..
        }]
    ));
}

#[test]
fn queue_success_updates_the_row_and_an_older_list_cannot_undo_it() {
    let now = Instant::now();
    let mut app = opened(now);
    let command = app.key(Key::Char('Q'), now).send.remove(0);
    let mut queued = task();
    queued.is_queued = true;
    queued.state_revision += 1;
    assert_eq!(
        answer(
            &mut app,
            command,
            Reply::Task {
                task: Box::new(queued)
            },
            now
        ),
        Some(LIST)
    );
    answer(
        &mut app,
        LIST,
        Reply::Tasks {
            tasks: vec![task()],
        },
        now,
    );
    assert_eq!(
        app.key(Key::Char('Q'), now).send,
        vec![Command::TaskQueue {
            item: 7,
            queued: false
        }]
    );
}

#[test]
fn explanation_shows_the_daemons_reason_limits_action_and_next_evaluation() {
    let now = Instant::now();
    let mut app = opened(now);
    let command = Command::QueueExplain { item: Some(7) };
    assert_eq!(app.key(Key::Char('?'), now).send, vec![command.clone()]);
    let explanation = serde_json::from_value(serde_json::json!({
        "item":7, "project":"app", "title":"Fix login", "reason":"no-room", "why":"Needs more memory",
        "limited_by":"shared budget", "sessions":[], "action":"Wait for capacity", "action_by":"torlek",
        "priority":3, "aging_boost":1, "demand":{"cpu_millis":500,"memory_mib":512}, "headroom_percent":20,
        "evaluated_at":100, "next_evaluation_at":120
    })).unwrap();
    answer(
        &mut app,
        command,
        Reply::QueueExplanations {
            explanations: vec![explanation],
        },
        now,
    );
    let text = screen_text(&app, now);
    for expected in [
        "Needs more memory",
        "shared budget",
        "Wait for capacity",
        "120",
        "512",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    app.key(Key::Esc, now);
    assert!(screen_text(&app, now).contains("Tasks"));
}

#[test]
fn cancelling_a_form_and_opening_another_ignores_the_old_save_answer() {
    let now = Instant::now();
    let mut app = opened(now);
    app.key(Key::Char('e'), now);
    let old = app.key(Key::Enter, now).send.remove(0);
    press(&mut app, &[Key::Esc, Key::Char('a')], now);
    typing(&mut app, "Unsaved new task", now);
    answer(
        &mut app,
        old,
        Reply::Task {
            task: Box::new(task()),
        },
        now,
    );
    assert!(screen_text(&app, now).contains("Unsaved new task"));
    for (width, height) in [(20, 8), (40, 12), (100, 40)] {
        let drawn = common::sized_screen(&app, now, width, height);
        assert_eq!(drawn.len(), usize::from(height));
    }
}

#[test]
fn the_approval_pane_remains_reachable_from_the_task_list() {
    let now = Instant::now();
    let mut app = opened(now);
    common::queue(
        &mut app,
        vec![common::run(1, "s0123456789ab", "check")],
        vec![],
        now,
    );
    assert_eq!(app.focus(), cockpit_tui::Focus::Pane);
    app.key(Key::Tab, now);
    assert_eq!(app.focus(), cockpit_tui::Focus::List);
    assert_eq!(
        app.key(Key::Char('?'), now).send,
        vec![Command::QueueExplain { item: Some(7) }]
    );
    app.key(Key::Tab, now);
    assert_eq!(app.focus(), cockpit_tui::Focus::Pane);
}

#[test]
fn delayed_manager_focus_does_not_take_keys_from_an_open_task_draft() {
    let now = Instant::now();
    let mut app = app_with(&["app"], vec![], now);
    let focus = common::queue(&mut app, vec![common::run(1, "s1", "check")], vec![], now).unwrap();
    common::queue(&mut app, vec![], vec![], now);
    app.key(Key::Char('T'), now);
    app.key(Key::Char('a'), now);
    assert_eq!(
        answer(
            &mut app,
            focus,
            Reply::ManagerFocused {
                took_from_viewer: true
            },
            now
        ),
        None
    );
}

#[test]
fn the_session_picker_scrolls_to_keep_the_selected_target_visible() {
    let now = Instant::now();
    let sessions = (1..=30)
        .map(|i| {
            let mut session = common::session(&format!("s{i:012}"), 1, i);
            session.launch = Some(cockpit_protocol::LaunchView {
                number: i,
                kind: cockpit_protocol::LaunchKindView::Start,
                launched_at: 1,
            });
            session
        })
        .collect();
    let mut app = app_with(&["app"], sessions, now);
    app.key(Key::Char('T'), now);
    answer(
        &mut app,
        LIST,
        Reply::Tasks {
            tasks: vec![task()],
        },
        now,
    );
    app.key(Key::Char('h'), now);
    for _ in 0..29 {
        app.key(Key::Down, now);
    }
    let text = common::sized_screen(&app, now, 20, 12).join("\n");
    assert!(text.contains("> s000000000030"), "{text}");
}

#[test]
fn a_finished_session_folded_out_of_the_session_list_is_still_a_handoff_target() {
    let now = Instant::now();
    let mut session = common::session("s0123456789ab", 1, 1);
    session.finished = true;
    session.launch = Some(cockpit_protocol::LaunchView {
        number: 4,
        kind: cockpit_protocol::LaunchKindView::Start,
        launched_at: 1,
    });
    let mut app = app_with(&["app"], vec![session.clone()], now);
    app.key(Key::Char('T'), now);
    answer(
        &mut app,
        LIST,
        Reply::Tasks {
            tasks: vec![task()],
        },
        now,
    );
    app.key(Key::Char('h'), now);
    assert!(screen_text(&app, now).contains(&session.id));
    assert!(
        matches!(&app.key(Key::Enter, now).send[..], [Command::TaskHandoff { session: target, expected_launch:4, .. }] if *target == session.id)
    );
}
