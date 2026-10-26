//! `N`: a blank session from a form (frame 1707): project (recent first, typing filters), agent,
//! first message, and whether it gets its own clone. Most work starts from a todo instead (`r`).

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field { Project, Agent, Message, Clone }

const FIELDS: [Field; 4] = [Field::Project, Field::Agent, Field::Message, Field::Clone];
const VISIBLE_PROJECTS: usize = 6;

pub(crate) struct NewSession {
    projects: Vec<String>,
    filter: String,
    selected: usize,
    agent: usize,
    message: String,
    own_clone: bool,
    field: Field,
}

impl NewSession {
    pub(crate) fn new(projects: Vec<String>, default_agent: &str) -> Self {
        let agent = agents::AGENTS.iter().position(|agent| agent.name == default_agent).unwrap_or(0);
        Self { projects, filter: String::new(), selected: 0, agent, message: String::new(), own_clone: true, field: Field::Project }
    }

    fn matching(&self) -> Vec<&String> {
        let filter = self.filter.to_lowercase();
        self.projects.iter().filter(|project| project_name(project).to_lowercase().contains(&filter)).collect()
    }

    fn move_field(&mut self, step: isize) {
        let index = FIELDS.iter().position(|field| *field == self.field).unwrap() as isize;
        self.field = FIELDS[(index + step).rem_euclid(FIELDS.len() as isize) as usize];
    }

    pub(crate) fn draw(&self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        if width < 20 || height < 18 { return; }
        let (top, bottom) = (HEADER_LINES, height - 2);
        let inner = width.saturating_sub(4) as usize;
        let body = inner.saturating_sub(2);
        for row in top..=bottom {
            put(buffer, row, 1, &" ".repeat((width - 2) as usize), on(FG, BLACK));
            put(buffer, row, 1, "│", style(BLUE));
            put(buffer, row, width - 2, "│", style(BLUE));
        }
        put(buffer, top, 1, &format!("┌{}┐", "─".repeat(inner)), style(BLUE));
        put(buffer, bottom, 1, &format!("└{}┘", "─".repeat(inner)), style(BLUE));
        let heading = |field: Field| if self.field == field { bold(BLUE) } else { bold(FG_DARK) };
        let mut row = top + 1;
        put(buffer, row, 3, "New session", bold(FG));
        for line in approval::wrap("Most work starts from a todo: Todo tab → r run now. This is for a blank session.", body) {
            row += 1;
            put(buffer, row, 3, &line, dim(FG_DARK));
        }
        row += 2;
        put(buffer, row, 3, "PROJECT  (recent first)", heading(Field::Project));
        let matching = self.matching();
        let first = self.selected.saturating_sub(VISIBLE_PROJECTS - 1);
        for (index, project) in matching.iter().enumerate().skip(first).take(VISIBLE_PROJECTS) {
            row += 1;
            if index == self.selected {
                put(buffer, row, 3, &format!("{:<body$}", format!("▶ {}", project_name(project))), on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD));
            } else {
                put(buffer, row, 3, &format!("  {}", project_name(project)), style(FG));
            }
        }
        row += 1;
        let filter = if self.filter.is_empty() { "  … type to filter".to_string() }
            else if matching.is_empty() { format!("  no project matches “{}”", self.filter) } else { format!("  filter: {}", self.filter) };
        put(buffer, row, 3, &filter, dim(FG_DARK));
        row += 2;
        put(buffer, row, 3, "AGENT", heading(Field::Agent));
        row += 1;
        let mut column = 3;
        for (index, agent) in agents::AGENTS.iter().enumerate() {
            let choice = format!("({}) {}", if index == self.agent { "●" } else { " " }, agent.label);
            put(buffer, row, column, &choice, if index == self.agent { bold(FG) } else { style(FG_DARK) });
            column += width_of(&choice) as u16 + 4;
        }
        row += 2;
        put(buffer, row, 3, "FIRST MESSAGE", heading(Field::Message));
        row += 1;
        let cursor = if self.field == Field::Message { "▏" } else { "" };
        let shown = format!("{}{cursor}", self.message);
        let tail: String = shown.chars().rev().take(body.saturating_sub(2)).collect::<Vec<_>>().into_iter().rev().collect();
        put(buffer, row, 3, &format!("{tail:<width$}", width = body.saturating_sub(2)), on(FG, BG_HIGHLIGHT));
        row += 2;
        put(buffer, row, 3, &format!("[{}] in its own clone (recommended)", if self.own_clone { "x" } else { " " }), heading(Field::Clone));
        put(buffer, bottom - 1, 3, &fit("Tab next field   ⌘S start   esc cancel", body), dim(FG_DARK));
    }
}

impl App {
    pub(super) fn open_new_session(&mut self) {
        let default_agent = settings::launch_agent().unwrap_or_else(|_| agents::AGENTS[0].name.to_string());
        self.new_session = Some(NewSession::new(sessions::project_choices(), &default_agent));
    }

    pub(super) fn handle_new_session(&mut self, key: Key) -> String {
        let Some(form) = self.new_session.as_mut() else { return String::new() };
        match (key, form.field) {
            (Key::Esc, _) => { self.new_session = None; }
            (Key::Ctrl('s'), _) => return self.start_new_session(),
            (Key::Tab | Key::Enter, _) => form.move_field(1),
            (Key::BackTab, _) => form.move_field(-1),
            (Key::Down, Field::Project) => form.selected = (form.selected + 1).min(form.matching().len().saturating_sub(1)),
            (Key::Up, Field::Project) => form.selected = form.selected.saturating_sub(1),
            (Key::Backspace, Field::Project) => { form.filter.pop(); form.selected = 0; }
            (Key::Char(character), Field::Project) => { form.filter.push(character); form.selected = 0; }
            (Key::Left, Field::Agent) => form.agent = (form.agent + agents::AGENTS.len() - 1) % agents::AGENTS.len(),
            (Key::Right | Key::Char(' '), Field::Agent) => form.agent = (form.agent + 1) % agents::AGENTS.len(),
            (Key::Backspace, Field::Message) => { form.message.pop(); }
            (Key::Char(character), Field::Message) => form.message.push(character),
            (Key::Paste(text), Field::Message) => form.message.push_str(&util::collapse(&text)),
            (Key::Char(' '), Field::Clone) => form.own_clone = !form.own_clone,
            _ => {}
        }
        String::new()
    }

    fn start_new_session(&mut self) -> String {
        let form = self.new_session.as_ref().unwrap();
        let Some(directory) = form.matching().get(form.selected).map(|project| project.to_string()) else {
            return "no project matches: change the filter".into();
        };
        let agent = agents::AGENTS[form.agent].name;
        let message = form.message.trim().to_string();
        let own_clone = form.own_clone;
        self.new_session = None;
        if own_clone {
            let branch = chrono::Local::now().format("parallel-%H%M").to_string();
            let mut arguments = vec!["--agent", agent, "parallel", &directory, &branch];
            if !message.is_empty() { arguments.push(&message); }
            manager_in_background(&arguments);
            return format!("cloning {} for {branch} …", project_name(&directory));
        }
        let mut arguments = vec!["--agent", agent, "new", &directory];
        if !message.is_empty() { arguments.push(&message); }
        self.manager_async(&arguments)
    }
}
