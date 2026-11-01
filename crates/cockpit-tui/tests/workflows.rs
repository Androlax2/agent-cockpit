//! The workflow view, `F` (#330): a running workflow's steps, checkpoint, next action and last
//! handoff from `workflow-list` and `workflow-packet`, and the owner's actions as keys.

mod common;

use std::time::Instant;

use cockpit_protocol::{
    Command, Reply, TaskKindView, TaskStatusView, WorkflowCheckpointView, WorkflowGoalView,
    WorkflowHandoffRecordView, WorkflowHandoffView, WorkflowNextView, WorkflowPacketView,
    WorkflowStatusView, WorkflowStepStatusView, WorkflowStepView, WorkflowView,
};
use cockpit_tui::{App, Key};
use common::{answer, app_with, press, screen_text, sent, typing};

const LIST: Command = Command::WorkflowList {
    launch_token: None,
    project: None,
    all: false,
};

fn packet_command(workflow: i64) -> Command {
    Command::WorkflowPacket {
        launch_token: None,
        workflow,
    }
}

fn step(position: u32, name: &str, status: WorkflowStepStatusView) -> WorkflowStepView {
    WorkflowStepView {
        position,
        name: name.to_owned(),
        purpose: format!("{name} the change"),
        doer: "agent".to_owned(),
        status,
        session: None,
        checkpoint: None,
        attempts: 0,
        requirements: Vec::new(),
        required_capabilities: vec!["events".to_owned(), "outcome".to_owned()],
    }
}

/// Workflow 7, `code-change` on item 3: `s0123456789ab` implements, at its checkpoint.
fn code_change() -> WorkflowView {
    let implement = WorkflowStepView {
        session: Some("s0123456789ab".to_owned()),
        checkpoint: Some(WorkflowCheckpointView {
            note: "Validation rewritten; tests next".to_owned(),
            revision: 1,
            at: 10,
        }),
        attempts: 1,
        ..step(1, "implement", WorkflowStepStatusView::Active)
    };
    let check = WorkflowStepView {
        requirements: vec!["checks".to_owned(), "review".to_owned()],
        ..step(2, "check", WorkflowStepStatusView::Pending)
    };
    WorkflowView {
        id: 7,
        item: 3,
        project: "app".to_owned(),
        preset: "code-change".to_owned(),
        preset_version: 1,
        status: WorkflowStatusView::Running,
        intent_revision: 1,
        blocked: None,
        steps: vec![
            implement,
            check,
            step(3, "deliver", WorkflowStepStatusView::Pending),
        ],
        next: WorkflowNextView::InProgress {
            position: 1,
            session: Some("s0123456789ab".to_owned()),
        },
        created_at: 1,
        changed_at: 10,
    }
}

/// The same workflow once its first step is done: `check` is what a session begins next.
fn at_check() -> WorkflowView {
    let mut run = code_change();
    run.steps[0].status = WorkflowStepStatusView::Done;
    run.steps[1].status = WorkflowStepStatusView::Ready;
    run.next = WorkflowNextView::AwaitPlacement { position: 2 };
    run
}

fn packet(goal: Option<WorkflowGoalView>) -> WorkflowPacketView {
    WorkflowPacketView {
        workflow: 7,
        status: WorkflowStatusView::Running,
        preset: "code-change".to_owned(),
        preset_version: 1,
        item: 3,
        goal,
        project: "app".to_owned(),
        step: None,
        next: WorkflowNextView::AwaitPlacement { position: 2 },
        blocked: None,
        decisions: Vec::new(),
        handoffs: vec![WorkflowHandoffRecordView {
            position: 1,
            step: "implement".to_owned(),
            kind: "completed".to_owned(),
            session: Some("s0123456789ab".to_owned()),
            handoff: WorkflowHandoffView {
                changed: "Rewrote the form validation".to_owned(),
                checked: "commit abc123".to_owned(),
                outcome: "12 passed".to_owned(),
                ..WorkflowHandoffView::default()
            },
            satisfied: Vec::new(),
            waived: Vec::new(),
            at: 20,
        }],
    }
}

fn goal() -> WorkflowGoalView {
    WorkflowGoalView {
        title: "Fix the login form".to_owned(),
        notes: String::new(),
        kind: TaskKindView::Bug,
        item_status: TaskStatusView::Doing,
        started_on_intent: 1,
        intent_revision: 1,
        intent_changed: false,
    }
}

/// The view open on `run`, the daemon having answered the list and the packet.
fn opened(run: WorkflowView, packet: WorkflowPacketView, now: Instant) -> App {
    let mut app = app_with(&["app"], Vec::new(), now);
    assert_eq!(sent(&press(&mut app, &[Key::Char('F')], now)), [LIST]);
    answer(
        &mut app,
        LIST,
        Reply::Workflows {
            workflows: vec![run],
        },
        now,
    );
    assert_eq!(
        sent(&press(&mut app, &[Key::Enter], now)),
        [packet_command(7)]
    );
    answer(
        &mut app,
        packet_command(7),
        Reply::WorkflowPacket {
            packet: Box::new(packet),
        },
        now,
    );
    app
}

