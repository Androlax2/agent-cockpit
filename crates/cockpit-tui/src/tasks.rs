//! The basic task view: task and session snapshots are intent, never refreshed into a
//! pending edit or handoff. Scheduling remains the authority on where work may run.
use crate::{Board, Key, form::FormOutcome, task_form::TaskForm};
use cockpit_protocol::{Command, QueueExplanationView, Reply, SessionView, TaskView};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Paragraph},
};

#[derive(Debug, Default)]
pub(crate) struct Tasks {
    items: Vec<TaskView>,
    selected: Option<i64>,
    form: Option<TaskForm>,
    handoff: Option<Handoff>,
    pending: Option<Command>,
    explaining: Option<i64>,
    explanation: Option<QueueExplanationView>,
    scroll: u16,
}
#[derive(Debug)]
struct Handoff {
    task: TaskView,
    sessions: Vec<SessionView>,
    selected: usize,
}
pub(crate) enum TaskOutcome {
    Closed,
    Handled {
        used: &'static str,
        send: Vec<Command>,
    },
}
fn handled(used: &'static str, send: Vec<Command>) -> TaskOutcome {
    TaskOutcome::Handled { used, send }
}
pub(crate) fn list_command() -> Command {
    Command::TaskList {
        launch_token: None,
        project: None,
        all: false,
    }
}

