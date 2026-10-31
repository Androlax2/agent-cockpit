//! Wire views for the TUI's tests, and the screen as text.
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use std::time::Instant;

use cockpit_protocol::{
    CheckoutKindView, CheckoutView, Command, FoldedView, GuideStateView, GuideStepView, GuideView,
    HiddenView, ProjectView, Reply, RunRequestView, RunStateView, SessionStateView, SessionView,
    ShownText, TrustView,
};
use cockpit_tui::{App, Key, Step};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

pub fn project(name: &str) -> ProjectView {
    ProjectView {
        name: name.to_owned(),
        root: format!("/p/{name}"),
    }
}

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
        design_link: None,
        state: SessionStateView::Running,
        reason: None,
        revision: 1,
        created_at,
        changed_at: created_at,
        launch: None,
        changed_start_files: Vec::new(),
        restart_pending: false,
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
        changed_build_files: None,
        digest: format!("digest-{id}-1"),
        asked_at: 1,
        expires_at: 1000,
        waiting_for_input: false,
        result: None,
        note: None,
    }
}

/// A run that started and stopped mid-line: it waits for a line of input.
pub fn waiting_run(id: i64, session: &str) -> RunRequestView {
    RunRequestView {
        state: RunStateView::Running,
        waiting_for_input: true,
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

pub fn hidden(text: &str, kinds: Vec<HiddenView>, non_ascii: Vec<&str>) -> ShownText {
    ShownText {
        text: text.to_owned(),
        has_hidden_characters: true,
        non_ascii: non_ascii.into_iter().map(str::to_owned).collect(),
        warnings: kinds,
    }
}

pub fn folded(preview: &str, more_lines: usize, more_chars: usize) -> Option<FoldedView> {
    Some(FoldedView {
        preview: preview.to_owned(),
        more_lines,
        more_chars,
    })
}

/// An app that knows `projects`, their main checkouts (ids from 1) and `sessions`.
pub fn app_with(projects: &[&str], sessions: Vec<SessionView>, now: Instant) -> App {
    let mut app = App::new();
    answer(
        &mut app,
        Command::ProjectList,
        Reply::Projects {
            projects: projects.iter().map(|name| project(name)).collect(),
        },
        now,
    );
    answer(
        &mut app,
        Command::CheckoutList { project: None },
        Reply::Checkouts {
            checkouts: projects
                .iter()
                .zip(1..)
                .map(|(name, id)| checkout(id, name))
                .collect(),
        },
        now,
    );
    answer(
        &mut app,
        Command::SessionList { ended: false },
        Reply::Sessions { sessions },
        now,
    );
    app
}

pub fn answer(app: &mut App, command: Command, reply: Reply, now: Instant) {
    app.answer(&command, Ok(reply), now);
}

pub fn queue(app: &mut App, runs: Vec<RunRequestView>, guides: Vec<GuideView>, now: Instant) {
    answer(
        app,
        Command::ApprovalQueue,
        Reply::ApprovalQueue { runs, guides },
        now,
    );
}

pub fn press(app: &mut App, keys: &[Key], now: Instant) -> Vec<Step> {
    keys.iter().map(|key| app.key(*key, now)).collect()
}

pub fn typing(app: &mut App, text: &str, now: Instant) -> Vec<Step> {
    text.chars()
        .map(|character| app.key(Key::Char(character), now))
        .collect()
}

/// The screen `app` draws on a 100×40 terminal, one string per row, right-trimmed.
pub fn screen(app: &App, now: Instant) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    terminal
        .draw(|frame| cockpit_tui::draw(app, frame, now))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|row| {
            (0..buffer.area.width)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

pub fn screen_text(app: &App, now: Instant) -> String {
    screen(app, now).join("\n")
}

/// Every command the steps send.
pub fn sent(steps: &[Step]) -> Vec<Command> {
    steps.iter().flat_map(|step| step.send.clone()).collect()
}
