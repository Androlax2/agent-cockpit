//! Todo panel construction, actions and composer coordination.

use super::*;

/// The Todo panel's tabs: the statuses each one shows. Open merges everything not done yet.
// Proposals live in the Ideas tab: Open shows only work Theo agreed to.
pub(super) const TODO_TABS: [(&str, &[&str]); 5] = [
    ("Open", &["todo", "doing"]),
    ("Proposed", &[todo::PROPOSED]),
    ("To do", &["todo"]),
    ("In progress", &["doing"]),
    ("Done", &["done"]),
];
pub(super) const TODO_TAB_OPEN: usize = 0;
pub(super) const TODO_TAB_PROPOSED: usize = 1;
pub(super) const TODO_TAB_TO_DO: usize = 2;
pub(super) const TODO_TAB_IN_PROGRESS: usize = 3;

impl App {
    pub(super) fn build_todo(&mut self) {
        // Like the issues: the viewed session's project only, every project when none is viewed.
        let viewed = current_session();
        if self
            .todo_jump
            .as_ref()
            .is_some_and(|(source, _)| *source != viewed)
        {
            self.todo_jump = None;
        }
        if self
            .saved_todo_scope
            .as_ref()
            .is_some_and(|scope| scope.viewed_session != viewed)
        {
            self.saved_todo_scope = None;
        }
        self.todo_scope = self
            .todo_jump
            .as_ref()
            .and_then(|(_, key)| {
                self.todos
                    .items
                    .iter()
                    .find(|item| item.id() == *key)
                    .map(|item| item.project())
            })
            .or_else(|| {
                self.saved_todo_scope
                    .as_ref()
                    .map(|scope| scope.project.clone())
            })
            .or_else(|| {
                self.running_sessions
                    .iter()
                    .find(|s| s.session == viewed)
                    .map(|s| s.project.clone())
            })
            .unwrap_or_default();
        self.todo_tab = self
            .todo_tabs
            .get(&self.todo_owner())
            .copied()
            .unwrap_or(TODO_TAB_OPEN);
        let scope = self.todo_scope.clone();
        let mut entries: Vec<Entry> = self
            .todos
            .items
            .iter()
            .filter(|item| scope.is_empty() || item.project() == scope)
            .map(|item| Entry::Typed(item.clone()))
            .collect();
        for issue in self.reviews.issues() {
            if self.saved_todo_scope.is_some() && issue.repository != scope {
                continue;
            }
            let status = self.todos.status_of(&issue.key());
            let priority = todo::issue_priority(&issue.labels);
            entries.push(Entry::Issue {
                issue,
                status,
                priority,
            });
        }
        entries.sort_by(todo::todo_order);
        if let Some(category) = self.todo_category_filter() {
            entries.retain(
                |entry| matches!(entry, Entry::Typed(item) if item.category() == Some(category)),
            );
        }
        for (index, (_, statuses)) in TODO_TABS.iter().enumerate() {
            self.todo_counts[index] = entries
                .iter()
                .filter(|entry| statuses.contains(&entry.status().as_str()))
                .count();
        }
        let shown_statuses = TODO_TABS[self.todo_tab].1;
        entries.retain(|entry| shown_statuses.contains(&entry.status().as_str()));
        if self.todo_tab == TODO_TAB_OPEN {
            entries.sort_by_key(|entry| match entry.status().as_str() {
                "doing" => 0,
                todo::PROPOSED => 1,
                _ => 2,
            });
        }
        let mut lines = Vec::new();
        if entries.is_empty() {
            let empty = if self.todo_category_filter().is_some() {
                "Nothing here matches the kind filter. f changes it; [ ] switch tabs."
            } else {
                match shown_statuses {
                    [_, _, ..] => "Nothing open. ] next tab.",
                    [todo::PROPOSED] => "No idea from Claude waiting for you. ] next tab.",
                    ["doing"] => "Nothing in progress. ] next tab.",
                    ["done"] => "Nothing done in the last 7 days.",
                    _ => "Nothing to do. t adds an item for the viewed project; [ ] switch tabs.",
                }
            };
            lines.push(Line::new(vec![segment(0, empty, dim(FG_DARK))]));
        }
        for (index, entry) in entries.iter().enumerate() {
            let status = entry.status();
            if self.todo_tab == TODO_TAB_OPEN
                && (index == 0 || entries[index - 1].status() != status)
            {
                let heading = match status.as_str() {
                    "doing" => "In progress",
                    todo::PROPOSED => "Proposed",
                    _ => "To do",
                };
                lines.push(Line::new(vec![segment(0, heading, bold(FG_DARK))]));
            }
            let done = status == "done";
            let mark = if done {
                String::new()
            } else {
                todo::priority_mark(entry.priority())
            };
            let mark_style = if entry.priority() > todo::PRIORITY_NORMAL {
                bold(RED)
            } else {
                dim(FG_DARK)
            };
            let start = 19;
            let (glyph, glyph_style) = match status.as_str() {
                "doing" => ("◐", bold(BLUE)),
                todo::PROPOSED => ("?", bold(MAGENTA)),
                "done" => ("✓", dim(GREEN)),
                _ => ("•", bold(YELLOW)),
            };
            let faded = done.then(|| dim(FG_DARK));
            let queued = status == "todo" && self.todos.is_queued(&entry.key());
            let mut right = if queued {
                vec![part("Q", bold(CYAN))]
            } else {
                vec![]
            };
            if let Entry::Typed(item) = entry {
                if item.is_design() {
                    right = vec![part(
                        if queued { "✎Q" } else { "✎" },
                        faded.unwrap_or(bold(MAGENTA)),
                    )];
                }
                if item.is_blocked_report() {
                    right = vec![part(
                        if queued { "⛔Q" } else { "⛔" },
                        faded.unwrap_or(bold(RED)),
                    )];
                }
                if !item.link().is_empty() {
                    right.insert(0, part("↔", faded.unwrap_or(bold(CYAN))));
                }
                if item.is_waiting() && !done {
                    right = vec![part("waits", bold(YELLOW))];
                }
            }
            let line = match entry {
                Entry::Typed(item) => Line {
                    segments: vec![
                        segment(0, glyph, glyph_style),
                        segment(2, &mark, mark_style),
                        segment(
                            7,
                            match item.category() {
                                Some(todo::Category::Task) => "",
                                Some(category) => category.label(),
                                None => "Unknown ?",
                            },
                            faded.unwrap_or(style(
                                if item.category() == Some(todo::Category::Bug) {
                                    RED
                                } else {
                                    FG_DARK
                                },
                            )),
                        ),
                        segment(
                            start,
                            if scope.is_empty() {
                                format!("{} {}", fit(&item.project(), 9), item.text())
                            } else {
                                item.text()
                            },
                            faded.unwrap_or(style(FG)),
                        ),
                    ],
                    right,
                    item: Some(index),
                    accent: None,
                    fill: None,
                },
                Entry::Issue { issue, .. } => {
                    let label = format!("{}#{}", issue.repository, issue.number);
                    let labels = issue
                        .labels
                        .iter()
                        .take(2)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    let (glyph, glyph_style) = if status == "todo" {
                        ("◇", style(MAGENTA))
                    } else {
                        (glyph, glyph_style)
                    };
                    let right = if !right.is_empty() {
                        right
                    } else if !labels.is_empty() {
                        vec![part(labels, dim(FG_DARK))]
                    } else {
                        vec![]
                    };
                    let right = if queued && status == "todo" {
                        vec![part("Q", bold(CYAN))]
                    } else {
                        right
                    };
                    Line {
                        segments: vec![
                            segment(0, glyph, glyph_style),
                            segment(2, &mark, mark_style),
                            segment(7, &label, faded.unwrap_or(bold(MAGENTA))),
                            segment(
                                8 + width_of(&label) as u16,
                                &issue.title,
                                faded.unwrap_or(style(FG_DARK)),
                            ),
                        ],
                        right,
                        item: Some(index),
                        accent: None,
                        fill: None,
                    }
                }
            };
            lines.push(line);
        }
        self.todo_entries = entries;
        let panel = &mut self.panels[3];
        panel.items = self.todo_entries.len();
        panel.clamp();
        panel.lines = lines;
    }

