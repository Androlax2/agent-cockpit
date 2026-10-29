use super::*;

impl App {
    /// What the session cards are drawn from, read from tmux, pgrep and the per-session files: once
    /// per tick, never on a key press (security audit 2026-10-19, performance 1).
    pub(super) fn gather_sessions(&mut self) -> Gathered {
        let mut running: Vec<Session> = sessions::running();
        let known: Vec<String> = running
            .iter()
            .map(|session| session.conversation.as_ref().unwrap_or(&session.session))
            .filter(|session_id| agents::is_session_id(session_id))
            .cloned()
            .collect();
        self.history.load_titles(&known, TITLES_PER_TICK);
        for session in &mut running {
            if session.state != "unknown" {
                session.facts = sessions::statusline_facts(&session.session);
            }
            session.title = self
                .history
                .title(session.conversation.as_deref().unwrap_or(&session.session));
            session.design = sessions::design_of(&session.session);
            session.role = sessions::role_of(&session.session);
        }
        let running_copies: HashSet<PathBuf> = running
            .iter()
            .filter_map(|s| sessions::running_copy_of(&s.cwd))
            .collect();
        self.observe_task_trails(&running);
        if util::read_json(&paths::snapshot())
            .is_some_and(|snapshot| snapshot.get("sessions").is_some_and(Value::is_array))
            && let Ok(continuity) = &mut self.continuity
        {
            match continuity.observe_sessions(&running, util::now_secs()) {
                Ok(true) => self.continuity_error = None,
                Ok(false) => {}
                Err(error) => self.continuity_error = Some(error),
            }
        }
        Gathered {
            branches: running
                .iter()
                .map(|session| (session.session.clone(), util::git_branch(&session.cwd)))
                .collect(),
            queued: running
                .iter()
                .map(|session| (session.session.clone(), prompts::count(&session.session)))
                .collect(),
            roots: running
                .iter()
                .map(|session| (session.session.clone(), util::repository_root(&session.cwd)))
                .collect(),
            running,
            paused: std::fs::read_to_string(paths::game_frozen())
                .map(|list| list.lines().filter(|line| !line.trim().is_empty()).count())
                .unwrap_or(0),
            start_files: sessions::start_file_mtimes(),
            // What a running copy holds and GitHub lacks: U on a card of its clones pushes it
            // (apply_run). Counted here, out of the sandbox, in the running copy no session writes;
            // only for the cards on screen, and like craft_roots at most every CRAFT_CHECK_S.
            running_copies_unpushed: if now() - self.running_copies_checked >= CRAFT_CHECK_S {
                self.running_copies_checked = now();
                running_copies
                    .into_iter()
                    .map(|live| {
                        let count = sessions::running_copy_unpushed(&live);
                        (live, count)
                    })
                    .collect()
            } else {
                self.running_copies_unpushed
                    .iter()
                    .filter(|(live, _)| running_copies.contains(*live))
                    .map(|(live, count)| (live.clone(), *count))
                    .collect()
            },
            craft_roots: if now() - self.craft_checked >= CRAFT_CHECK_S {
                self.craft_checked = now();
                ["figcraft"]
                    .iter()
                    .filter_map(|app| Some((app.to_string(), sessions::craft_roots(app)?)))
                    .collect()
            } else {
                self.craft_roots.clone()
            },
        }
    }

