//! The workflow view, `F` (#330): the running workflows, and for the one opened its preset,
//! its steps with status and checkpoint, who acts next, the blocker and the last handoff, read
//! from `workflow-list` and `workflow-packet` and refreshed with the list while open. The
//! owner's actions are keys: complete the step waiving its requirements (with a handoff typed
//! here), release the session doing it, unblock, cancel. The daemon decides whether each one
//! is allowed; a refusal reaches the status line like any other.

use cockpit_protocol::{
    Command, Reply, WorkflowHandoffView, WorkflowNextView, WorkflowPacketView, WorkflowView,
};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::Key;

/// What a key did in the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowsOutcome {
    Handled {
        used: &'static str,
        send: Vec<Command>,
    },
    Closed,
}

/// An action the owner confirms with `y`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowConfirm {
    Release,
    Cancel,
}

/// The handoff the owner types to complete the opened workflow's step: the step it was opened
/// for, and the requirements the owner saw it would waive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandoffEntry {
    pub(crate) position: Option<u32>,
    pub(crate) waive: Vec<String>,
    /// Changed, checked, outcome.
    pub(crate) fields: [String; 3],
    pub(crate) field: usize,
    pub(crate) problem: Option<String>,
    /// The `workflow-complete` sent and not answered yet: the entry stays until it is
    /// accepted, so a refusal keeps what the owner typed.
    pub(crate) submitted: Option<Command>,
}

const HANDOFF_LABELS: [&str; 3] = ["What changed", "What was checked", "Outcome"];

#[derive(Debug, Default)]
pub struct Workflows {
    /// The running workflows, as the last `workflow-list` answered.
    runs: Vec<WorkflowView>,
    selected: Option<i64>,
    /// The workflow shown in detail, as last answered; it stays shown once it is no longer
    /// running, so the owner sees how it ended.
    opened: Option<WorkflowView>,
    packet: Option<WorkflowPacketView>,
    pub(crate) confirm: Option<WorkflowConfirm>,
    pub(crate) handoff: Option<HandoffEntry>,
}

fn handled(used: &'static str, send: Vec<Command>) -> WorkflowsOutcome {
    WorkflowsOutcome::Handled { used, send }
}

fn list_command() -> Command {
    Command::WorkflowList {
        launch_token: None,
        project: None,
        all: false,
    }
}

fn show_command(workflow: i64) -> Command {
    Command::WorkflowShow {
        launch_token: None,
        workflow,
    }
}

fn packet_command(workflow: i64) -> Command {
    Command::WorkflowPacket {
        launch_token: None,
        workflow,
    }
}

impl Workflows {
    /// The view and the command that fills it.
    pub fn open() -> (Workflows, Command) {
        (Workflows::default(), list_command())
    }

    pub fn opened(&self) -> Option<&WorkflowView> {
        self.opened.as_ref()
    }

    pub fn selected(&self) -> Option<i64> {
        self.selected
    }

    /// Whether the owner is typing a handoff: every key is text then.
    pub fn is_typing(&self) -> bool {
        self.handoff.is_some()
    }

    /// What keeps the view current, sent with every refresh while it is open. The opened
    /// workflow is shown by itself, so one delivered or cancelled elsewhere shows how it ended.
    pub fn refresh_commands(&self) -> Vec<Command> {
        let mut commands = vec![list_command()];
        if let Some(run) = &self.opened {
            commands.extend([show_command(run.id), packet_command(run.id)]);
        }
        commands
    }

    pub fn key(&mut self, key: Key) -> WorkflowsOutcome {
        if self.handoff.is_some() {
            return self.handoff_key(key);
        }
        if let Some(confirm) = self.confirm.take() {
            return match (key, &self.opened) {
                (Key::Char('y'), Some(run)) => match confirm {
                    WorkflowConfirm::Release => handled(
                        "workflow-release",
                        vec![Command::WorkflowRelease {
                            launch_token: None,
                            workflow: run.id,
                            handoff: None,
                        }],
                    ),
                    WorkflowConfirm::Cancel => handled(
                        "workflow-cancel",
                        vec![Command::WorkflowCancel { workflow: run.id }],
                    ),
                },
                _ => handled("workflow-confirm-cancel", Vec::new()),
            };
        }
        match &self.opened {
            Some(run) => self.detail_key(key, run.id),
            None => self.list_key(key),
        }
    }