    pub(super) fn todo_fixed_lines(&mut self, height: u16) {
        let mut tabs = Vec::new();
        let mut column = 0;
        if let Some(scope) = &self.saved_todo_scope {
            let active_width = width_of(&format!(
                " {} {} ",
                TODO_TABS[self.todo_tab].0, self.todo_counts[self.todo_tab]
            ));
            let project_room = (self.width as usize).saturating_sub(4 + 10 + active_width + 1);
            let context = format!("saved: {} · ", fit(&scope.project, project_room));
            column = width_of(&context) as u16;
            tabs.push(segment(0, context, bold(CYAN)));
        }
        for index in std::iter::once(self.todo_tab)
            .chain((0..TODO_TABS.len()).filter(|index| *index != self.todo_tab))
        {
            let text = format!(" {} {} ", TODO_TABS[index].0, self.todo_counts[index]);
            let tint = if index == self.todo_tab {
                on(BLACK, BLUE)
            } else if index == TODO_TAB_PROPOSED && self.todo_counts[index] > 0 {
                on(BLACK, MAGENTA)
            } else {
                dim(FG_DARK)
            };
            tabs.push(segment(column, &text, tint));
            column += width_of(&text) as u16 + 1;
        }
        self.panels[3].fixed_top = vec![
            Line::new(tabs),
            Line::new(vec![segment(0, self.todo_subtitle(), dim(FG_DARK))]),
        ];
        self.panels[3].fixed_bottom.clear();
        if self.focus != 3 || height < 11 {
            return;
        }
        let Some(entry) = self.todo_entries.get(self.panels[3].selected) else {
            return;
        };
        let (title, context, notes) = match entry {
            Entry::Typed(item) => (
                item.text(),
                todo_context(item),
                item.notes()
                    .lines()
                    .find(|line| !line.trim().is_empty())
                    .unwrap_or("No notes · e Edit · Enter Details")
                    .to_string(),
            ),
            Entry::Issue { issue, status, .. } => (
                issue.title.clone(),
                format!("GitHub · {status} · {}#{}", issue.repository, issue.number),
                issue.url.clone(),
            ),
        };
        self.panels[3].fixed_bottom = vec![
            Line::new(vec![segment(0, title, bold(FG))]),
            Line::new(vec![segment(0, context, dim(FG_DARK))]),
            Line::new(vec![segment(0, notes, dim(FG_DARK))]),
        ];
    }

