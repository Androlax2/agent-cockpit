use super::*;
use crate::continuity::{TaskSnapshot, TrailTarget, DeferralTarget, task_key};

impl App {
    pub(super) fn run_continuity_action(&mut self, name: &str, screen: &mut dyn Screen) -> String {
        if name == "queue" { return self.open_queue(); }
        if name == "attention-filter" { self.attention_only = !self.attention_only; return "attention filter toggled".into(); }
        if let Err(error) = &self.continuity { return format!("Continuity unavailable: {error}"); }
        if name == "updates" { return self.open_updates(screen); }
        if name == "defer-20m" || name == "bring-back" { return self.change_deferral(name); }
        if name == "task-trail" {
            let target = match self.focus {
                3 => self.todo_entries.get(self.panels[3].selected).map(|entry| TrailTarget::Task(task_key(entry))),
                2 => self.review_rows.get(self.panels[2].selected).map(|review| TrailTarget::PullRequest { repository: review.full_repository.clone(), number: review.number }),
                _ => self.selected_session().map(|session| TrailTarget::Session(session.session)),
            };
            return target.map(|target| self.open_task_trail(target, screen)).unwrap_or_else(|| "select a task, session or PR first".into());
        }
        let Some(session) = self.selected_session() else { return "select a session first".into() };
        if name == "reviewed" {
            let identities = self.continuity.as_ref().unwrap().updates().into_iter()
                .filter(|update| update.session == session.session).map(|update| update.identity).collect();
            return self.review_updates(&session.session, &identities);
        }
        let note = match name {
            "return-note" => {
                let note = input_line(screen, "Your return note (empty cancels)> ");
                if note.is_empty() { return "unchanged".into(); }
                Some(note)
            }
            "clear-return-note" => None,
            _ => return format!("unknown continuity action: {name}"),
        };
        match self.continuity.as_mut().unwrap().set_note(&session, note.as_deref()) {
            Ok(changed) => {
                self.continuity_error = None;
                if changed {
                    crate::audit::record("column", "continuity.note", "ok", serde_json::json!({"session":session.session,
                        "text":crate::audit::fingerprint(note.as_deref().unwrap_or(""))}));
                }
                if note.is_some() { "return note saved".into() } else { "return note cleared".into() }
            }
            Err(error) => {
                if error.starts_with("cannot save") { self.continuity_error = Some(error.clone()); }
                error
            }
        }
    }

    pub(super) fn all_sessions(&self) -> Vec<Session> {
        let mut sessions = self.gathered.as_ref().map(|gathered| gathered.running.clone()).unwrap_or_else(|| self.sessions.clone());
        sessions.retain(|session| !self.exiting.iter().any(|exit| exit.session == session.session || exit.tmux_pane == session.tmux_pane));
        sessions
    }

    pub(super) fn session_deferred_until(&self, session: &Session, now: i64) -> Option<i64> {
        let target = DeferralTarget::question(session)?;
        self.continuity.as_ref().ok()?.deferred_until(&target, now)
    }

    pub(super) fn is_actionable(&self, session: &Session, now: i64) -> bool {
        !session.is_external() && session.needs_attention() && self.session_deferred_until(session, now).is_none()
    }

    pub(crate) fn attention_indices(&self, now: i64) -> Vec<usize> {
        self.sessions.iter().enumerate().filter(|(_, session)| self.is_actionable(session, now)).map(|(index, _)| index).collect()
    }