    fn list_key(&mut self, key: Key) -> WorkflowsOutcome {
        match key {
            Key::Esc | Key::Char('q') => WorkflowsOutcome::Closed,
            Key::Char('j') | Key::Down => {
                self.move_selection(1);
                handled("workflow-move", Vec::new())
            }
            Key::Char('k') | Key::Up => {
                self.move_selection(-1);
                handled("workflow-move", Vec::new())
            }
            Key::Enter => {
                let Some(run) = self.runs.iter().find(|run| Some(run.id) == self.selected) else {
                    return handled("workflow-open", Vec::new());
                };
                self.opened = Some(run.clone());
                self.packet = None;
                handled("workflow-open", vec![packet_command(run.id)])
            }
            _ => handled("unbound", Vec::new()),
        }
    }

    fn detail_key(&mut self, key: Key, workflow: i64) -> WorkflowsOutcome {
        match key {
            Key::Esc | Key::Char('q') => {
                self.opened = None;
                self.packet = None;
                handled("workflow-back", Vec::new())
            }
            Key::Char('a') => {
                self.handoff = Some(self.handoff_for_current_step());
                handled("workflow-complete-form", Vec::new())
            }
            Key::Char('p') => {
                self.confirm = Some(WorkflowConfirm::Release);
                handled("workflow-release-ask", Vec::new())
            }
            Key::Char('x') => {
                self.confirm = Some(WorkflowConfirm::Cancel);
                handled("workflow-cancel-ask", Vec::new())
            }
            Key::Char('b') => handled(
                "workflow-unblock",
                vec![Command::WorkflowUnblock {
                    launch_token: None,
                    workflow,
                }],
            ),
            _ => handled("unbound", Vec::new()),
        }
    }

    fn handoff_key(&mut self, key: Key) -> WorkflowsOutcome {
        let Some(entry) = &mut self.handoff else {
            unreachable!("handoff_key runs only while the handoff is typed");
        };
        match key {
            Key::Esc => {
                self.handoff = None;
                handled("workflow-complete-cancel", Vec::new())
            }
            Key::Tab | Key::Down => {
                entry.field = (entry.field + 1) % HANDOFF_LABELS.len();
                handled("form-edit", Vec::new())
            }
            Key::BackTab | Key::Up => {
                entry.field = (entry.field + HANDOFF_LABELS.len() - 1) % HANDOFF_LABELS.len();
                handled("form-edit", Vec::new())
            }
            Key::Char(typed) => {
                entry.fields[entry.field].push(typed);
                handled("form-edit", Vec::new())
            }
            Key::Backspace => {
                entry.fields[entry.field].pop();
                handled("form-edit", Vec::new())
            }
            Key::Enter => self.complete(),
            Key::Left | Key::Right => handled("form-edit", Vec::new()),
        }
    }

    /// A handoff for the opened workflow's current step, waiving every requirement the step
    /// declares: a requirement the owner does not waive waits for verification, which the TUI
    /// cannot claim.
    fn handoff_for_current_step(&self) -> HandoffEntry {
        let current = self.opened.as_ref().and_then(|run| {
            let position = current_position(&run.next)?;
            run.steps.iter().find(|step| step.position == position)
        });
        HandoffEntry {
            position: current.map(|step| step.position),
            waive: current
                .map(|step| step.requirements.clone())
                .unwrap_or_default(),
            ..HandoffEntry::default()
        }
    }

