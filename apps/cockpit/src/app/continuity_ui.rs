use super::*;

impl App {
    pub(super) fn run_continuity_action(&mut self, name: &str, screen: &mut dyn Screen) -> String {
        if let Err(error) = &self.continuity { return format!("Continuity unavailable: {error}"); }
        let Some(session) = self.selected_session() else { return "select a session first".into() };
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
                if changed {
                    crate::audit::record("column", "continuity.note", "ok", serde_json::json!({"session":session.session,
                        "text":crate::audit::fingerprint(note.as_deref().unwrap_or(""))}));
                }
                if note.is_some() { "return note saved".into() } else { "return note cleared".into() }
            }
            Err(error) => error,
        }
    }
}