    /// Drops the exits that are over: the session is gone, or it failed and its card comes back
    /// with the reason in the footer. A clean exit may still be in the snapshot for a moment, until
    /// its hook says it ended; after EXIT_SETTLE_S it shows again rather than vanish unexplained.
    pub(super) fn settle_exits(&mut self, running: &[Session]) {
        let now = now();
        let mut warnings = Vec::new();
        let mut is_pending = |exit: &mut Exiting| {
            // The hook can disappear before tmux closes, leaving a fallback card with a new ID.
            if !running
                .iter()
                .any(|s| s.session == exit.session || s.tmux_pane == exit.tmux_pane)
            {
                return false;
            }
            if exit.ended_at.is_none() {
                match exit.child.try_wait() {
                    Ok(None) => return true,
                    Ok(Some(status)) if status.success() => exit.ended_at = Some(now),
                    Ok(Some(status)) => {
                        warnings.push(format!(
                            "⚠ {} did not exit ({status}): see ~/.local/state/cockpit/log",
                            exit.project
                        ));
                        return false;
                    }
                    Err(error) => {
                        warnings.push(format!("⚠ {} exit unknown: {error}", exit.project));
                        return false;
                    }
                }
            }
            if exit
                .ended_at
                .is_some_and(|ended| now - ended > EXIT_SETTLE_S)
            {
                warnings.push(format!(
                    "⚠ {} still reports after its exit: it may live on elsewhere",
                    exit.project
                ));
                return false;
            }
            true
        };
        for mut exit in std::mem::take(&mut self.exiting) {
            if is_pending(&mut exit) {
                self.exiting.push(exit);
            } else {
                // Its session can be gone while `cockpit exit` still runs.
                util::reap_in_background(exit.child);
            }
        }
        if let Some(warning) = warnings.pop() {
            self.message = warning;
        }
    }

    /// A session being restarted keeps its card, marked Restarting, and the selection while its old
    /// process is gone and the new one has not reported: without it the column selected and viewed
    /// the next card, which also killed a restart launching in the session pane. The new process's
    /// card takes the selection over.
    pub(super) fn hold_restarting_cards(
        &mut self,
        running: &mut Vec<Session>,
        selected_id: &mut Option<String>,
    ) {
        let mut held = HashMap::new();
        for restart in sessions::restarts() {
            let previous = self.restarting_cards.remove(&restart.session).or_else(|| {
                self.running_sessions
                    .iter()
                    .find(|session| session.session == restart.session)
                    .cloned()
            });
            let old_pane = previous
                .as_ref()
                .map(|card| card.tmux_pane.clone())
                .unwrap_or_default();
            if let Some(replacement) = restart.replacement(running, &old_pane) {
                if selected_id.as_deref() == Some(restart.session.as_str()) {
                    *selected_id = Some(replacement.session.clone());
                }
                sessions::end_restart(&restart.session);
                continue;
            }
            if now() as i64 - restart.since > RESTART_TIMEOUT_S {
                util::log_event(
                    "WARN",
                    &format!(
                        "{} did not report within {RESTART_TIMEOUT_S}s of its restart",
                        restart.project
                    ),
                );
                self.message = format!(
                    "⚠ {} did not come back from its restart: cockpit logs",
                    restart.project
                );
                sessions::end_restart(&restart.session);
                continue;
            }
            running.retain(|session| !restart.is_unreported_launch(session));
            let still_running = running
                .iter()
                .position(|session| session.session == restart.session)
                .map(|index| running.remove(index));
            let mut card = still_running.or(previous).unwrap_or_else(|| Session {
                session: restart.session.clone(),
                project: restart.project.clone(),
                cwd: restart.cwd.clone(),
                since: restart.since,
                started: Some(restart.since),
                ..Session::default()
            });
            card.restarting = true;
            card.state = "unknown".into();
            card.needs_input = false;
            card.ask_to_run.clear();
            held.insert(restart.session.clone(), card.clone());
            running.push(card);
        }
        self.restarting_cards = held;
    }

    /// A sleeping session keeps a card until it is woken. A note the session outlived (it started
    /// after the sleep) is stale. While the old process still shows, or a wake is under way
    /// (the restarting card), that card stands and the note adds none.
    pub(super) fn add_asleep_cards(&self, running: &mut Vec<Session>) {
        let waking: HashSet<String> = sessions::restarts()
            .into_iter()
            .map(|restart| restart.session)
            .collect();
        for note in sleep::asleep() {
            if waking.contains(&note.session) {
                continue;
            }
            match running
                .iter()
                .find(|session| session.session == note.session)
            {
                Some(session)
                    if session
                        .started
                        .is_some_and(|started| started >= note.slept_at) =>
                {
                    sleep::forget(&note.session)
                }
                Some(_) => {}
                None => running.push(note.card()),
            }
        }
    }
}
