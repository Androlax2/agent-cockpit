use super::*;

impl App {
    pub(super) fn run_continuity_action(&mut self, name: &str, screen: &mut dyn Screen) -> String {
        if name == "queue" { return self.open_queue(); }
        if let Err(error) = &self.continuity { return format!("Continuity unavailable: {error}"); }
        if name == "updates" { return self.open_updates(screen); }
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
        self.gathered.as_ref().map(|gathered| gathered.running.clone()).unwrap_or_else(|| self.sessions.clone())
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
        if self.sessions.iter().any(|candidate| candidate.session == session && !candidate.is_external()) {
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
                if !self.sessions.iter().any(|candidate| candidate.session == *session && !candidate.is_external()) { return "session has exited".into(); }
                self.session_details = None;
                self.manager_async(&["view", session])
            }
            None => String::new(),
        }
    }
}