    /// Keep the filter ahead of the scope: long project names must not hide it.
    pub(super) fn todo_subtitle(&self) -> String {
        let scope = if self.todo_scope.is_empty() {
            "all projects"
        } else {
            self.todo_scope.as_str()
        };
        let queued = self.todos.queued.len();
        let queue = if queued > 0 {
            format!(" · {queued} queued")
        } else {
            String::new()
        };
        let category = self
            .todo_category_filter()
            .map(|category| format!("kind: {} · ", category.label()))
            .unwrap_or_default();
        let saved = if self.saved_todo_scope.is_some() {
            "saved: "
        } else {
            ""
        };
        format!("{category}{saved}{scope}{queue}")
    }

    fn todo_owner(&self) -> String {
        let viewed = current_session();
        self.saved_todo_scope
            .as_ref()
            .filter(|scope| scope.viewed_session == viewed)
            .map_or(viewed, |scope| scope.owner_session.clone())
    }

    fn todo_category_filter(&self) -> Option<todo::Category> {
        self.todo_categories
            .get(&self.todo_owner())
            .copied()
            .flatten()
    }

    fn reveal_todo(&mut self, id: &str) {
        if let Some(category) = self.todo_category_filter()
            && self
                .todos
                .items
                .iter()
                .any(|item| item.id() == id && item.category() != Some(category))
        {
            self.todo_categories.insert(self.todo_owner(), None);
        }
        self.build_todo();
        if let Some(index) = self.todo_entries.iter().position(|entry| entry.key() == id) {
            self.panels[3].selected = index;
        }
    }