#[test]
fn a_running_code_change_workflow_shows_its_active_steps_checkpoint_and_next() {
    let now = Instant::now();
    let app = opened(code_change(), packet(Some(goal())), now);

    let shown = screen_text(&app, now);

    assert!(
        shown.contains("1 implement  agent  active  by s0123456789ab"),
        "{shown}"
    );
    assert!(
        shown.contains("checkpoint: Validation rewritten; tests next"),
        "{shown}"
    );
    assert!(
        shown.contains("next: s0123456789ab works on step 1"),
        "{shown}"
    );
    assert!(
        shown.contains("item 3: Fix the login form (doing)"),
        "{shown}"
    );
}

#[test]
fn the_list_names_each_running_workflow_and_who_acts_next() {
    let now = Instant::now();
    let mut app = app_with(&["app"], Vec::new(), now);
    press(&mut app, &[Key::Char('F')], now);
    answer(
        &mut app,
        LIST,
        Reply::Workflows {
            workflows: vec![at_check()],
        },
        now,
    );

    let shown = screen_text(&app, now);

    assert!(
        shown.contains("> 7  app  item 3  code-change  next: step 2 waits for a session"),
        "{shown}"
    );
}

#[test]
fn the_last_handoff_shows_under_the_steps() {
    let now = Instant::now();
    let app = opened(at_check(), packet(Some(goal())), now);

    let shown = screen_text(&app, now);

    assert!(
        shown.contains("last handoff, step 1 implement (completed): changed: Rewrote the form"),
        "{shown}"
    );
}

#[test]
fn a_workflow_whose_item_was_deleted_says_so() {
    let now = Instant::now();
    let app = opened(at_check(), packet(None), now);

    assert!(
        screen_text(&app, now).contains("item 3: the item was deleted"),
        "{}",
        screen_text(&app, now)
    );
}

#[test]
fn while_open_the_refresh_keeps_the_list_and_the_packet_current() {
    let now = Instant::now();
    let app = opened(code_change(), packet(Some(goal())), now);

    assert_eq!(
        app.view_refresh_commands(),
        [
            LIST,
            Command::WorkflowShow {
                launch_token: None,
                workflow: 7
            },
            packet_command(7)
        ]
    );
}

#[test]
fn cancel_and_release_are_sent_only_after_y() {
    let now = Instant::now();
    let mut app = opened(code_change(), packet(Some(goal())), now);

    let declined = sent(&press(&mut app, &[Key::Char('x'), Key::Char('n')], now));
    let released = sent(&press(&mut app, &[Key::Char('p'), Key::Char('y')], now));
    let cancelled = sent(&press(&mut app, &[Key::Char('x'), Key::Char('y')], now));

    assert_eq!(declined, []);
    assert_eq!(
        released,
        [Command::WorkflowRelease {
            launch_token: None,
            workflow: 7,
            handoff: None,
        }]
    );
    assert_eq!(cancelled, [Command::WorkflowCancel { workflow: 7 }]);
}

#[test]
fn unblock_is_sent_at_once() {
    let now = Instant::now();
    let mut app = opened(code_change(), packet(Some(goal())), now);

    assert_eq!(
        sent(&press(&mut app, &[Key::Char('b')], now)),
        [Command::WorkflowUnblock {
            launch_token: None,
            workflow: 7,
        }]
    );
}

#[test]
fn complete_sends_the_typed_handoff_waiving_the_current_steps_requirements() {
    let now = Instant::now();
    let mut app = opened(at_check(), packet(Some(goal())), now);
    press(&mut app, &[Key::Char('a')], now);
    typing(&mut app, "Reviewed by hand", now);
    press(&mut app, &[Key::Tab], now);
    typing(&mut app, "commit def456", now);
    press(&mut app, &[Key::Tab], now);
    typing(&mut app, "the owner read the diff", now);

    let completed = sent(&press(&mut app, &[Key::Enter], now));

    assert_eq!(
        completed,
        [Command::WorkflowComplete {
            launch_token: None,
            workflow: 7,
            handoff: WorkflowHandoffView {
                changed: "Reviewed by hand".to_owned(),
                checked: "commit def456".to_owned(),
                outcome: "the owner read the diff".to_owned(),
                ..WorkflowHandoffView::default()
            },
            waive: vec!["checks".to_owned(), "review".to_owned()],
        }]
    );
}

#[test]
fn an_incomplete_handoff_is_not_sent_and_says_why() {
    let now = Instant::now();
    let mut app = opened(at_check(), packet(Some(goal())), now);
    press(&mut app, &[Key::Char('a')], now);
    typing(&mut app, "Reviewed by hand", now);

    let completed = sent(&press(&mut app, &[Key::Enter], now));

    assert_eq!(completed, []);
    assert!(
        screen_text(&app, now).contains("Cannot complete: each of the three fields is needed"),
        "{}",
        screen_text(&app, now)
    );
}

