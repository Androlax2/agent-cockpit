//! A task draft keeps the exact edit revision until it is accepted or cancelled.
use crate::{Key, form::FormOutcome};
use cockpit_protocol::{Command, TaskKindView, TaskView};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Paragraph, Wrap},
};

#[derive(Debug)]
pub(crate) struct TaskForm {
    projects: Vec<String>,
    project: String,
    item: Option<(i64, i64)>,
    title: String,
    notes: String,
    kind: TaskKindView,
    field: usize,
    pub(crate) problem: Option<String>,
    pub(crate) submitted: Option<Command>,
}
impl TaskForm {
    pub(crate) fn new(
        projects: Vec<String>,
        project: Option<String>,
        item: Option<&TaskView>,
    ) -> Self {
        Self {
            project: item
                .map(|t| t.project.clone())
                .or(project)
                .or_else(|| projects.first().cloned())
                .unwrap_or_default(),
            projects,
            item: item.map(|t| (t.id, t.intent_revision)),
            title: item.map(|t| t.title.clone()).unwrap_or_default(),
            notes: item.map(|t| t.notes.clone()).unwrap_or_default(),
            kind: item.map(|t| t.kind).unwrap_or(TaskKindView::Task),
            field: 0,
            problem: None,
            submitted: None,
        }
    }
    pub(crate) fn key(&mut self, key: Key) -> FormOutcome {
        if key == Key::Esc {
            return FormOutcome::Cancelled;
        }
        if self.submitted.is_some() {
            return FormOutcome::Editing;
        }
        match key {
            Key::Enter => return self.submit(),
            Key::Tab | Key::Down => self.field = (self.field + 1) % 4,
            Key::BackTab | Key::Up => self.field = (self.field + 3) % 4,
            Key::Left | Key::Right if self.field == 2 => {
                let kinds = TaskKindView::KNOWN;
                let i = kinds.iter().position(|k| *k == self.kind).unwrap_or(0);
                self.kind =
                    kinds[(i + if key == Key::Left { kinds.len() - 1 } else { 1 }) % kinds.len()];
            }
            Key::Left | Key::Right
                if self.field == 3 && self.item.is_none() && !self.projects.is_empty() =>
            {
                let len = self.projects.len();
                let i = self
                    .projects
                    .iter()
                    .position(|p| p == &self.project)
                    .unwrap_or(0);
                self.project =
                    self.projects[(i + if key == Key::Left { len - 1 } else { 1 }) % len].clone();
            }
            Key::Char(c) if !c.is_control() || (c == '\n' && self.field == 1) => {
                if self.field == 0 {
                    self.title.push(c);
                }
                if self.field == 1 {
                    self.notes.push(c);
                }
            }
            Key::Backspace => {
                if self.field == 0 {
                    self.title.pop();
                }
                if self.field == 1 {
                    self.notes.pop();
                }
            }
            _ => {}
        }
        FormOutcome::Editing
    }
    fn submit(&mut self) -> FormOutcome {
        if self.title.trim().is_empty() || self.project.is_empty() {
            self.problem = Some("Choose a project and enter a title".into());
            return FormOutcome::Editing;
        }
        let command = match self.item {
            Some((item, revision)) => Command::TaskEdit {
                launch_token: None,
                item,
                intent_revision: Some(revision),
                title: Some(self.title.clone()),
                notes: Some(self.notes.clone()),
                item_kind: Some(self.kind),
            },
            None => Command::TaskAdd {
                launch_token: None,
                project: Some(self.project.clone()),
                title: self.title.clone(),
                notes: self.notes.clone(),
                item_kind: Some(self.kind),
                priority: None,
                after: Vec::new(),
            },
        };
        self.problem = None;
        self.submitted = Some(command.clone());
        FormOutcome::Submit(Box::new(command))
    }
    pub(crate) fn draw(&self, frame: &mut Frame<'_>, area: Rect) {
        let fields = [
            ("Title", self.title.as_str()),
            ("Notes", self.notes.as_str()),
            ("Kind (Left/Right)", self.kind.as_str()),
            ("Project (Left/Right)", self.project.as_str()),
        ];
        let mut lines = vec!["Tab changes field; Enter saves; Esc cancels".to_owned()];
        if self.submitted.is_some() {
            lines.push("Saving…".into());
        }
        if let Some(problem) = &self.problem {
            lines.push(problem.clone());
        }
        lines.extend(fields.into_iter().enumerate().map(|(i, (label, value))| {
            format!(
                "{} {label}: {value}",
                if i == self.field { ">" } else { " " }
            )
        }));
        frame.render_widget(
            Paragraph::new(lines.join("\n"))
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(if self.item.is_some() {
                            "Edit task"
                        } else {
                            "Add task"
                        }),
                ),
            area,
        );
    }
}