    pub(super) fn show_todo_tab(&mut self, tab: usize) {
        self.todo_tab = tab;
        self.todo_tabs.insert(self.todo_owner(), tab);
    }

    fn switch_todo_tab(&mut self, step: isize) {
        self.show_todo_tab(
            (self.todo_tab as isize + step).rem_euclid(TODO_TABS.len() as isize) as usize,
        );
        self.panels[3].selected = 0;
        self.build_todo();
    }

    pub(crate) fn handle_todo(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        let entry = self.todo_entries.get(self.panels[3].selected).cloned();
        let viewed = current_session();
        match key {
            Key::Char('T') => self.run_continuity_action("task-trail", screen),
            Key::Char('i') => {
                if let Some(entry) = entry {
                    self.session_details = Some(SessionDetails {
                        title: "Todo details".into(),
                        text: todo_details(&entry),
                        todo: Some(crate::continuity::task_key(&entry)),
                        key_groups: vec![(
                            "ACTIONS",
                            vec![("T", "task trail"), ("i", "in progress / back to do")],
                        )],
                        ..Default::default()
                    });
                }
                String::new()
            }
            Key::Esc if self.saved_todo_scope.is_some() => {
                self.saved_todo_scope = None;
                self.build_todo();
                String::new()
            }
            Key::Char('t') => {
                let viewed = self
                    .running_sessions
                    .iter()
                    .find(|s| s.session == viewed && !s.cwd.is_empty())
                    .cloned();
                let directory = match &viewed {
                    Some(session) => session.cwd.clone(),
                    None => screen.outside(&mut || pick_project()).unwrap_or_default(),
                };
                if directory.is_empty() {
                    return String::new();
                }
                let project = viewed
                    .map(|s| s.project)
                    .unwrap_or_else(|| project_name(&directory));
                match TodoComposer::new(project, directory, current_session(), None) {
                    Ok(composer) => self.todo_composer = Some(composer),
                    Err(error) => return format!("cannot protect the TODO draft: {error}"),
                }
                String::new()
            }
            Key::Char('[') => {
                self.switch_todo_tab(-1);
                String::new()
            }
            Key::Char(']') => {
                self.switch_todo_tab(1);
                String::new()
            }
            Key::Char('e') => match entry {
                Some(Entry::Typed(item)) => {
                    match TodoComposer::new(
                        item.project(),
                        item.cwd(),
                        self.todo_owner(),
                        Some(&item),
                    ) {
                        Ok(composer) => self.todo_composer = Some(composer),
                        Err(error) => return format!("cannot protect the TODO draft: {error}"),
                    }
                    String::new()
                }
                Some(Entry::Issue { .. }) => "an issue is edited on GitHub".into(),
                None => String::new(),
            },
            Key::Char('K') => match entry {
                Some(Entry::Typed(item)) => {
                    let Some(chosen) =
                        screen.outside(&mut || pick_todo_kind(item.category(), false))
                    else {
                        return "unchanged".into();
                    };
                    let Some(category) = todo::Category::parse(&chosen.to_lowercase()) else {
                        return "unchanged: invalid kind".into();
                    };
                    self.todos.set_category(&item.id(), category);
                    self.reveal_todo(&item.id());
                    format!("kind: {}", category.label())
                }
                Some(Entry::Issue { .. }) => "an issue's labels are edited on GitHub".into(),
                None => String::new(),
            },
            Key::Char('m') => match entry {
                Some(Entry::Typed(item)) => {
                    let Some(directory) = screen
                        .outside(&mut || pick_project())
                        .filter(|directory| !directory.is_empty())
                    else {
                        return "unchanged".into();
                    };
                    let project = project_name(&directory);
                    match self.todos.move_to(&item.id(), &project, &directory) {
                        Ok(()) => {
                            self.build_todo();
                            format!("moved to {project}")
                        }
                        Err(reason) => format!("not moved: {reason}"),
                    }
                }
                Some(Entry::Issue { .. }) => "an issue belongs to its repository".into(),
                None => String::new(),
            },
            Key::Char('f') => {
                let Some(chosen) =
                    screen.outside(&mut || pick_todo_kind(self.todo_category_filter(), true))
                else {
                    return String::new();
                };
                let category = if chosen == "All" {
                    None
                } else {
                    let Some(category) = todo::Category::parse(&chosen.to_lowercase()) else {
                        return "invalid kind filter".into();
                    };
                    Some(category)
                };
                self.todo_categories.insert(self.todo_owner(), category);
                self.panels[3].selected = 0;
                self.build_todo();
                String::new()
            }
            Key::Char('y') => match entry {
                Some(Entry::Typed(item)) if item.status() == todo::PROPOSED => {
                    self.todos.approve(&item.id());
                    "approved: it is in To do now".into()
                }
                _ => "y approves a proposal (Proposed tab)".into(),
            },
            Key::Char('n') => match entry {
                Some(Entry::Typed(item)) if item.status() == todo::PROPOSED => {
                    self.todos.decline(&item.id());
                    "declined: Claude will not propose it again".into()
                }
                _ => "n declines a proposal (Proposed tab)".into(),
            },
            Key::Char('h') => match entry {
                Some(entry) if entry.status() == todo::PROPOSED => "approve it first (y)".into(),
                Some(entry) => self.hand_to_claude(&entry, screen),
                None => String::new(),
            },
            Key::Char('r') => match entry {
                Some(entry) if entry.status() == todo::PROPOSED => "approve it first (y)".into(),
                Some(entry) => self.hand_to_new_session(&entry, screen),
                None => String::new(),
            },
            Key::Char('x') => match entry {
                Some(Entry::Typed(item)) => {
                    let token = self.todos.manual_reservation(&item.id());
                    if let Err(error) = self.todos.remove(&item.id()) {
                        return format!("cannot remove TODO: {error}");
                    }
                    if let Err(error) =
                        queue::release_manual(&item.cwd(), &item.id(), token.as_deref())
                    {
                        return error;
                    }
                    "removed".into()
                }
                _ => String::new(),
            },
            Key::Char('d') => entry
                .map(|entry| self.toggle_todo_status(&entry, "done"))
                .unwrap_or_default(),
            Key::Char(pressed @ ('+' | '-')) => match entry {
                Some(Entry::Typed(item)) => {
                    self.todos
                        .change_priority(&item.id(), if pressed == '+' { 1 } else { -1 });
                    self.build_todo();
                    if let Some(index) = self.todo_entries.iter().position(|e| e.key() == item.id())
                    {
                        self.panels[3].selected = index;
                    }
                    String::new()
                }
                Some(Entry::Issue { .. }) => {
                    "an issue's priority comes from its labels on GitHub".into()
                }
                None => String::new(),
            },
            Key::Enter => self.handle_todo(Key::Char('i'), screen),
            Key::Char('Q') => match entry {
                Some(entry) if entry.status() == "todo" => {
                    if self.todos.toggle_queued(&entry.key()) {
                        "queued: an idle session of its project takes it up".into()
                    } else {
                        "out of the queue".into()
                    }
                }
                Some(_) => "only items still to do can be queued".into(),
                None => String::new(),
            },
            _ => String::new(),
        }
    }