#[test]
fn an_action_answer_updates_the_opened_workflow() {
    let now = Instant::now();
    let mut app = opened(code_change(), packet(Some(goal())), now);
    let cancel = Command::WorkflowCancel { workflow: 7 };
    let mut cancelled = code_change();
    cancelled.status = WorkflowStatusView::Cancelled;
    cancelled.next = WorkflowNextView::Nothing;

    answer(
        &mut app,
        cancel,
        Reply::Workflow {
            workflow: Box::new(cancelled),
        },
        now,
    );

    let shown = screen_text(&app, now);
    assert!(
        shown.contains("code-change v1  cancelled") && shown.contains("next: nothing"),
        "{shown}"
    );
}

#[test]
fn esc_goes_back_to_the_list_then_closes_the_view() {
    let now = Instant::now();
    let mut app = opened(code_change(), packet(Some(goal())), now);

    let used: Vec<_> = press(&mut app, &[Key::Esc, Key::Esc], now)
        .into_iter()
        .map(|step| step.used)
        .collect();

    assert_eq!(used, [Some("workflow-back"), Some("workflows-close")]);
    assert!(app.workflows().is_none());
}

#[test]
fn a_handoff_typed_for_one_step_is_not_sent_once_the_workflow_moved_on() {
    let now = Instant::now();
    let mut app = opened(at_check(), packet(Some(goal())), now);
    press(&mut app, &[Key::Char('a')], now);
    typing(&mut app, "a", now);
    press(&mut app, &[Key::Tab], now);
    typing(&mut app, "b", now);
    press(&mut app, &[Key::Tab], now);
    typing(&mut app, "c", now);
    let mut moved_on = at_check();
    moved_on.steps[1].status = WorkflowStepStatusView::Done;
    moved_on.steps[2].status = WorkflowStepStatusView::Ready;
    moved_on.next = WorkflowNextView::AwaitPlacement { position: 3 };
    answer(
        &mut app,
        Command::WorkflowShow {
            launch_token: None,
            workflow: 7,
        },
        Reply::Workflow {
            workflow: Box::new(moved_on),
        },
        now,
    );

    let completed = sent(&press(&mut app, &[Key::Enter], now));

    assert_eq!(completed, []);
    assert!(
        screen_text(&app, now).contains("the workflow moved on to another step"),
        "{}",
        screen_text(&app, now)
    );
}

#[test]
fn a_refused_complete_keeps_the_typed_handoff_and_says_why() {
    let now = Instant::now();
    let mut app = opened(at_check(), packet(Some(goal())), now);
    press(&mut app, &[Key::Char('a')], now);
    typing(&mut app, "Reviewed by hand", now);
    press(&mut app, &[Key::Tab], now);
    typing(&mut app, "commit def456", now);
    press(&mut app, &[Key::Tab], now);
    typing(&mut app, "read the diff", now);
    let complete = sent(&press(&mut app, &[Key::Enter], now)).remove(0);

    app.answer(
        &complete,
        Err("conflict: workflow 7 is blocked".to_owned()),
        now,
    );

    let shown = screen_text(&app, now);
    assert!(
        shown.contains("What changed: Reviewed by hand")
            && shown.contains("Cannot complete: conflict: workflow 7 is blocked"),
        "{shown}"
    );
}

#[test]
fn the_opened_workflow_is_shown_by_itself_so_one_delivered_elsewhere_shows_how_it_ended() {
    let now = Instant::now();
    let mut app = opened(code_change(), packet(Some(goal())), now);
    let show = Command::WorkflowShow {
        launch_token: None,
        workflow: 7,
    };
    assert!(app.view_refresh_commands().contains(&show));
    let mut delivered = code_change();
    delivered.status = WorkflowStatusView::Delivered;
    delivered.next = WorkflowNextView::Nothing;

    answer(
        &mut app,
        LIST,
        Reply::Workflows {
            workflows: Vec::new(),
        },
        now,
    );
    answer(
        &mut app,
        show,
        Reply::Workflow {
            workflow: Box::new(delivered),
        },
        now,
    );

    assert!(
        screen_text(&app, now).contains("code-change v1  delivered"),
        "{}",
        screen_text(&app, now)
    );
}

#[test]
fn an_approval_pane_that_takes_focus_keeps_its_keys_while_the_view_is_open() {
    let now = Instant::now();
    let mut app = opened(code_change(), packet(Some(goal())), now);
    common::queue(
        &mut app,
        vec![common::run(1, "s1", "make test")],
        Vec::new(),
        now,
    );

    let declined = sent(&press(
        &mut app,
        &[Key::Char('x')],
        now + cockpit_tui::INPUT_GRACE,
    ));

    assert!(
        !declined
            .iter()
            .any(|command| matches!(command, Command::WorkflowCancel { .. })),
        "{declined:?}"
    );
    assert_eq!(app.focus(), cockpit_tui::Focus::Pane);
}