    /// Completes the opened workflow's step with the typed handoff, refused here when the
    /// workflow moved on to another step since the entry opened: the owner waives only the
    /// requirements shown.
    fn complete(&mut self) -> WorkflowsOutcome {
        let (Some(entry), Some(run)) = (&mut self.handoff, &self.opened) else {
            unreachable!("the handoff is typed only for an opened workflow");
        };
        if entry.submitted.is_some() {
            return handled("workflow-complete", Vec::new());
        }
        if current_position(&run.next) != entry.position {
            entry.problem = Some(
                "the workflow moved on to another step: Esc, then a to complete that one"
                    .to_owned(),
            );
            return handled("workflow-complete", Vec::new());
        }
        if entry.fields.iter().any(|field| field.trim().is_empty()) {
            entry.problem = Some("each of the three fields is needed".to_owned());
            return handled("workflow-complete", Vec::new());
        }
        let [changed, checked, outcome] = entry.fields.clone();
        let command = Command::WorkflowComplete {
            launch_token: None,
            workflow: run.id,
            handoff: WorkflowHandoffView {
                changed,
                checked,
                outcome,
                needs_human: String::new(),
                evidence: Vec::new(),
            },
            waive: entry.waive.clone(),
        };
        entry.problem = None;
        entry.submitted = Some(command.clone());
        handled("workflow-complete", vec![command])
    }

    /// The daemon refused `command`: a refused complete keeps the typed handoff, with why.
    pub fn failed(&mut self, command: &Command, problem: &str) {
        if let Some(entry) = &mut self.handoff
            && entry.submitted.as_ref() == Some(command)
        {
            entry.submitted = None;
            entry.problem = Some(problem.to_owned());
        }
    }

    fn move_selection(&mut self, step: isize) {
        if self.runs.is_empty() {
            self.selected = None;
            return;
        }
        let at = self
            .runs
            .iter()
            .position(|run| Some(run.id) == self.selected);
        let next = match at {
            None => 0,
            Some(at) => at.saturating_add_signed(step).min(self.runs.len() - 1),
        };
        self.selected = Some(self.runs[next].id);
    }