    pub(super) fn toggle_todo_status(&mut self, entry: &Entry, target: &str) -> String {
        let status = if self.todos.status_of(&entry.key()) == target {
            "todo"
        } else {
            target
        };
        let directory = match entry {
            Entry::Typed(item) => item.cwd(),
            Entry::Issue { issue, .. } => self
                .reviews
                .checkouts
                .get(&issue.full_repository)
                .cloned()
                .unwrap_or_default(),
        };
        let mut token = self.todos.manual_reservation(&entry.key());
        if status == "doing" {
            token = Some(match queue::reserve_manual(&directory, &entry.key()) {
                Ok(token) => token,
                Err(error) => return error,
            });
            self.todos.record_manual_reservation(
                &entry.key(),
                token.as_deref().expect("reservation obtained"),
            );
        }
        if !self.todos.set_status(&entry.key(), status, "") {
            if status == "doing"
                && let Err(error) =
                    queue::release_manual(&directory, &entry.key(), token.as_deref())
            {
                return error;
            }
            return "cannot mark TODO; check the log".into();
        }
        if status == "doing" {
            self.todos.record_manual_reservation(
                &entry.key(),
                token.as_deref().expect("reservation obtained"),
            );
        }
        if status != "doing"
            && let Err(error) = queue::release_manual(&directory, &entry.key(), token.as_deref())
        {
            return error;
        }
        match status {
            "doing" => "in progress",
            "done" => "done",
            _ => "back to do",
        }
        .into()
    }