    fn change_deferral(&mut self, name: &str) -> String {
        let target = match self.focus {
            1 => self.selected_session().as_ref().and_then(DeferralTarget::question),
            2 => self.review_rows.get(self.panels[2].selected).and_then(DeferralTarget::review),
            _ => None,
        };
        let target = target.or_else(|| {
            if name != "bring-back" { return None; }
            let key = match self.focus {
                1 => format!("question:{}", self.selected_session()?.session),
                2 => { let pr = self.review_rows.get(self.panels[2].selected)?; format!("pr:{}#{}", pr.full_repository, pr.number) }
                _ => return None,
            };
            self.continuity.as_ref().ok()?.saved_deferral(&key).map(|deferral| deferral.target.clone())
        });
        let Some(target) = target else { return "only explicit questions or reviews with a known revision can be deferred; approvals and command input stay immediate".into() };
        let state = self.continuity.as_mut().unwrap();
        let result = if name == "bring-back" { state.bring_back(&target) } else { state.defer(&target, util::now_secs()) };
        match result {
            Ok(_) => {
                self.continuity_error = None;
                crate::audit::record("column", "continuity.deferral", "ok", serde_json::json!({"action":name,"target":target.storage_key()}));
                if name == "bring-back" { "reminder restored; nothing was answered or approved".into() }
                else { "deferred for twenty minutes; question/review remains unresolved".into() }
            }
            Err(error) => { self.continuity_error = Some(error.clone()); error }
        }
    }

    pub(super) fn record_task_assignment(&mut self, entry: &Entry, session: Option<&Session>) -> Result<bool, String> {
        let repository = match entry {
            Entry::Issue { issue, .. } => issue.full_repository.to_lowercase(),
            Entry::Typed(item) => session.map(|session| session.repository.clone()).unwrap_or_else(|| {
                let root = util::repository_root(&item.cwd());
                self.reviews.checkouts.iter().find(|(_, checkout)| util::repository_root(checkout) == root)
                    .map(|(repository, _)| repository.to_lowercase()).unwrap_or_default()
            }),
        };
        let mut snapshot = TaskSnapshot::from_entry(entry, &repository);
        snapshot.status = "doing".into();
        let state = self.continuity.as_mut().map_err(|error| error.clone())?;
        match session {
            Some(session) => state.assign(&snapshot, session, util::now_secs()),
            None => state.prepare_task(&snapshot),
        }
    }

    pub(super) fn observe_task_trails(&mut self, sessions: &[Session]) {
        let reviews = self.reviews.cached();
        let issues = self.reviews.issues();
        if let Ok(state) = &mut self.continuity {
            let result = state.observe_tasks(&self.todos, sessions, &reviews, util::now_secs()).and_then(|_| {
                for issue in issues {
                    let entry = Entry::Issue { status: self.todos.status_of(&issue.key()), priority: todo::issue_priority(&issue.labels), issue: issue.clone() };
                    state.remember_task(&TaskSnapshot::from_entry(&entry, &issue.full_repository.to_lowercase()))?;
                }
                Ok(())
            });
            if let Err(error) = result { self.continuity_error = Some(error); }
        }
    }