    /// Takes the daemon's answer to a workflow command; returns whether it was one.
    pub fn answer(&mut self, command: &Command, reply: &Reply) -> bool {
        match reply {
            Reply::Workflows { workflows } => {
                self.runs = workflows.clone();
                if let Some(opened) = &mut self.opened
                    && let Some(listed) = workflows.iter().find(|run| run.id == opened.id)
                {
                    *opened = listed.clone();
                }
                if !self.runs.iter().any(|run| Some(run.id) == self.selected) {
                    self.selected = self.runs.first().map(|run| run.id);
                }
            }
            Reply::Workflow { workflow } => {
                if self
                    .handoff
                    .as_ref()
                    .is_some_and(|entry| entry.submitted.as_ref() == Some(command))
                {
                    self.handoff = None;
                }
                if let Some(opened) = &mut self.opened
                    && opened.id == workflow.id
                {
                    *opened = (**workflow).clone();
                }
                if let Some(listed) = self.runs.iter_mut().find(|run| run.id == workflow.id) {
                    *listed = (**workflow).clone();
                }
            }
            Reply::WorkflowPacket { packet } => {
                if self.opened.as_ref().map(|run| run.id) == Some(packet.workflow) {
                    self.packet = Some((**packet).clone());
                }
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn draw(&self, frame: &mut Frame<'_>, area: Rect) {
        let (title, text) = match (&self.handoff, &self.opened) {
            (Some(entry), _) => ("Complete the step", handoff_lines(entry)),
            (None, Some(run)) => ("Workflow", self.detail_lines(run)),
            (None, None) => ("Workflows", self.list_lines()),
        };
        frame.render_widget(
            Paragraph::new(text.join("\n"))
                .wrap(Wrap { trim: false })
                .block(Block::default().borders(Borders::ALL).title(title)),
            area,
        );
    }

    fn list_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .runs
            .iter()
            .map(|run| {
                let marker = if Some(run.id) == self.selected {
                    ">"
                } else {
                    " "
                };
                format!(
                    "{marker} {}  {}  item {}  {}  next: {}",
                    run.id,
                    run.project,
                    run.item,
                    run.preset,
                    next_text(&run.next)
                )
            })
            .collect();
        if lines.is_empty() {
            lines.push("No running workflow: cockpit workflow start ITEM".to_owned());
        }
        lines.push(String::new());
        lines.push("j / k select  Enter open  Esc close".to_owned());
        lines
    }

    fn detail_lines(&self, run: &WorkflowView) -> Vec<String> {
        let goal = match self.packet.as_ref().map(|packet| &packet.goal) {
            None => "loading…".to_owned(),
            Some(None) => "the item was deleted".to_owned(),
            Some(Some(goal)) => format!("{} ({})", goal.title, goal.item_status.as_str()),
        };
        let mut lines = vec![
            format!(
                "workflow {}  {} v{}  {}  project {}",
                run.id,
                run.preset,
                run.preset_version,
                run.status.as_str(),
                run.project
            ),
            format!("item {}: {goal}", run.item),
            "steps".to_owned(),
        ];
        for step in &run.steps {
            let mut line = format!(
                "  {} {}  {}  {}",
                step.position,
                step.name,
                step.doer,
                step.status.as_str()
            );
            if let Some(session) = &step.session {
                line.push_str(&format!("  by {session}"));
            }
            if !step.requirements.is_empty() {
                line.push_str(&format!("  requires {}", step.requirements.join(", ")));
            }
            lines.push(line);
            if let Some(checkpoint) = &step.checkpoint {
                lines.push(format!("      checkpoint: {}", checkpoint.note));
            }
        }
        lines.push(format!("next: {}", next_text(&run.next)));
        if let Some(blocked) = &run.blocked {
            lines.push(format!(
                "blocked: {}: {}",
                blocked.action.as_str(),
                blocked.detail
            ));
        }
        if let Some(record) = self
            .packet
            .as_ref()
            .and_then(|packet| packet.handoffs.last())
        {
            lines.push(format!(
                "last handoff, step {} {} ({}): changed: {}; checked: {}; outcome: {}",
                record.position,
                record.step,
                record.kind,
                record.handoff.changed,
                record.handoff.checked,
                record.handoff.outcome
            ));
        }
        lines.push(String::new());
        lines.push(match self.confirm {
            Some(WorkflowConfirm::Release) => "release the step's session? y to confirm".to_owned(),
            Some(WorkflowConfirm::Cancel) => "cancel this workflow? y to confirm".to_owned(),
            None => {
                "a complete, waiving its requirements  p release  b unblock  x cancel  Esc back"
                    .to_owned()
            }
        });
        lines
    }
}

fn handoff_lines(entry: &HandoffEntry) -> Vec<String> {
    let mut lines: Vec<String> = HANDOFF_LABELS
        .iter()
        .zip(&entry.fields)
        .enumerate()
        .map(|(index, (label, value))| {
            let marker = if index == entry.field { ">" } else { " " };
            format!("{marker} {label}: {value}")
        })
        .collect();
    lines.push(String::new());
    lines.push(
        "Tab next field  Enter complete, waiving the step's requirements  Esc cancel".to_owned(),
    );
    if let Some(problem) = &entry.problem {
        lines.push(format!("Cannot complete: {problem}"));
    }
    lines
}

/// The step the next action is about, if any.
fn current_position(next: &WorkflowNextView) -> Option<u32> {
    match next {
        WorkflowNextView::AwaitPlacement { position }
        | WorkflowNextView::OwnerCompletes { position }
        | WorkflowNextView::InProgress { position, .. } => Some(*position),
        WorkflowNextView::Unblock { .. }
        | WorkflowNextView::Nothing
        | WorkflowNextView::Unknown => None,
    }
}

fn next_text(next: &WorkflowNextView) -> String {
    match next {
        WorkflowNextView::Unblock { action } => format!("unblock: {}", action.as_str()),
        WorkflowNextView::AwaitPlacement { position } => {
            format!("step {position} waits for a session")
        }
        WorkflowNextView::OwnerCompletes { position } => {
            format!("you complete step {position}")
        }
        WorkflowNextView::InProgress {
            position,
            session: Some(session),
        } => format!("{session} works on step {position}"),
        WorkflowNextView::InProgress {
            position,
            session: None,
        } => format!("step {position} in progress"),
        WorkflowNextView::Nothing => "nothing".to_owned(),
        WorkflowNextView::Unknown => "unknown to this TUI".to_owned(),
    }
}