impl Tasks {
    pub(crate) fn is_typing(&self) -> bool {
        self.form.is_some()
    }
    pub(crate) fn refresh_commands(&self) -> Vec<Command> {
        let mut commands = vec![list_command()];
        if let Some(item) = self.explaining {
            commands.push(Command::QueueExplain { item: Some(item) });
        }
        commands
    }
    pub(crate) fn key(&mut self, key: Key, board: &Board) -> TaskOutcome {
        if let Some(form) = &mut self.form {
            return match form.key(key) {
                FormOutcome::Cancelled => {
                    self.form = None;
                    handled("task-form-cancel", vec![])
                }
                FormOutcome::Editing => handled("task-form-edit", vec![]),
                FormOutcome::Submit(command) => handled("task-save", vec![command]),
            };
        }
        if self.explaining.is_some() {
            match key {
                Key::Esc | Key::Char('q') => {
                    self.explaining = None;
                    self.explanation = None;
                }
                Key::Down | Key::Char('j') => self.scroll = self.scroll.saturating_add(1),
                Key::Up | Key::Char('k') => self.scroll = self.scroll.saturating_sub(1),
                _ => {}
            }
            return handled("task-explanation", vec![]);
        }
        if let Some(handoff) = &mut self.handoff {
            if key == Key::Esc {
                self.handoff = None;
                return handled("task-handoff-cancel", vec![]);
            }
            if self.pending.is_some() {
                return handled("task-pending", vec![]);
            }
            let len = handoff.sessions.len();
            match key {
                Key::Down | Key::Char('j') if len > 0 => {
                    handoff.selected = (handoff.selected + 1) % len
                }
                Key::Up | Key::Char('k') if len > 0 => {
                    handoff.selected = (handoff.selected + len - 1) % len
                }
                Key::Enter if len > 0 => {
                    let session = &handoff.sessions[handoff.selected];
                    if let Some(launch) = &session.launch {
                        let command = Command::TaskHandoff {
                            item: handoff.task.id,
                            intent_revision: handoff.task.intent_revision,
                            state_revision: handoff.task.state_revision,
                            session: session.id.clone(),
                            expected_launch: launch.number,
                        };
                        self.pending = Some(command.clone());
                        return handled("task-handoff", vec![command]);
                    }
                }
                _ => {}
            }
            return handled("task-handoff-select", vec![]);
        }
        if key == Key::Esc || key == Key::Char('q') {
            return TaskOutcome::Closed;
        }
        let selected = self.items.iter().find(|t| Some(t.id) == self.selected);
        match key {
            Key::Down | Key::Char('j') | Key::Up | Key::Char('k') if !self.items.is_empty() => {
                let i = self
                    .items
                    .iter()
                    .position(|t| Some(t.id) == self.selected)
                    .unwrap_or(0);
                let delta = if matches!(key, Key::Up | Key::Char('k')) {
                    self.items.len() - 1
                } else {
                    1
                };
                self.selected = Some(self.items[(i + delta) % self.items.len()].id);
                handled("task-move", vec![])
            }
            Key::Char('a') | Key::Char('e') => {
                if key == Key::Char('e') && selected.is_none() {
                    return handled("task-edit", vec![]);
                }
                self.form = Some(TaskForm::new(
                    board.projects.iter().map(|p| p.name.clone()).collect(),
                    selected.map(|t| t.project.clone()),
                    if key == Key::Char('e') {
                        selected
                    } else {
                        None
                    },
                ));
                handled("task-form-open", vec![])
            }
            Key::Char('Q') if self.pending.is_none() => {
                if let Some(task) = selected {
                    let command = Command::TaskQueue {
                        item: task.id,
                        queued: !task.is_queued,
                    };
                    self.pending = Some(command.clone());
                    return handled("task-queue", vec![command]);
                }
                handled("task-queue", vec![])
            }
            Key::Char('h') if self.pending.is_none() => {
                if let Some(task) = selected {
                    let sessions = board
                        .sessions
                        .values()
                        .filter(|s| {
                            s.state != cockpit_protocol::SessionStateView::Ended
                                && s.launch.is_some()
                                && board.project_of(s) == Some(task.project.as_str())
                        })
                        .cloned()
                        .collect();
                    self.handoff = Some(Handoff {
                        task: task.clone(),
                        sessions,
                        selected: 0,
                    });
                }
                handled("task-handoff-open", vec![])
            }
            Key::Char('?') => {
                if let Some(task) = selected {
                    self.explaining = Some(task.id);
                    self.explanation = None;
                    self.scroll = 0;
                    return handled(
                        "task-explain",
                        vec![Command::QueueExplain {
                            item: Some(task.id),
                        }],
                    );
                }
                handled("task-explain", vec![])
            }
            _ => handled("unbound", vec![]),
        }
    }
    pub(crate) fn answer(&mut self, command: &Command, reply: &Reply) -> bool {
        match (command, reply) {
            (Command::TaskList { .. }, Reply::Tasks { tasks }) => {
                let previous: std::collections::HashMap<_, _> =
                    self.items.iter().map(|task| (task.id, task)).collect();
                self.items = tasks
                    .iter()
                    .map(|task| {
                        previous
                            .get(&task.id)
                            .copied()
                            .filter(|old| {
                                old.intent_revision > task.intent_revision
                                    || old.state_revision > task.state_revision
                            })
                            .cloned()
                            .unwrap_or_else(|| task.clone())
                    })
                    .collect();
                if !self.items.iter().any(|t| Some(t.id) == self.selected) {
                    self.selected = self.items.first().map(|t| t.id);
                }
                true
            }
            (
                Command::QueueExplain { item: Some(id) },
                Reply::QueueExplanations { explanations },
            ) if self.explaining == Some(*id) => {
                self.explanation = explanations.iter().find(|e| e.item == *id).cloned();
                true
            }
            _ => {
                let saved = self
                    .form
                    .as_ref()
                    .is_some_and(|f| f.submitted.as_ref() == Some(command))
                    && matches!(reply, Reply::Task { .. });
                if saved {
                    self.form = None;
                }
                if let Reply::Task { task } = reply {
                    if let Some(current) = self.items.iter_mut().find(|t| t.id == task.id) {
                        if task.intent_revision >= current.intent_revision
                            && task.state_revision >= current.state_revision
                        {
                            *current = (**task).clone();
                        }
                    } else if saved {
                        self.items.push((**task).clone());
                    }
                    if saved {
                        self.selected = Some(task.id);
                    }
                }
                let changed =
                    self.pending.as_ref() == Some(command) && matches!(reply, Reply::Task { .. });
                if changed {
                    self.pending = None;
                    self.handoff = None;
                }
                saved || changed
            }
        }
    }
    pub(crate) fn failed(&mut self, command: &Command, problem: &str) {
        if let Some(form) = &mut self.form
            && form.submitted.as_ref() == Some(command)
        {
            form.submitted = None;
            form.problem = Some(problem.to_owned());
        }
        if self.pending.as_ref() == Some(command) {
            self.pending = None;
        }
    }
    pub(crate) fn draw(&self, frame: &mut Frame<'_>, area: Rect) {
        if let Some(form) = &self.form {
            form.draw(frame, area);
            return;
        }
        let mut lines = Vec::new();
        let title;
        if let Some(h) = &self.handoff {
            title = "Hand task to session";
            lines.push(format!("Item {}: {}", h.task.id, h.task.title));
            lines.push("j/k selects; Enter hands off; Esc cancels".into());
            if h.sessions.is_empty() {
                lines.push("No launched session in this project".into());
            }
            lines.extend(h.sessions.iter().enumerate().map(|(i, s)| {
                format!(
                    "{} {} {} {}",
                    if i == h.selected { ">" } else { " " },
                    s.id,
                    s.agent,
                    s.state
                )
            }));
        } else if let Some(item) = self.explaining {
            title = "Queue explanation";
            lines.push(format!("Item {item}; j/k scrolls; Esc returns"));
            if let Some(e) = &self.explanation {
                lines.extend([
                    e.title.clone(),
                    e.why.clone(),
                    format!("Action ({}): {}", e.action_by, e.action),
                    format!(
                        "Priority {} + aging {}; demand {} millicores / {} MiB",
                        e.priority, e.aging_boost, e.demand.cpu_millis, e.demand.memory_mib
                    ),
                    format!(
                        "Evaluated {}; next {}",
                        e.evaluated_at, e.next_evaluation_at
                    ),
                ]);
                if let Some(limit) = &e.limited_by {
                    lines.push(format!("Limited by {limit}"));
                }
                lines.extend(
                    e.sessions
                        .iter()
                        .map(|s| format!("{}: {}", s.session, s.why)),
                );
            } else {
                lines.push("No explanation received; see the status line for refusals".into());
            }
        } else {
            title = "Tasks";
            lines.push("a add; e edit; h hand off; Q queue; ? why; Esc back".into());
            if self.pending.is_some() {
                lines.push("Waiting for the daemon…".into());
            }
            if self.items.is_empty() {
                lines.push("No open tasks".into());
            }
            lines.extend(self.items.iter().map(|t| {
                format!(
                    "{} {} {} {} P{} {} {}",
                    if Some(t.id) == self.selected {
                        ">"
                    } else {
                        " "
                    },
                    t.id,
                    t.project,
                    t.status.as_str(),
                    t.priority,
                    if t.is_queued { "[Q]" } else { "[ ]" },
                    t.title
                )
            }));
        }
        let width = usize::from(area.width.saturating_sub(2));
        let height = usize::from(area.height.saturating_sub(2));
        let mut rows = Vec::new();
        let mut selected_rows = None;
        for line in lines {
            let start = rows.len();
            for part in line.lines() {
                rows.extend(crate::render::wrapped(part, width, ""));
            }
            if line.starts_with("> ") {
                selected_rows = Some(start..rows.len());
            }
        }
        let offset = if self.explaining.is_some() {
            usize::from(self.scroll).min(rows.len().saturating_sub(height))
        } else {
            selected_rows.map_or(0, |rows| crate::render::scroll_to_show(&rows, height))
        };
        frame.render_widget(
            Paragraph::new(rows.join("\n"))
                .scroll((u16::try_from(offset).unwrap_or(u16::MAX), 0))
                .block(Block::default().borders(Borders::ALL).title(title)),
            area,
        );
    }
}