    pub(super) fn open_task_trail(&mut self, target: TrailTarget, screen: &mut dyn Screen) -> String {
        let state = self.continuity.as_ref().unwrap();
        let keys: Vec<_> = state.tasks().iter().filter(|(key, record)| match &target {
            TrailTarget::Task(task) => *key == task,
            TrailTarget::Session(session) => record.assignments.iter().any(|assignment| assignment.session == *session),
            TrailTarget::PullRequest { repository, number } => record.assignments.iter().any(|assignment| assignment.prs.contains(&format!("{repository}#{number}"))),
        }).map(|(key, _)| key.clone()).collect();
        if keys.is_empty() {
            let mut text = "No linked session or task recorded.\nNo result recorded.\nNo linked PR.\n".to_string();
            if let TrailTarget::Task(key) = &target {
                if let Some(entry) = self.todo_entries.iter().find(|entry| task_key(entry) == *key) { text = format!("{}\n\n{text}", todo_details(entry)); }
            }
            self.session_details = Some(SessionDetails { title: "Task trail".into(), text, scroll: 0, actions: Vec::new() });
            return String::new();
        }
        let key = if keys.len() == 1 { keys[0].clone() } else {
            let entries: Vec<_> = keys.iter().enumerate().map(|(index, key)| format!("{index}\t{} · {key}", state.tasks()[key].snapshot.title)).collect();
            let index = screen.outside(&mut || fzf("linked task> ", &entries, &["--delimiter", "\t", "--with-nth", "2.."]))
                .and_then(|choice| choice.split('\t').next()?.parse::<usize>().ok());
            let Some(key) = index.and_then(|index| keys.get(index)) else { return String::new() };
            key.clone()
        };
        let record = &state.tasks()[&key];
        let snapshot = &record.snapshot;
        let mut text = format!("{}\nTask: {key} · {}\n{}\n\n", snapshot.title, snapshot.status, snapshot.notes);
        let mut actions = vec![DetailAction::OpenTask(key.clone())];
        if record.pending { text += "Starting; awaiting assignment\n\n"; }
        if record.assignments.is_empty() { text += "No linked session\nNo result recorded\n"; }
        let sessions = self.all_sessions();
        let mut linked_prs = std::collections::BTreeSet::new();
        for assignment in &record.assignments {
            text += &format!("Session: {}\nAssigned {} · {} / {}\n", assignment.session, age(assignment.assigned_at), assignment.repository, assignment.branch);
            if let Some(result) = &assignment.result { text += &format!("Latest recorded result ({}): {}\n", age(result.completed_at), result.outcome); }
            else { text += "No result recorded\n"; }
            if sessions.iter().any(|session| session.session == assignment.session && !session.is_external()) {
                actions.push(DetailAction::OpenSession(assignment.session.clone()));
            } else {
                text += "Archived session; no live terminal target.\n";
                if let Some(memory) = state.memories().get(&assignment.session) {
                    if let Some(note) = &memory.note { text += &format!("Your note: {note}\n"); }
                    if !memory.cwd.is_empty() && self.history.entries.iter().any(|past| past.session == assignment.session && past.cwd == memory.cwd) {
                        actions.push(DetailAction::ResumeSession { session: assignment.session.clone(), cwd: memory.cwd.clone() });
                    }
                }
            }
            linked_prs.extend(assignment.prs.iter().cloned());
            text += "\n";
        }
        match self.reviews.availability(&snapshot.repository) {
            Ok(()) if linked_prs.is_empty() => text += "No linked PR\n",
            Err(reason) => text += &format!("{reason}\n"),
            _ => {},
        }
        let current = self.reviews.cached();
        for id in linked_prs {
            let pr = &state.prs()[&id];
            let present = current.iter().any(|row| row.full_repository == pr.repository && row.number == pr.number);
            text += &format!("{}#{} · {}{}\n", pr.repository, pr.number, pr.title, if present { "" } else { " (last known; current state unavailable)" });
            if !pr.url.is_empty() { actions.push(DetailAction::OpenPr(id)); }
        }
        self.session_details = Some(SessionDetails { title: "Task trail".into(), text, scroll: 0, actions });
        String::new()
    }

    fn open_trail_task(&mut self, key: &str) -> String {
        let entry = self.todos.items.iter().find(|item| item.id() == key).map(|item| Entry::Typed(item.clone()))
            .or_else(|| self.reviews.issues().into_iter().find(|issue| issue.key().eq_ignore_ascii_case(key)).map(|issue|
                Entry::Issue { status: self.todos.status_of(&issue.key()), priority: todo::issue_priority(&issue.labels), issue }));
        if let Some(entry) = entry {
            self.todo_jump = Some((current_session(), key.into()));
            self.show_todo_tab(match entry.status().as_str() { "doing" => 3, "done" => 4, "proposed" => 1, _ => 2 });
            self.build_todo();
            if let Some(index) = self.todo_entries.iter().position(|entry| task_key(entry) == key) { self.panels[3].selected = index; }
            self.focus_panel(3);
            self.session_details = None;
        } else if let Some(record) = self.continuity.as_ref().ok().and_then(|state| state.tasks().get(key)) {
            self.session_details = Some(SessionDetails { title: "Archived task".into(),
                text: format!("Archived task\n{}\n{}\nLast recorded status: {}\n\n{}\n\nThe source item is no longer available; it was not recreated.",
                    record.snapshot.title, key, record.snapshot.status, record.snapshot.notes), scroll: 0, actions: Vec::new() });
        }
        String::new()
    }

    pub(super) fn queue_context(&self) -> queue::QueueContext {
        queue::QueueContext { now: util::now_secs(), enabled: settings::is_on("todo-queue"), gaming: (self.gaming)(),
            viewed: current_session(), workspace_active: util::claude_workspace_active() }
    }