    /// The item to the project's running session, or to a new one; then it is in progress.
    pub fn hand_to_claude(&mut self, entry: &Entry, _screen: &mut dyn Screen) -> String {
        let (directory, text, repository) = match entry {
            Entry::Typed(item) => (item.cwd(), todo::prompt_of(entry), String::new()),
            Entry::Issue { issue, .. } => (
                self.reviews
                    .checkouts
                    .get(&issue.full_repository)
                    .cloned()
                    .unwrap_or_default(),
                todo::prompt_of(entry),
                issue.repository.clone(),
            ),
        };
        if directory.is_empty() {
            return format!(
                "no local checkout of {}: start a session there first",
                if repository.is_empty() {
                    "?"
                } else {
                    &repository
                }
            );
        }
        let directory = match start_directory(&directory) {
            Ok(directory) => directory,
            Err(error) => return error,
        };
        let key = entry.key();
        let text = todo::with_todo_note(&text, &key);
        // A design to-do goes to the project's designer session, wherever its checkout is; any
        // other item to the session on screen when it works on the item's project (a parallel
        // clone's session lives in another folder than the one the item was filed in), else to a
        // session of its folder that is not the designer.
        let design_project = match entry {
            Entry::Typed(item) if item.is_design() => Some(item.project()),
            _ => None,
        };
        let is_design = design_project.is_some();
        let mut candidates = self.running_sessions.iter().filter(|session| {
            !self.qa_sessions.contains(&session.session)
                && session.state != "unknown"
                && match &design_project {
                    Some(project) => session.role == "designer" && session.project == *project,
                    None => session.cwd == directory && session.role != "designer",
                }
        });
        let viewed = if is_design {
            None
        } else {
            self.viewed_session_for(entry)
        };
        let running = viewed.or_else(|| {
            candidates
                .clone()
                .find(|session| !session.is_external())
                .cloned()
        });
        if running.is_none() && candidates.any(|session| session.is_external()) {
            return "external session: no terminal target; use its own window".into();
        };
        let text = if is_design && running.is_none() {
            format!("Load the figcraft-design skill first. {text}")
        } else {
            text
        };
        let agent = if running.is_none() {
            match settings::launch_agent() {
                Ok(agent) => Some(agent),
                Err(error) => return error,
            }
        } else {
            None
        };
        let token = match queue::reserve_task(
            &directory,
            &key,
            running.as_ref().map(|session| session.session.as_str()),
            false,
        ) {
            Ok(token) => token,
            Err(error) => return error,
        };
        let outcome = match &running {
            Some(session) => {
                if let Err(error) =
                    queue::launch_reserved(&token, &["say", &session.session, &text], false)
                {
                    return error;
                }
                format!("sent to {}", session.project)
            }
            None => {
                let agent = agent.expect("launch agent validated before reservation");
                // Out of sight, as a card of its own (cockpit background): in the session pane
                // it replaced the session on screen, and the Todo panel lost its project.
                if let Err(error) = queue::launch_reserved(
                    &token,
                    &["--agent", &agent, "background", &directory, &text],
                    true,
                ) {
                    return error;
                }
                format!("starting a session in {}", project_name(&directory))
            }
        };
        if !self.todos.set_status(
            &key,
            "doing",
            running.as_ref().map(|s| s.session.as_str()).unwrap_or(""),
        ) {
            return format!("{outcome}; cannot save TODO status");
        }
        if let Err(error) = self.record_task_assignment(entry, running.as_ref()) {
            self.continuity_error = Some(error.clone());
            return format!("{outcome}; task trail unavailable: {error}");
        }
        outcome
    }

