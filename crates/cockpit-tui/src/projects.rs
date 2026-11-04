//! The registered projects (#727): `d` asks to remove the selected one, and only `y` sends
//! the removal. The daemon refuses a project still in use and says why on the status line.
use cockpit_protocol::Command;
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Block, Borders, Paragraph},
};

use crate::{Board, Key};

#[derive(Debug, Default)]
pub(crate) struct Projects {
    selected: usize,
    /// The project the owner asked to remove, waiting for `y`.
    confirming: Option<String>,
}

pub(crate) enum ProjectsOutcome {
    Closed,
    Handled {
        used: &'static str,
        send: Vec<Command>,
    },
}

fn handled(used: &'static str, send: Vec<Command>) -> ProjectsOutcome {
    ProjectsOutcome::Handled { used, send }
}

impl Projects {
    pub(crate) fn key(&mut self, key: Key, board: &Board) -> ProjectsOutcome {
        if let Some(project) = self.confirming.take() {
            return match key {
                Key::Char('y') => {
                    handled("project-remove", vec![Command::ProjectRemove { project }])
                }
                _ => handled("project-remove-cancel", Vec::new()),
            };
        }
        let count = board.projects().len();
        match key {
            Key::Esc | Key::Char('q') => ProjectsOutcome::Closed,
            Key::Down | Key::Char('j') if count > 0 => {
                self.selected = (self.selected(count) + 1) % count;
                handled("project-move", Vec::new())
            }
            Key::Up | Key::Char('k') if count > 0 => {
                self.selected = (self.selected(count) + count - 1) % count;
                handled("project-move", Vec::new())
            }
            Key::Char('d') if count > 0 => {
                self.confirming = Some(board.projects()[self.selected(count)].name.clone());
                handled("project-remove-ask", Vec::new())
            }
            _ => handled("unbound", Vec::new()),
        }
    }

    /// The selection, kept within a list that shrank since it was made.
    fn selected(&self, count: usize) -> usize {
        self.selected.min(count.saturating_sub(1))
    }

    /// The status line's question while a removal waits for `y`.
    pub(crate) fn confirm_line(&self) -> Option<String> {
        self.confirming
            .as_ref()
            .map(|project| format!("remove project {project}? y to confirm"))
    }

    pub(crate) fn draw(&self, board: &Board, frame: &mut Frame<'_>, area: Rect) {
        let projects = board.projects();
        let mut lines = vec![
            "d removes the selected project (its files, clones and history stay); Esc back"
                .to_owned(),
        ];
        if projects.is_empty() {
            lines.push("No registered projects".to_owned());
        }
        let selected = self.selected(projects.len());
        lines.extend(projects.iter().enumerate().map(|(index, project)| {
            let marker = if index == selected { ">" } else { " " };
            format!("{marker} {}  {}", project.name, project.root)
        }));
        frame.render_widget(
            Paragraph::new(lines.join("\n"))
                .block(Block::default().borders(Borders::ALL).title("Projects")),
            area,
        );
    }
}
