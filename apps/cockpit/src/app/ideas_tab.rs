use super::*;

impl App {
    pub(super) fn build_ideas(&mut self) {
        let selected = self
            .idea_entries
            .get(self.panels[4].selected)
            .map(todo::Item::id);
        let mut entries: Vec<_> = self
            .todos
            .items
            .iter()
            .filter(|item| item.status() == todo::PROPOSED)
            .cloned()
            .collect();
        entries.sort_by_key(|item| (-item.priority(), item.created()));
        let mut lines = Vec::new();
        if entries.is_empty() {
            lines.push(Line::new(vec![segment(
                0,
                "No ideas waiting for your approval.",
                dim(FG_DARK),
            )]));
        }
        for (index, item) in entries.iter().enumerate() {
            let category = item
                .category()
                .map_or("Unknown kind", |category| category.label());
            let reason = if item.why().is_empty() {
                "No reason provided".to_string()
            } else {
                util::collapse(&item.why())
            };
            for (text, tint) in [
                (format!("? {}", item.text()), bold(MAGENTA)),
                (
                    format!("{} · {category}", item.project()),
                    bold(project_color(&item.project())),
                ),
                (self.idea_source(item), style(FG_DARK)),
                (format!("Why: {reason}"), style(FG)),
            ] {
                lines.push(Line {
                    segments: vec![segment(0, text, tint)],
                    item: Some(index),
                    ..Line::default()
                });
            }
            lines.push(Line::default());
        }
        let panel = &mut self.panels[4];
        panel.items = entries.len();
        panel.selected = selected
            .and_then(|id| entries.iter().position(|item| item.id() == id))
            .unwrap_or(panel.selected);
        panel.clamp();
        panel.lines = lines;
        panel.fixed_top = vec![Line::new(vec![segment(
            0,
            "y Approve  n Decline  Enter Details",
            dim(FG_DARK),
        )])];
        self.idea_entries = entries;
    }

    pub(super) fn idea_source(&self, item: &todo::Item) -> String {
        let source = item.source();
        let Some(identity) = source
            .strip_prefix("session ")
            .filter(|identity| !identity.is_empty())
        else {
            return "Unknown session".into();
        };
        match self
            .all_sessions()
            .iter()
            .find(|session| session.session == identity)
        {
            Some(session) => {
                let label = if session.label.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", session.label)
                };
                format!("From {}{label} · {source}", session.project)
            }
            None => format!("{source} · not running"),
        }
    }

    pub(super) fn handle_ideas(&mut self, key: Key) -> String {
        let Some(item) = self.idea_entries.get(self.panels[4].selected).cloned() else {
            return String::new();
        };
        match key {
            Key::Char('y') | Key::Char('n') => {
                let approved = key == Key::Char('y');
                if approved {
                    self.todos.approve(&item.id());
                } else {
                    self.todos.decline(&item.id());
                }
                self.build_ideas();
                self.build_todo();
                if approved {
                    "approved: it is in To do now".into()
                } else {
                    "declined: it will not be proposed again".into()
                }
            }
            Key::Enter | Key::Char('i') => {
                self.session_details = Some(SessionDetails {
                    title: "Idea".into(),
                    text: format!(
                        "{}\n\n{}",
                        self.idea_source(&item),
                        todo_details(&Entry::Typed(item))
                    ),
                    scroll: 0,
                    actions: Vec::new(),
                    chief_answers_of: None,
                    ..Default::default()
                });
                String::new()
            }
            _ => String::new(),
        }
    }
}