    pub(super) fn open_queue(&mut self) -> String {
        let context = self.queue_context();
        let sessions = self.all_sessions();
        let issues = self.reviews.issues();
        let mut entries: Vec<_> = self.todos.items.iter()
            .filter(|item| self.todos.is_queued(&item.id()) && (self.todo_scope.is_empty() || item.project() == self.todo_scope))
            .map(|item| Entry::Typed(item.clone())).collect();
        entries.extend(issues.iter().filter(|issue| self.todos.is_queued(&issue.key())).map(|issue|
            Entry::Issue { issue: issue.clone(), status: self.todos.status_of(&issue.key()), priority: todo::issue_priority(&issue.labels) }));
        entries.sort_by(todo::queue_order);
        let mut text = format!("Queue · {}\n\n", if self.todo_scope.is_empty() { "all projects" } else { &self.todo_scope });
        if !context.enabled { text += "queue disabled\n\n"; }
        if context.gaming { text += "paused while gaming\n\n"; }
        for entry in &entries {
            let title = match entry { Entry::Typed(item) => item.text(), Entry::Issue { issue, .. } => issue.title.clone() };
            let role = if matches!(entry, Entry::Typed(item) if item.is_design()) { "designer" } else { "developer" };
            text += &format!("{title} · {role} · {}\n", entry.status());
            if entry.status() != "todo" { text += "  not To do; cannot dispatch\n\n"; continue; }
            let matching: Vec<_> = sessions.iter().filter(|session|
                queue::candidates(session, &self.todos, &issues, &self.reviews.checkouts).iter().any(|candidate| candidate.key() == entry.key())).collect();
            if matching.is_empty() { text += "  no matching session\n"; }
            for session in matching {
                let first = queue::candidates(session, &self.todos, &issues, &self.reviews.checkouts).into_iter().next();
                let reason = if let Some(block) = queue::block(session, &context, self.todos.busy(&session.session), prompts::count(&session.session) > 0) {
                    block.to_string()
                } else if let Some(first) = first.filter(|first| first.key() != entry.key()) {
                    format!("queued behind {}", match first { Entry::Typed(item) => item.text(), Entry::Issue { issue, .. } => issue.title })
                } else { "ready for the next dispatch check (every 30s)".into() };
                text += &format!("  {}: {reason}\n", if session.label.is_empty() { &session.session } else { &session.label });
            }
            text += "\n";
        }
        if entries.is_empty() { text += "No known queued items in this scope.\n"; }
        let (_, error, loading) = self.reviews.current();
        if !error.is_empty() { text += &format!("\nIssue data unavailable: {error}\n"); }
        if loading { text += "\nIssue data loading; explanation uses the last fetched items.\n"; }
        if self.todos.queued.iter().any(|key| key.contains('#') && !issues.iter().any(|issue| issue.key() == *key)) {
            text += "\nQueued issue data unavailable: not loaded, stale, or closed.\n";
        }
        self.session_details = Some(SessionDetails { title: "Queue".into(), text, scroll: 0, actions: Vec::new() });
        String::new()
    }

    fn review_updates(&mut self, session: &str, identities: &Vec<String>) -> String {
        match self.continuity.as_mut().map_err(|error| error.clone()).and_then(|state| state.reviewed(session, identities)) {
            Ok(changed) => {
                self.continuity_error = None;
                if changed { crate::audit::record("column", "continuity.reviewed", "ok", serde_json::json!({"session":session,"identities":identities})); }
                "updates marked reviewed; questions still need answers".into()
            }
            Err(error) => { self.continuity_error = Some(error.clone()); error }
        }
    }