    /// The item to a new session in a parallel clone of its repository, on a branch named after it
    /// (cockpit parallel), never to a running one: it does not touch the files a session
    /// already works on. Then it is in progress.
    pub fn hand_to_new_session(&mut self, entry: &Entry, _screen: &mut dyn Screen) -> String {
        let directory = match entry {
            Entry::Typed(item) => item.cwd(),
            Entry::Issue { issue, .. } => self
                .reviews
                .checkouts
                .get(&issue.full_repository)
                .cloned()
                .unwrap_or_default(),
        };
        if directory.is_empty() {
            return "no local checkout of this issue's repository: clone it first".into();
        }
        let directory = match start_directory(&directory) {
            Ok(directory) => directory,
            Err(error) => return error,
        };
        let agent = match settings::launch_agent() {
            Ok(agent) => agent,
            Err(error) => return error,
        };
        let key = entry.key();
        let (branch, text) = todo::handoff_of(entry);
        let token = match queue::reserve_task(&directory, &key, None, false) {
            Ok(token) => token,
            Err(error) => return error,
        };
        if let Err(error) = queue::launch_reserved(
            &token,
            &["--agent", &agent, "parallel", &directory, &branch, &text],
            true,
        ) {
            return error;
        }
        if !self.todos.set_status(&key, "doing", "") {
            return "starting a new session; cannot save TODO status".into();
        }
        if let Err(error) = self.record_task_assignment(entry, None) {
            self.continuity_error = Some(error);
        }
        format!(
            "starting a new session in a clone of {} (branch {branch})",
            project_name(&directory)
        )
    }

    /// The session in the session pane, when it can take this item: running, not the designer, and
    /// on the item's project (by name, as the Todo list is narrowed) or the issue's repository.
    fn viewed_session_for(&self, entry: &Entry) -> Option<Session> {
        let viewed = current_session();
        let session = self.running_sessions.iter().find(|s| {
            !s.is_external()
                && !self.qa_sessions.contains(&s.session)
                && s.session == viewed
                && s.state != "unknown"
                && s.role != "designer"
        })?;
        let belongs = match entry {
            Entry::Typed(item) => item.project() == session.project,
            Entry::Issue { issue, .. } => {
                self.reviews.repositories.contains(&issue.full_repository)
            }
        };
        belongs.then(|| session.clone())
    }

    /// Each tab's keys, the one that moves its items along first.
    /// In the Open tab, the selected item's status picks them.
    pub(crate) fn todo_hints(&self) -> Vec<(&'static str, &'static str)> {
        let selected_status = self
            .todo_entries
            .get(self.panels[3].selected)
            .map(Entry::status);
        let status = match TODO_TABS[self.todo_tab].1 {
            [only] => *only,
            _ => selected_status.as_deref().unwrap_or("todo"),
        };
        let mut hints = match status {
            todo::PROPOSED => vec![
                ("y", "approve"),
                ("n", "decline"),
                ("⏎", "details"),
                ("e", "edit"),
                ("[ ]", "tabs"),
            ],
            "doing" => vec![
                ("d", "done"),
                ("i", "details"),
                ("T", "trail"),
                ("e", "edit"),
                ("t", "add"),
                ("[ ]", "tabs"),
            ],
            "done" => vec![
                ("⏎", "details"),
                ("d", "back to to do"),
                ("t", "add"),
                ("[ ]", "tabs"),
            ],
            _ => vec![
                ("r", "run now"),
                ("h", "to a session"),
                ("i", "details"),
                ("T", "trail"),
                ("Q", "queue"),
                ("d", "done"),
                ("e", "edit"),
                ("t", "add"),
                ("[ ]", "tabs"),
            ],
        };
        hints.extend([("K", "kind"), ("m", "move"), ("f", "filter kind")]);
        hints
    }

