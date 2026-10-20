use super::*;

impl App {
    pub(super) fn run_continuity_action(&mut self, name: &str, screen: &mut dyn Screen) -> String {
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