    pub(super) fn open_updates(&mut self, screen: &mut dyn Screen) -> String {
        let state = self.continuity.as_ref().unwrap();
        let updates = state.updates();
        let mut entries: Vec<String> = updates.iter().enumerate().map(|(index, update)| {
            let title = state.memories().get(&update.session).map(|memory| memory.title.as_str()).unwrap_or("");
            format!("{index}\t{} · {title} · {}", match update.kind { crate::continuity::UpdateKind::Result => "Latest turn", crate::continuity::UpdateKind::Question => "Question" }, age(update.observed_at))
        }).collect();
        let notes: Vec<_> = state.memories().iter().filter(|(id, memory)| memory.note.is_some()
            && !updates.iter().any(|update| update.session == **id)).collect();
        for (index, (_, memory)) in notes.iter().enumerate() {
            entries.push(format!("{}\tYour note · {}", updates.len() + index, memory.title));
        }
        if entries.is_empty() { return "no unseen updates or return notes".into(); }
        let choice = screen.outside(&mut || fzf("updates> ", &entries, &["--delimiter", "\t", "--with-nth", "2.."]))
            .and_then(|choice| choice.split('\t').next()?.parse::<usize>().ok());
        let Some(index) = choice else { return String::new() };
        let (session, text, identities) = if let Some(update) = updates.get(index) {
            (update.session.clone(), format!("{}\n{}\n\n{}", match update.kind { crate::continuity::UpdateKind::Result => "Latest turn", crate::continuity::UpdateKind::Question => "Question" }, age(update.observed_at), update.text), vec![update.identity.clone()])
        } else if let Some((id, _)) = index.checked_sub(updates.len()).and_then(|index| notes.get(index)) {
            ((*id).clone(), "Return note".into(), Vec::new())
        } else { return "update no longer available".into() };
        let memory = &state.memories()[&session];
        let mut text = format!("{}\nSession: {session}\n\n{text}", memory.title);
        if let Some(note) = &memory.note { text += &format!("\n\nYour note: {note}"); }
        let mut actions = Vec::new();
        if !identities.is_empty() { actions.push(DetailAction::Reviewed { session: session.clone(), identities }); }
        if self.running_sessions.iter().any(|candidate| candidate.session == session && !candidate.is_external()) {
            actions.push(DetailAction::OpenSession(session));
        } else { text += "\n\nArchived session; no live terminal target."; }
        self.session_details = Some(SessionDetails { title: "Updates".into(), text, scroll: 0, actions });
        String::new()
    }

    pub(super) fn pick_detail_action(&mut self, screen: &mut dyn Screen) -> String {
        let actions = self.session_details.as_ref().unwrap().actions.clone();
        let entries: Vec<_> = actions.iter().enumerate().map(|(index, action)| format!("{index}\t{}", match action {
            DetailAction::Reviewed { .. } => "Mark displayed update reviewed (not answered)",
            DetailAction::OpenSession(_) => "Open live session",
            DetailAction::OpenTask(_) => "Open task (archived details if its source is gone)",
            DetailAction::OpenPr(_) => "Open linked PR (never merge)",
            DetailAction::ResumeSession { .. } => "Resume this archived conversation explicitly",
        })).collect();
        let index = screen.outside(&mut || fzf("update action> ", &entries, &["--delimiter", "\t", "--with-nth", "2.."]))
            .and_then(|choice| choice.split('\t').next()?.parse::<usize>().ok());
        match index.and_then(|index| actions.get(index)) {
            Some(DetailAction::Reviewed { session, identities }) => {
                let result = self.review_updates(session, identities);
                if self.continuity_error.is_none() { self.session_details = None; }
                result
            }
            Some(DetailAction::OpenSession(session)) => {
                if !self.all_sessions().iter().any(|candidate| candidate.session == *session && !candidate.is_external()) { return "session has exited".into(); }
                self.session_details = None;
                self.manager_async(&["view", session])
            }
            Some(DetailAction::OpenTask(key)) => self.open_trail_task(key),
            Some(DetailAction::OpenPr(key)) => {
                let Some(pr) = self.continuity.as_ref().ok().and_then(|state| state.prs().get(key)) else { return "PR link unavailable".into() };
                let url = pr.url.clone();
                self.session_details = None;
                open_in_scratchpad("prs", &url)
            }
            Some(DetailAction::ResumeSession { session, cwd }) => {
                if !self.history.entries.iter().any(|past| past.session == *session && past.cwd == *cwd) { return "archived conversation unavailable".into(); }
                let result = self.manager_async(&["resume", cwd, session]);
                self.session_details = None;
                result
            }
            None => String::new(),
        }
    }
}