    pub(super) fn handle_composer(&mut self, key: Key) {
        if let Some(mut composer) = self.todo_composer.take() {
            match composer.handle(key) {
                Outcome::Editing => {
                    if let Err(error) = composer.checkpoint() {
                        composer.error = format!("Draft checkpoint failed: {error}");
                    }
                    self.todo_composer = Some(composer);
                }
                Outcome::Cancel => {
                    if let Err(error) = composer.cancel() {
                        composer.error = format!("Cannot discard draft: {error}");
                        self.todo_composer = Some(composer);
                    }
                }
                Outcome::Save => match composer.save(&mut self.todos) {
                    Ok(id) => {
                        let viewed = current_session();
                        self.saved_todo_scope =
                            (composer.owner_session != viewed).then(|| SavedTodoScope {
                                viewed_session: viewed,
                                owner_session: composer.owner_session.clone(),
                                project: composer.project.clone(),
                            });
                        if composer.id.is_none() {
                            self.show_todo_tab(TODO_TAB_TO_DO);
                        }
                        self.reveal_todo(&id);
                        self.message = if self.saved_todo_scope.is_some() {
                            "saved · Esc returns to viewed project"
                        } else {
                            "saved"
                        }
                        .into();
                    }
                    Err(error) => {
                        composer.error = error;
                        self.todo_composer = Some(composer);
                    }
                },
            }
        }
    }
}

fn todo_context(item: &todo::Item) -> String {
    let category = item
        .category()
        .map(|category| category.label().to_string())
        .unwrap_or_else(|| format!("Unknown: {}", item.0["category"]));
    let source = if !item.source().is_empty() {
        "Claude"
    } else if item.status() == todo::PROPOSED {
        "unknown session"
    } else {
        "you"
    };
    let routing = if item.is_design() {
        " · Design".to_string()
    } else if item.is_blocked_report() {
        format!(" · setup report from {}", item.reporter())
    } else if !item.link().is_empty() {
        format!(" · from a {} session ({})", item.reporter(), item.link())
    } else {
        " · standard routing".to_string()
    };
    format!("{category} · {} · {source}{routing}", item.status())
}

/// What Enter shows for an item or an issue.
pub(crate) fn todo_details(entry: &Entry) -> String {
    match entry {
        Entry::Typed(item) => {
            let status = match item.status().as_str() {
                todo::PROPOSED => "proposed by Claude",
                "doing" => "in progress",
                "done" => "done",
                _ => "to do",
            };
            let created = chrono::DateTime::from_timestamp(item.created(), 0)
                .map(|time| {
                    time.with_timezone(&chrono::Local)
                        .format("%d/%m %H:%M")
                        .to_string()
                })
                .unwrap_or_default();
            let filed = if !item.source().is_empty() {
                format!("Claude ({})", item.source())
            } else if item.status() == todo::PROPOSED {
                "an unknown session".into()
            } else {
                "you".into()
            };
            let mut text = format!(
                "{}\n\n{} · {status} · filed by {filed} on {created}\n",
                item.text(),
                item.project()
            );
            let category = item
                .category()
                .map(|category| category.label().to_string())
                .unwrap_or_else(|| format!("Unknown: {}", item.0["category"]));
            text.push_str(&format!("Kind: {category}\n"));
            text.push_str(&format!("{}\n", todo_context(item)));
            if !item.why().is_empty() {
                text.push_str(&format!("\nWhy Claude proposed it\n{}\n", item.why()));
            }
            text.push_str(&handoff_details(item));
            let notes = item.notes();
            text.push_str(&format!(
                "\nNotes\n{}\n",
                if notes.is_empty() {
                    "(none: e adds some)"
                } else {
                    notes.as_str()
                }
            ));
            for (number, file) in item.images() {
                text.push_str(&format!(
                    "{} {}\n",
                    crate::todo_images::marker(number),
                    crate::todo_images::path_of(&file).display()
                ));
            }
            text.push_str(if item.status() == todo::PROPOSED {
                "\nIn the column: y approve · n decline · e edit\n"
            } else {
                "\nIn the column: e edit · h hand to a session · d done\n"
            });
            text
        }
        Entry::Issue { issue, status, .. } => format!(
            "{}#{} {}\n\n{status} · labels: {}\n\n{}\n",
            issue.repository,
            issue.number,
            issue.title,
            issue.labels.join(", "),
            issue.url
        ),
    }
}
