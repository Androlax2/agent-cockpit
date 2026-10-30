use super::*;

impl App {
    pub(super) fn build_sessions(&mut self) {
        use crate::teams::sidebar::{self, SidebarRow};
        let selected_row = self.sidebar_rows.get(self.panels[1].selected).cloned();
        let selected_team = selected_row.as_ref().and_then(|row| match row {
            SidebarRow::Team { task } | SidebarRow::PlannedQa { task } => Some(task.clone()),
            SidebarRow::Session { session_id } => {
                self.sidebar_session_tasks.get(session_id).cloned()
            }
        });
        let mut selected_id = self.selected_session().map(|session| session.session);
        let gathered = match self.gathered.take() {
            Some(gathered) => gathered,
            None => self.gather_sessions(),
        };
        let mut running: Vec<Session> = gathered.running.clone();
        self.session_branches = gathered.branches.clone();
        self.session_queued = gathered.queued.clone();
        self.session_roots = gathered.roots.clone();
        self.settle_exits(&running);
        running.retain(|s| {
            !s.is_external()
                && !self
                    .exiting
                    .iter()
                    .any(|exit| exit.session == s.session || exit.tmux_pane == s.tmux_pane)
        });
        self.hold_restarting_cards(&mut running, &mut selected_id);
        self.add_asleep_cards(&mut running);
        for session in &mut running {
            session.pinned = self.view.pinned.contains(&session.session);
        }
        self.running_sessions = sessions::grouped_by_project(running.clone());
        // A card keeps its place: pinned first, then in the order the sessions started.
        let running = sessions::grouped_by_project(running);
        let mut rows = match &self.team_links {
            TeamLinks::Ready(snapshot) => {
                sidebar::grouped_rows(snapshot, &running, &self.collapsed_teams).unwrap_or_default()
            }
            _ => running
                .iter()
                .map(|session| SidebarRow::Session {
                    session_id: session.session.clone(),
                })
                .collect(),
        };
        if let TeamLinks::Ready(snapshot) = &self.team_links {
            let hidden_tasks: Vec<_> = rows
                .iter()
                .filter_map(|row| match row {
                    SidebarRow::Team { task }
                        if self.team_is_inactive(snapshot, task, &running) =>
                    {
                        Some(task.clone())
                    }
                    _ => None,
                })
                .collect();
            rows.retain(|row| match row {
                SidebarRow::Team { task } | SidebarRow::PlannedQa { task } => {
                    !hidden_tasks.contains(task)
                }
                SidebarRow::Session { session_id } => !running
                    .iter()
                    .find(|session| session.session == *session_id)
                    .and_then(|session| sidebar::member_task(snapshot, session))
                    .is_some_and(|task| hidden_tasks.contains(&task)),
            });
        }
        // Grouped by attention across projects (NEEDS YOU, WORKING, QUIET); a team stays one block.
        let now_secs = util::now_secs();
        let mut blocks: Vec<(Attention, (bool, i64), Vec<SidebarRow>)> = Vec::new();
        for row in rows {
            let team_task = match (&self.team_links, &row) {
                (
                    TeamLinks::Ready(_),
                    SidebarRow::Team { task } | SidebarRow::PlannedQa { task },
                ) => Some(task.clone()),
                (TeamLinks::Ready(snapshot), SidebarRow::Session { session_id }) => running
                    .iter()
                    .find(|session| session.session == *session_id)
                    .and_then(|session| sidebar::member_task(snapshot, session)),
                _ => None,
            };
            let joins_team = team_task.is_some() && !matches!(row, SidebarRow::Team { .. }) && blocks.last().is_some_and(|(_, _, block)|
                matches!(block.first(), Some(SidebarRow::Team { task }) if Some(task) == team_task.as_ref()));
            if joins_team {
                blocks.last_mut().unwrap().2.push(row);
            } else {
                blocks.push((Attention::Quiet, (false, 0), vec![row]));
            }
        }
        for (group, oldest_ask, block) in &mut blocks {
            let members: Vec<&Session> = block
                .iter()
                .filter_map(|row| match row {
                    SidebarRow::Session { session_id } => running
                        .iter()
                        .find(|session| session.session == *session_id),
                    _ => None,
                })
                .collect();
            let team_needs_you = matches!((&self.team_links, block.first()), (TeamLinks::Ready(snapshot), Some(SidebarRow::Team { task })) if sidebar::attention(snapshot, task).is_some());
            *group = members
                .iter()
                .map(|session| self.session_attention(session, now_secs))
                .min()
                .unwrap_or(Attention::Quiet);
            if team_needs_you {
                *group = Attention::NeedsYou;
            }
            let is_pinned = members.iter().any(|session| session.pinned);
            let oldest = members
                .iter()
                .filter(|session| self.is_actionable(session, now_secs))
                .map(|session| session.since)
                .min()
                .unwrap_or(0);
            *oldest_ask = (
                !is_pinned,
                if *group == Attention::NeedsYou {
                    oldest
                } else {
                    0
                },
            );
        }
        // Pinned first inside their group, then the oldest ask first in NEEDS YOU; the others keep their places.
        blocks.sort_by_key(|(group, order, _)| (*group, *order));
        let mut blocks_per_group: HashMap<Attention, usize> = HashMap::new();
        for (group, _, _) in &blocks {
            *blocks_per_group.entry(*group).or_default() += 1;
        }
        let row_groups: Vec<Attention> = blocks
            .iter()
            .flat_map(|(group, _, block)| std::iter::repeat_n(*group, block.len()))
            .collect();
        let row_ends_block: Vec<bool> = blocks
            .iter()
            .flat_map(|(_, _, block)| {
                (0..block.len()).map(move |position| position + 1 == block.len())
            })
            .collect();
        let rows: Vec<SidebarRow> = blocks.into_iter().flat_map(|(_, _, block)| block).collect();
        let current = current_session();
        let mut lines = Vec::new();
        if now() - self.restore_checked > RESTORE_CHECK_S {
            self.restore_checked = now();
            self.restore = sessions::restore_offer(
                &self.view,
                &self
                    .running_sessions
                    .iter()
                    .map(|s| s.session.clone())
                    .collect(),
            );
        }
        if now() - self.held_checked > GREYPROXY_POLL_S {
            self.held_checked = now();
            let held = sessions::greyproxy_held(&paths::greyproxy_db());
            for host in held.iter().filter(|host| !self.held.contains(host)) {
                crate::audit::record(
                    "column",
                    "greyproxy.held",
                    "waiting",
                    serde_json::json!({"destination": host}),
                );
            }
            self.held_since = if held.is_empty() {
                None
            } else {
                self.held_since.or(Some(util::now_secs()))
            };
            self.held = held;
        }
        if now() - self.disk_checked > DISK_CHECK_S {
            self.disk_checked = now();
            self.disk = (self.disk_usage)();
        }
        // game-performance froze the sessions for a game: their cards stand still until it ends.
        let paused = gathered.paused;
        self.start_files = gathered.start_files.clone();
        self.running_copies_unpushed = gathered.running_copies_unpushed.clone();
        self.craft_roots = gathered.craft_roots.clone();
        self.gathered = Some(gathered);
        self.notices = self.current_notices(paused);
        for notice in self.notices.iter().take(NOTICES_SHOWN) {
            lines.push(notice_line(notice));
        }
        if self.notices.len() > NOTICES_SHOWN {
            lines.push(Line::new(vec![segment(
                0,
                format!(
                    "+{} more · s for Status",
                    self.notices.len() - NOTICES_SHOWN
                ),
                dim(FG_DARK),
            )]));
        }
        if !self.notices.is_empty() {
            lines.push(Line::default());
        }
        self.session_task_titles.clear();
        for session in &running {
            let branch = self
                .session_branches
                .get(&session.session)
                .map(String::as_str)
                .unwrap_or("");
            self.session_task_titles.insert(
                session.session.clone(),
                self.session_task_title(session, branch),
            );
        }
        let link_notice = match &self.team_links {
            TeamLinks::Loading => Some("Loading team links…"),
            TeamLinks::Unavailable => Some("Team links unavailable"),
            TeamLinks::Ready(snapshot)
                if !rows
                    .iter()
                    .any(|row| matches!(row, SidebarRow::Team { .. }))
                    && sidebar::list(snapshot, "members").is_empty() =>
            {
                Some("No linked teams")
            }
            _ => None,
        };
        let mut group = None;
        // A team's own ask closes its block, under the members (frame 3737).
        let mut team_ask: Option<Line> = None;
        let held_cards = usize::from(!self.held.is_empty());
        if held_cards > 0 {
            group = Some(Attention::NeedsYou);
            let count = blocks_per_group
                .get(&Attention::NeedsYou)
                .copied()
                .unwrap_or(0)
                + held_cards;
            lines.push(Line::new(vec![segment(
                0,
                format!("{} · {count}", Attention::NeedsYou.heading()),
                bold(YELLOW),
            )]));
            lines.extend(greyproxy_card(
                &self.held,
                self.held_since.unwrap_or(now_secs),
            ));
        }
        for (index, row) in rows.iter().enumerate() {
            let session = if let SidebarRow::Session { session_id } = row {
                running
                    .iter()
                    .find(|session| session.session == *session_id)
            } else {
                None
            };
            let team_task = match (&self.team_links, row) {
                (
                    TeamLinks::Ready(_),
                    SidebarRow::Team { task } | SidebarRow::PlannedQa { task },
                ) => Some(task.clone()),
                (TeamLinks::Ready(snapshot), _) => {
                    session.and_then(|session| sidebar::member_task(snapshot, session))
                }
                _ => None,
            };
            let row_group = row_groups.get(index).copied().unwrap_or(Attention::Quiet);
            if group != Some(row_group) {
                group = Some(row_group);
                let count = blocks_per_group.get(&row_group).copied().unwrap_or(0);
                let tint = if row_group == Attention::NeedsYou {
                    bold(YELLOW)
                } else {
                    bold(FG_DARK)
                };
                lines.push(Line::new(vec![segment(
                    0,
                    format!("{} · {count}", row_group.heading()),
                    tint,
                )]));
            }
            if let (TeamLinks::Ready(snapshot), Some(task)) = (&self.team_links, &team_task) {
                match row {
                    // Frame 3737: the block keeps role, doing now and age per member; messages,
                    // handoffs, links and history are on the 4 Teams tab.
                    SidebarRow::Team { .. } => {
                        let members = sidebar::member_count(snapshot, task, &running);
                        let is_folded = self.collapsed_teams.contains(&sidebar::task_key(task));
                        let marker = if is_folded { "▸" } else { "▾" };
                        let accent = (row_group == Attention::NeedsYou).then_some(bold(YELLOW));
                        lines.push(Line {
                            segments: vec![segment(
                                0,
                                format!("{marker} {}", self.team_task_title(snapshot, task)),
                                bold(CYAN),
                            )],
                            right: vec![
                                part(" 4 ", on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD)),
                                part("team details", dim(FG_DARK)),
                            ],
                            item: Some(index),
                            accent,
                            ..Line::default()
                        });
                        let project =
                            sidebar::shown(&sidebar::project_name(snapshot, task, &running));
                        lines.push(Line {
                            segments: vec![segment(
                                2,
                                format!("{project} › {}", sidebar::shown(&task.task_id)),
                                dim(FG_DARK),
                            )],
                            item: Some(index),
                            accent,
                            ..Line::default()
                        });
                        team_ask = sidebar::attention(snapshot, task).map(|reason| {
                            ask_bar(
                                Some(index),
                                &format!("! {}", reason.trim_start_matches("Needs you: ")),
                                "",
                                "⏎ answer",
                            )
                        });
                        if members == 0 || is_folded {
                            lines.extend(team_ask.take());
                            lines.push(Line::default());
                        }
                        continue;
                    }
                    SidebarRow::PlannedQa { .. } => {
                        let (text, status) = match sidebar::qa_status(snapshot, task).as_str() {
                            "QA planned · Fresh reviewer starts at handoff" => {
                                ("Starts at the next handoff".to_string(), "planned")
                            }
                            other => (other.trim_start_matches("QA ").to_string(), ""),
                        };
                        lines.push(Line {
                            segments: vec![
                                segment(1, "○", dim(FG_DARK)),
                                segment(3, "QA", style(FG_DARK)),
                                segment(14, text, dim(FG_DARK)),
                            ],
                            right: vec![part(status, dim(FG_DARK))],
                            item: Some(index),
                            ..Line::default()
                        });
                        if row_ends_block.get(index).copied().unwrap_or(true) {
                            lines.extend(team_ask.take());
                            lines.push(Line::default());
                        }
                        continue;
                    }
                    SidebarRow::Session { .. } => {
                        let Some(session) = session else { continue };
                        let role = sidebar::membership(snapshot, session)
                            .and_then(sidebar::role)
                            .map(sidebar::role_label)
                            .unwrap_or("Member");
                        let accent = (session.session == current).then_some(style(BLUE));
                        let card = self.session_card(session, now_secs);
                        let (glyph, tint) = if card.ask.is_some() {
                            ("!", YELLOW)
                        } else if session.asleep.is_some() {
                            ("◌", FG_DARK)
                        } else if session.restarting {
                            ("⟳", BLUE)
                        } else {
                            ("●", BLUE)
                        };
                        // A member's own step while it works, else its title: the line still says which session it is.
                        let has_step = card.group == Attention::Working
                            && sessions::doing_now(session, now_secs).is_some();
                        let shown = if has_step
                            || card.ask.is_some()
                            || session.asleep.is_some()
                            || session.restarting
                        {
                            card.doing.clone()
                        } else {
                            sidebar::shown(self.session_display_title(session))
                        };
                        let age_tint = if card.group == Attention::Working {
                            style(BLUE)
                        } else {
                            dim(FG_DARK)
                        };
                        lines.push(Line {
                            segments: vec![
                                segment(1, glyph, bold(tint)),
                                segment(3, role, bold(FG)),
                                segment(14, shown, style(FG)),
                            ],
                            right: vec![part(card.age.clone(), age_tint)],
                            item: Some(index),
                            accent,
                            ..Line::default()
                        });
                        if let Some((ask, verb)) = &card.ask {
                            lines.push(ask_bar(
                                Some(index),
                                &format!("! {role}: {ask}"),
                                &card.ask_count,
                                &format!("⏎ {verb}"),
                            ));
                        }
                        if row_ends_block.get(index).copied().unwrap_or(true) {
                            lines.extend(team_ask.take());
                            lines.push(Line::default());
                        }
                        continue;
                    }
                }
            }
            let Some(session) = session else { continue };
            let card = self.session_card(session, now_secs);
            let accent = if card.ask.is_some() {
                Some(bold(YELLOW))
            } else {
                (session.session == current).then_some(bold(BLUE))
            };
            let task_title = self.session_task_title(
                session,
                self.session_branches
                    .get(&session.session)
                    .map(String::as_str)
                    .unwrap_or(""),
            );
            self.session_task_titles
                .insert(session.session.clone(), task_title);
            let text_style = if card.group == Attention::Quiet && !card.bright {
                style(FG_DARK)
            } else {
                bold(FG)
            };
            // Until the grouping replaces continuity updates (todo f192c901), a new question stays marked; an
            // unseen result is the card's own state (session_card).
            let updates = self
                .continuity
                .as_ref()
                .ok()
                .map(|state| state.updates())
                .unwrap_or_default();
            // Chief ✓N leads line one when it fits beside the age, else it opens line two.
            let chief_count = self
                .chief_answers
                .get(&session.session)
                .map(|answers| format!("Chief ✓{}", answers.count));
            let (chief_on_line_one, chief_on_line_two) = match chief_count {
                Some(count)
                    if width_of(&count) + 2 + width_of(&card.age)
                        <= session_card_right_room(self.width, 2) =>
                {
                    (Some(count), None)
                }
                other => (None, other),
            };
            let mut right: Vec<Part> = chief_on_line_one
                .into_iter()
                .map(|count| part(count, dim(FG_DARK)))
                .collect();
            right.extend(
                updates
                    .iter()
                    .filter(|update| {
                        update.session == session.session
                            && update.kind == crate::continuity::UpdateKind::Question
                    })
                    .map(|_| part("new question", style(MAGENTA))),
            );
            // The badges give way before the age does.
            let room = session_card_right_room(self.width, 2);
            let parts_width = |parts: &[Part]| {
                parts
                    .iter()
                    .map(|part| width_of(&part.text) + 2)
                    .sum::<usize>()
            };
            let mut badges: Vec<Part> = if session.restarting {
                Vec::new()
            } else {
                self.card_badges(session)
            }
            .into_iter()
            .take(CARD_BADGES_SHOWN)
            .map(|badge| part(badge, style(FG_DARK)))
            .collect();
            while !badges.is_empty()
                && parts_width(&right) + parts_width(&badges) + width_of(&card.age) > room
            {
                badges.pop();
            }
            right.extend(badges);
            right.push(part(
                card.age.clone(),
                if card.stale { style(FG) } else { dim(FG_DARK) },
            ));
            lines.push(Line {
                segments: vec![
                    segment(0, card.glyph, bold(card.tint)),
                    segment(2, card.doing.clone(), text_style),
                ],
                right,
                item: Some(index),
                accent,
                ..Line::default()
            });
            let mut why = why_segments(
                &session.project,
                &sidebar::shown(self.session_display_title(session)),
            );
            if let Some(count) = chief_on_line_two {
                let shift = width_of(&count) as u16 + 3;
                for segment in &mut why {
                    segment.0 += shift;
                }
                why.insert(0, segment(2, format!("{count} · "), dim(FG_DARK)));
            }
            if session.pinned {
                why.insert(0, segment(0, "⚲", dim(FG_DARK)));
            }
            lines.push(Line {
                segments: why,
                item: Some(index),
                accent,
                ..Line::default()
            });
            if let Some((ask, verb)) = &card.ask {
                lines.push(ask_bar(
                    Some(index),
                    ask,
                    &card.ask_count,
                    &format!("⏎ {verb}"),
                ));
            }
            if selected_id.as_deref() == Some(session.session.as_str()) && session.asleep.is_none()
            {
                let meta = sidebar::shown(&self.card_meta(session));
                for text in wrap_detail_text(&meta, (self.width as usize).saturating_sub(8).max(8))
                    .into_iter()
                    .take(2)
                {
                    lines.push(Line {
                        segments: vec![segment(2, text, dim(FG_DARK))],
                        item: Some(index),
                        accent,
                        ..Line::default()
                    });
                }
            }
            lines.push(Line::default());
        }
        if rows.is_empty() {
            lines.push(Line::new(vec![segment(
                0,
                "No running session.",
                dim(FG_DARK),
            )]));
            lines.push(Line::new(vec![segment(
                0,
                format!(
                    "a  start one in a project{}",
                    link_notice
                        .map(|notice| format!(" · {notice}"))
                        .unwrap_or_default()
                ),
                dim(FG_DARK),
            )]));
        } else if let Some(notice) = link_notice {
            lines.push(Line::new(vec![segment(0, notice, style(FG_DARK))]));
        }
        self.sessions = running;
        // In the list's order, so walking the sessions follows what Theo sees.
        let row_position = |session: &Session| {
            rows.iter().position(|row| matches!(row, SidebarRow::Session { session_id } if *session_id == session.session)).unwrap_or(usize::MAX)
        };
        self.sessions.sort_by_key(|session| row_position(session));
        self.sidebar_rows = rows;
        self.sidebar_session_tasks = match &self.team_links {
            TeamLinks::Ready(snapshot) => self
                .sessions
                .iter()
                .filter_map(|session| {
                    sidebar::member_task(snapshot, session)
                        .map(|task| (session.session.clone(), task))
                })
                .collect(),
            _ => HashMap::new(),
        };
        let panel = &mut self.panels[1];
        let selected_index = selected_id.as_ref().and_then(|id| self.sidebar_rows.iter().position(|row| matches!(row, SidebarRow::Session { session_id } if session_id == id)))
            .or_else(|| selected_row.as_ref().and_then(|selected| self.sidebar_rows.iter().position(|row| row == selected)))
            .or_else(|| selected_team.as_ref().and_then(|task| self.sidebar_rows.iter().position(|row| matches!(row, SidebarRow::Team { task: candidate } if candidate == task))))
            .or_else(|| selected_row.is_none().then(|| self.sidebar_rows.iter().position(|row| matches!(row, SidebarRow::Session { .. }))).flatten());
        if selected_team.is_some() && selected_index.is_none() {
            self.sidebar_selection_lost = true;
        }
        if let Some(index) = selected_index {
            panel.selected = index;
        }
        panel.items = self.sidebar_rows.len();
        panel.clamp();
        panel.lines = lines;
        self.panels[1].fixed_top = self.session_summary_lines();
        self.panels[1].fixed_bottom = self.selected_session_detail_lines();
        if selected_team.is_none()
            && selected_id.as_ref().is_some_and(|id| {
                !self
                    .all_sessions()
                    .iter()
                    .any(|session| session.session == *id)
            })
            && selected_index.is_none()
            && !is_viewer_loading()
        {
            // Viewing an asleep card wakes it, and moving the selection never does.
            if let Some(session) = self
                .selected_session()
                .filter(|session| session.asleep.is_none())
            {
                let error = self.manager_async(&["view", &session.session]);
                if !error.is_empty() {
                    self.message = error;
                }
            }
        }
    }

    /// Claude's incident, sessions paused for a game, a full disk, sessions to restore: in this order.
    pub(super) fn current_notices(&self, paused: usize) -> Vec<Notice> {
        let mut notices = Vec::new();
        let (claude, _) = self.claude_status.snapshot();
        if let Some(claude) = claude.filter(|c| !c.indicator.is_empty() && c.indicator != "none") {
            let mut details: Vec<String> = claude
                .degraded
                .iter()
                .take(2)
                .map(|(name, state)| format!("{name}: {state}"))
                .collect();
            details.extend(claude.incidents.iter().take(1).cloned());
            let tint = if claude.indicator == "maintenance" {
                BLUE
            } else {
                RED
            };
            notices.push(Notice {
                glyph: "!",
                tint,
                text: format!("Claude: {}", claude.description),
                key: "s status",
                detail: details.join(" · "),
            });
        }
        if paused > 0 {
            notices.push(Notice {
                glyph: "‖",
                tint: BLUE,
                text: format!(
                    "{paused} session{} paused while you game · resume after",
                    plural(paused)
                ),
                key: "",
                detail: "they resume where they stopped when the game ends".into(),
            });
        }
        if let Some((mount, used)) = self
            .disk
            .as_ref()
            .filter(|(_, used)| *used >= DISK_WARNING_PERCENT)
        {
            notices.push(Notice {
                glyph: "▲",
                tint: RED,
                text: format!("Disk {used:.0}% full on {mount}"),
                key: "",
                detail: "make -C ~/dotfiles disk shows what uses it".into(),
            });
        }
        if !self.restore.is_empty() {
            let names = self
                .restore
                .iter()
                .map(|r| {
                    if r.project.is_empty() {
                        "?"
                    } else {
                        r.project.as_str()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            notices.push(Notice {
                glyph: "↺",
                tint: BLUE,
                text: format!(
                    "{} session{} ran before the restart",
                    self.restore.len(),
                    plural(self.restore.len())
                ),
                key: "B restore",
                detail: format!("{names}: B brings them back, one after the other"),
            });
        }
        notices
    }

    pub(super) fn session_attention(&self, session: &Session, now: i64) -> Attention {
        if self.is_actionable(session, now) {
            return Attention::NeedsYou;
        }
        if session.asleep.is_some() || session.needs_attention() {
            return Attention::Quiet;
        }
        match sessions::activity_label(session) {
            "Working" | "Command running" | "Restarting" => Attention::Working,
            _ => Attention::Quiet,
        }
    }

    pub(super) fn session_card(&self, session: &Session, now: i64) -> Card {
        let group = self.session_attention(session, now);
        let doing_now = sessions::doing_now(session, now);
        let doing_text = doing_now
            .as_ref()
            .map(|doing| crate::teams::sidebar::shown(&doing.text));
        let stale_for = doing_now.as_ref().and_then(|doing| doing.stale_for);
        let mut card = Card {
            group,
            glyph: "·",
            tint: FG_DARK,
            doing: String::new(),
            age: util::age(session.since),
            stale: false,
            bright: false,
            ask: None,
            ask_count: String::new(),
        };
        if let Some(slept_at) = session.asleep {
            (card.glyph, card.tint, card.doing, card.age) =
                ("☾", GREEN, "Asleep".into(), util::age(slept_at));
            return card;
        }
        if group == Attention::NeedsYou {
            let mut asks: Vec<(String, &'static str)> = Vec::new();
            match session.ask_to_run.as_str() {
                "approval" => asks.push(("Approve a command".into(), "review")),
                "input" => asks.push(("A running command needs your input".into(), "answer")),
                _ => {}
            }
            let request = util::collapse(&sessions::waiting_request(session));
            if !request.is_empty() {
                asks.push((crate::teams::sidebar::shown(&request), "answer"));
            } else if session.state == "needs-you" && asks.is_empty() {
                asks.push(("An answer or permission is needed".into(), "answer"));
            }
            if asks.is_empty() {
                asks.push(("Answer a question".into(), "answer"));
            }
            if asks.len() > 1 {
                card.ask_count = format!("1 of {}", asks.len());
            }
            card.ask = asks.into_iter().next();
            (card.glyph, card.tint) = ("!", YELLOW);
            card.doing = doing_text.unwrap_or_else(|| "Waiting for you".into());
            return card;
        }
        if session.restarting {
            (card.glyph, card.tint, card.doing) = ("⟳", BLUE, "Restarting…".into());
            return card;
        }
        if let Some((badge, detail)) = self.session_dependency_activity(session) {
            card.glyph = "↳";
            card.doing = detail
                .replacen("Waiting on TODO", "Waiting on", 1)
                .replacen("Blocked by TODO", "Blocked by", 1);
            if badge == "Waiting on TODO" {
                card.age = "queued".into();
            } else {
                card.doing = "Blocked: its todo isn't running · r run it".into();
            }
            return card;
        }
        if let Some(expiry) = self.session_deferred_until(session, now) {
            let minutes = ((expiry - now).max(0) + 59) / 60;
            (card.glyph, card.doing) = ("‖", format!("Later · back in {minutes}m"));
            return card;
        }
        match sessions::activity_label(session) {
            "Chief is reviewing" => card.doing = "Chief is reviewing".into(),
            "Command running" => {
                (card.glyph, card.tint, card.doing) =
                    ("▶", BLUE, "Running your approved command".into());
            }
            "Working" => {
                (card.glyph, card.tint) = ("◐", BLUE);
                card.doing = doing_text.unwrap_or_else(|| "Working".into());
                if let Some(stale_for) = stale_for {
                    (card.age, card.stale) =
                        (format!("no update {}", util::duration(stale_for)), true);
                } else if let Some(doing) = &doing_now {
                    card.age = util::age(doing.at);
                }
            }
            "Ready" if self.has_new_result(session) => {
                (card.glyph, card.tint, card.doing, card.bright) =
                    ("●", GREEN, "Finished · new result".into(), true);
            }
            "Ready" if sessions::is_idle(session, now) => {
                card.glyph = "○";
                card.doing = format!(
                    "Idle {} · waiting for your next message",
                    util::duration(now - session.since)
                );
            }
            "Ready" => {
                (card.glyph, card.tint) = ("✓", GREEN);
                let outcome = session
                    .last_result
                    .as_ref()
                    .map(|result| result.outcome.trim())
                    .filter(|outcome| !outcome.is_empty())
                    .or(Some(session.outcome.trim()).filter(|outcome| !outcome.is_empty()));
                card.doing = match outcome {
                    Some(outcome) => {
                        format!("Finished · {}", crate::teams::sidebar::shown(outcome))
                    }
                    None => "Finished".into(),
                };
            }
            _ if session.since > 0 => {
                card.doing = format!(
                    "State unknown (no report for {})",
                    util::duration(now - session.since)
                )
            }
            _ => card.doing = "State unknown (no report yet)".into(),
        }
        card
    }

    /// A result the session finished that Theo has not opened yet (continuity updates).
    pub(super) fn has_new_result(&self, session: &Session) -> bool {
        self.continuity.as_ref().is_ok_and(|continuity| {
            continuity.updates().iter().any(|update| {
                update.session == session.session
                    && update.kind == crate::continuity::UpdateKind::Result
            })
        })
    }

    /// A card's line-1 badges, the most urgent first: restart needed, unpushed, queued, update, API credits.
    pub(super) fn card_badges(&self, session: &Session) -> Vec<String> {
        let mut badges = Vec::new();
        if !session.is_external()
            && !sessions::changed_since_start(session, &self.start_files).is_empty()
        {
            badges.push("⟳".to_string());
        }
        let unpushed = if session.unpushed > 0 {
            session.unpushed as usize
        } else {
            self.running_copy_unpushed(&session.cwd)
        };
        if unpushed > 0 {
            badges.push(format!("↑{unpushed}"));
        }
        let queued = self
            .session_queued
            .get(&session.session)
            .copied()
            .unwrap_or(0);
        if queued > 0 {
            badges.push(format!("✉{queued}"));
        }
        if session.update {
            badges.push("⬆".into());
        }
        if session.credits {
            badges.push("$".into());
        }
        badges
    }

    /// The selected card's meta line: agent, model, effort, context, background work, remote, terminal,
    /// canvas and billing. None of these sit on a card's first line.
    pub(super) fn card_meta(&self, session: &Session) -> String {
        let facts = &session.facts;
        let mut parts = vec![session.agent().label.to_string()];
        if !facts.model.is_empty() {
            parts.push(facts.model.clone());
        }
        if !facts.effort.is_empty() {
            parts.push(facts.effort.clone());
        }
        if let Some(context) = facts.context {
            parts.push(format!("ctx {}%", context.round()));
        }
        parts.extend(sessions::background_badge(session));
        if facts.remote {
            parts.push("⇅ remote".into());
        }
        if session.terminal {
            parts.push("▭ terminal".into());
        }
        if let Some(design) = &session.design {
            parts.push(format!("◆ {}", design.title));
        }
        parts.push(if session.credits {
            "$ API".into()
        } else {
            "subscription".into()
        });
        parts.join(" · ")
    }

    /// The todo a blocked card waits on that nobody runs: `r` on the card starts it.
    pub(super) fn stopped_blocker(&self, session: &Session) -> Option<todo::Item> {
        let branch_id = self
            .session_branches
            .get(&session.session)
            .and_then(|branch| branch.strip_prefix("todo-"));
        self.todos
            .items
            .iter()
            .filter(|item| {
                item.status() != "done"
                    && (item.session() == session.session
                        || (branch_id == Some(item.id().as_str())
                            && item.project() == session.project))
            })
            .flat_map(|item| item.after())
            .find_map(|id| {
                self.todos
                    .items
                    .iter()
                    .find(|item| {
                        item.id() == id && !matches!(item.status().as_str(), "done" | "doing")
                    })
                    .cloned()
            })
    }

    pub(super) fn session_dependency_activity(
        &self,
        session: &Session,
    ) -> Option<(&'static str, String)> {
        if session.asleep.is_some()
            || session.needs_attention()
            || sessions::activity_label(session) != "Ready"
        {
            return None;
        }
        let branch_id = self
            .session_branches
            .get(&session.session)
            .and_then(|branch| branch.strip_prefix("todo-"));
        let mut seen = HashSet::new();
        let mut blockers: Vec<_> = self
            .todos
            .items
            .iter()
            .filter(|item| {
                item.status() != "done"
                    && (item.session() == session.session
                        || (branch_id == Some(item.id().as_str())
                            && item.project() == session.project))
            })
            .flat_map(|item| item.after())
            .filter(|id| seen.insert(id.clone()))
            .filter_map(
                |id| match self.todos.items.iter().find(|item| item.id() == id) {
                    Some(item) if item.status() == "done" => None,
                    Some(item) => Some((item.status() == "doing", util::collapse(&item.text()))),
                    None => Some((false, id)),
                },
            )
            .collect();
        blockers.sort_by_key(|(is_running, _)| *is_running);
        let (is_running, title) = blockers.first()?;
        let (badge, prefix) = if *is_running {
            ("Waiting on TODO", "Waiting on TODO")
        } else {
            ("Blocked · TODO not running", "Blocked by TODO")
        };
        let mut detail = format!("{prefix}: {title}");
        if blockers.len() > 1 {
            detail += &format!(" · +{} more", blockers.len() - 1);
        }
        Some((badge, detail))
    }

    pub(super) fn session_task_title(&self, session: &Session, branch: &str) -> String {
        let title = util::collapse(&session.title);
        if !title.is_empty() {
            return title;
        }
        if let Some(title) = branch
            .strip_prefix("todo-")
            .and_then(|id| {
                self.todos
                    .items
                    .iter()
                    .find(|item| item.id() == id && item.project() == session.project)
            })
            .map(|item| util::collapse(&item.text()))
            .filter(|title| !title.is_empty())
        {
            return title;
        }
        if branch.is_empty() {
            session.project.clone()
        } else {
            branch.into()
        }
    }

    pub(super) fn session_display_title<'a>(&'a self, session: &'a Session) -> &'a str {
        if !session.label.is_empty() {
            return &session.label;
        }
        self.session_task_titles
            .get(&session.session)
            .map(String::as_str)
            .unwrap_or(&session.project)
    }

    pub(super) fn selected_session_detail_lines(&self) -> Vec<Line> {
        use crate::teams::sidebar::{self, SidebarRow};
        if self.sidebar_selection_lost {
            return vec![Line::new(vec![segment(
                0,
                "Selected member unavailable · choose a row",
                style(YELLOW),
            )])];
        }
        if let (
            TeamLinks::Ready(snapshot),
            Some(SidebarRow::Team { task } | SidebarRow::PlannedQa { task }),
        ) = (
            &self.team_links,
            self.sidebar_rows.get(self.panels[1].selected),
        ) {
            return vec![
                Line::new(vec![segment(
                    0,
                    format!("Selected team: {}", self.team_task_title(snapshot, task)),
                    bold(CYAN),
                )]),
                Line::new(vec![segment(
                    0,
                    sidebar::shown(&format!("{} / {}", task.project, task.task_id)),
                    style(FG_DARK),
                )]),
                Line::new(vec![segment(
                    0,
                    sidebar::attention(snapshot, task)
                        .unwrap_or_else(|| sidebar::handoff_summary(snapshot, task)),
                    style(FG_DARK),
                )]),
            ];
        }
        let Some(selected) = self.selected_session() else {
            return vec![];
        };
        let session = &selected;
        let branch = self
            .session_branches
            .get(&session.session)
            .map(String::as_str)
            .unwrap_or("");
        let task_title = self
            .session_task_titles
            .get(&session.session)
            .map(String::as_str)
            .unwrap_or(&session.project);
        let mut lines = Vec::new();
        let needs_attention = session.needs_attention();
        let facts = &session.facts;
        let mut badges = Vec::new();
        if !session.ask_to_run.is_empty() {
            let tint = if session.ask_to_run == "running" {
                BLUE
            } else {
                YELLOW
            };
            badges.push(part(
                format!("ask-to-run: {}", session.ask_to_run),
                bold(tint),
            ));
        }
        if session.is_external() {
            badges.push(part("external", bold(CYAN)));
        }
        if !session.is_external()
            && facts
                .context
                .is_some_and(|context| context >= CONTEXT_COMPACT_AT)
        {
            badges.push(part("/compact", bold(BLUE)));
        }
        if session.update {
            badges.push(part("⬆ update", bold(GREEN)));
        }
        if !session.is_external()
            && !sessions::changed_since_start(session, &self.start_files).is_empty()
        {
            badges.push(part("⟳ restart", bold(BLUE)));
        }
        if facts.remote {
            badges.push(part("⇅ remote", bold(CYAN)));
        }
        if session.terminal {
            badges.push(part("▭ terminal", bold(MAGENTA)));
        }
        if session.credits {
            badges.push(part("$ credits", dim(FG_DARK)));
        }
        if let Some(badge) = session.agent().badge {
            badges.push(part(badge, bold(BLUE)));
        }
        match session.role.as_str() {
            "designer" => badges.push(part("✎ designer", bold(MAGENTA))),
            "developer" => badges.push(part("⌨ developer", style(CYAN))),
            _ => {}
        }
        if let Some(team) = self.team_roles.get(&session.session) {
            badges.push(part(team.clone(), bold(MAGENTA)));
        }
        // Launchers attach every craft server; show only instances serving this checkout.
        for app in session.apps.split_whitespace() {
            let Some((name, "on")) = app.split_once(':') else {
                continue;
            };
            let serves_it = self.craft_roots.get(name).is_none_or(|roots| {
                self.session_roots
                    .get(&session.session)
                    .is_some_and(|root| roots.contains(root))
            });
            if serves_it {
                badges.push(part(format!("◆ {name}"), bold(GREEN)));
            }
        }
        if let Some(background) = sessions::background_badge(session) {
            badges.push(part(background, bold(BLUE)));
        }
        let queued = self
            .session_queued
            .get(&session.session)
            .copied()
            .unwrap_or(0);
        if queued > 0 {
            badges.push(part(format!("✉ {queued} queued"), bold(CYAN)));
        }
        if session.unpushed > 0 {
            let count = if sessions::is_reviewed_clone(&session.cwd) {
                "unapplied"
            } else {
                "unpushed"
            };
            badges.push(part(format!("↑{} {count}", session.unpushed), bold(BLUE)));
        }
        let running_copy_unpushed = self.running_copy_unpushed(&session.cwd);
        if running_copy_unpushed > 0 {
            badges.push(part(
                format!("↑{running_copy_unpushed} unpushed"),
                bold(BLUE),
            ));
        }
        if session.pinned {
            badges.push(part("⚲", dim(FG_DARK)));
        }
        if let Some(answers) = self.chief_answers.get(&session.session) {
            badges.push(part(format!("Chief ✓{}", answers.count), dim(FG_DARK)));
        }
        badges.push(part(age(session.since), dim(FG_DARK)));
        let mut right = Vec::new();
        if session.state != "unknown" {
            if !session.is_external() {
                let effort = if facts.effort.is_empty() {
                    "effort"
                } else {
                    facts.effort.as_str()
                };
                right.push(Part {
                    text: format!("{effort} ▾"),
                    style: style(CYAN),
                    action: Some("effort"),
                });
            }
            right.push(part(&facts.model, dim(FG_DARK)));
            right.push(part(
                facts
                    .context
                    .map(|c| format!("{}%", c.round()))
                    .unwrap_or_default(),
                bold(context_color(facts.context)),
            ));
        }
        right.retain(|p| !p.text.is_empty());
        lines.push(Line {
            segments: vec![segment(0, format!("Selected: {task_title}"), bold(BLUE))],
            right: vec![Part {
                text: "i Full details".into(),
                style: style(CYAN),
                action: Some("session-details"),
            }],
            ..Line::default()
        });
        let location = if branch.is_empty() {
            session.project.clone()
        } else {
            format!("{} / {branch}", session.project)
        };
        lines.push(Line {
            segments: vec![segment(0, location, dim(FG_DARK))],
            right,
            ..Line::default()
        });
        if let Ok(continuity) = &self.continuity
            && let Some(note) = continuity.note(&session.session)
        {
            lines.push(Line::new(vec![segment(
                0,
                format!("Your note: {note}"),
                style(CYAN),
            )]));
        }
        if let Some(design) = &session.design {
            badges.push(part(format!("◆ {}", design.title), style(MAGENTA)));
        }
        let extra = if needs_attention {
            sessions::activity_text(session)
        } else if session.state == "waiting" && !session.outcome.is_empty() {
            format!("Last turn: {}", util::collapse(&session.outcome))
        } else {
            String::new()
        };
        let wrapped = wrap_detail_text(&extra, (self.width as usize).saturating_sub(5).max(2));
        for index in 0..2 {
            let mut text = wrapped.get(index).cloned().unwrap_or_default();
            if index == 1 && wrapped.len() > 2 {
                text = format!("{}…", fit(&text, (self.width as usize).saturating_sub(6)));
            }
            lines.push(Line::new(vec![segment(
                0,
                text,
                if needs_attention {
                    style(YELLOW)
                } else {
                    dim(FG_DARK)
                },
            )]));
        }
        lines.push(Line {
            right: badges,
            ..Line::default()
        });
        if let TeamLinks::Ready(snapshot) = &self.team_links
            && let Some(member) = sidebar::membership(snapshot, session)
            && let Some(task) = sidebar::task_of(member)
        {
            let role = sidebar::role(member)
                .map(sidebar::role_label)
                .unwrap_or("Member");
            lines.insert(
                0,
                Line::new(vec![segment(
                    0,
                    format!(
                        "{} / {role} · {}",
                        self.team_task_title(snapshot, &task),
                        self.session_display_title(session)
                    ),
                    bold(CYAN),
                )]),
            );
            lines.push(Line::new(vec![segment(
                0,
                format!("{} · {} · {}", session.agent, session.session, task.task_id),
                style(FG_DARK),
            )]));
            lines.push(Line::new(vec![segment(
                0,
                sidebar::handoff_summary(snapshot, &task),
                style(FG_DARK),
            )]));
            for line in &mut lines {
                for (_, text, _) in &mut line.segments {
                    *text = sidebar::shown(text);
                }
                for part in &mut line.right {
                    part.text = sidebar::shown(&part.text);
                }
            }
        }
        lines
    }

    pub(super) fn session_summary_lines(&self) -> Vec<Line> {
        let now = util::now_secs();
        // One bucket per session, in the cards' precedence, so the counts cannot overlap.
        let (
            mut needing,
            mut working,
            mut ready,
            mut idle,
            mut asleep,
            mut unknown,
            mut waiting,
            mut blocked,
        ) = (0, 0, 0, 0, 0, 0, 0, 0);
        for session in self.all_sessions() {
            if session.needs_attention() {
                if self.session_deferred_until(&session, now).is_none() {
                    needing += 1;
                }
            } else if let Some((badge, _)) = self.session_dependency_activity(&session) {
                if badge == "Waiting on TODO" {
                    waiting += 1;
                } else {
                    blocked += 1;
                }
            } else if sessions::is_idle(&session, now) {
                idle += 1;
            } else if session.asleep.is_some() {
                asleep += 1;
            } else {
                match sessions::activity_label(&session) {
                    "Working" | "Command running" => working += 1,
                    "Ready" => ready += 1,
                    _ => unknown += 1,
                }
            }
        }
        needing += usize::from(!self.held.is_empty());
        let mut attention: String = if needing == 0 {
            "Nothing needs you".into()
        } else {
            format!("! {needing} need you")
        };
        if let Ok(continuity) = &self.continuity {
            let unseen = continuity.updates().len();
            if unseen > 0 {
                attention += &format!(" · {unseen} unseen");
            }
            let deferred = continuity.deferred_count(util::now_secs());
            if deferred > 0 {
                attention += &format!(" · {deferred} deferred");
            }
        }
        let mut counts = format!("{working} working · {ready} ready");
        if waiting > 0 {
            counts += &format!(" · {waiting} waiting on TODO");
        }
        if blocked > 0 {
            counts += &format!(" · {blocked} blocked");
        }
        if idle > 0 {
            counts += &format!(" · {idle} idle");
        }
        if asleep > 0 {
            counts += &format!(" · {asleep} asleep");
        }
        if unknown > 0 {
            counts += &format!(" · {unknown} unknown");
        }
        let outdated = self
            .running_sessions
            .iter()
            .filter(|session| !sessions::changed_since_start(session, &self.start_files).is_empty())
            .count();
        if outdated > 0 {
            counts += &format!(" · {outdated} restart available");
        }
        if self.team_attention > 0 {
            counts += &format!(
                " · team: {} {} you (4 Teams)",
                self.team_attention,
                if self.team_attention == 1 {
                    "needs"
                } else {
                    "need"
                }
            );
        }
        vec![
            Line::new(vec![segment(
                0,
                attention,
                bold(if needing > 0 { YELLOW } else { FG_DARK }),
            )]),
            Line {
                segments: vec![segment(0, counts, dim(FG_DARK))],
                right: vec![Part {
                    text: "n Next needing me".into(),
                    style: style(if needing > 0 { YELLOW } else { FG_DARK }),
                    action: (needing > 0).then_some("next-attention"),
                }],
                ..Line::default()
            },
        ]
    }

    pub(crate) fn sessions_hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.selected_session().is_none()
            && self.sidebar_rows.get(self.panels[1].selected).is_some()
        {
            return vec![
                ("⏎", "fold/unfold team"),
                ("n", "Next needing me"),
                ("i", "team details"),
            ];
        }
        let selected = self.selected_session().unwrap_or_default();
        let mut hints = Vec::new();
        if selected.update {
            hints.push(("R", "restart to apply the update"));
        } else if !sessions::changed_since_start(&selected, &self.start_files).is_empty() {
            hints.push(("R", "restart to pick up the change"));
        }
        if selected
            .facts
            .context
            .is_some_and(|c| c >= CONTEXT_COMPACT_AT)
        {
            hints.push(("C", "/compact"));
        }
        if selected.design.is_some() {
            hints.push(("o", "canvas"));
        }
        if sessions::is_reviewed_clone(&selected.cwd)
            && (selected.unpushed > 0 || self.running_copy_unpushed(&selected.cwd) > 0)
        {
            hints.push((
                "U",
                if selected.unpushed > 0 {
                    "apply"
                } else {
                    "push"
                },
            ));
        } else if selected.unpushed > 0 {
            hints.push(("U", "push"));
        }
        hints.extend([("⏎", "open"), ("n", "Next needing me"), ("i", "details")]);
        if self.chief_answers.contains_key(&selected.session) {
            hints.push(("c", "Chief answers"));
        }
        hints.extend([
            ("e", "effort"),
            ("g", "git"),
            ("+", "another"),
            ("x", "exit"),
        ]);
        if !selected.session.is_empty() && selected.is_external() {
            hints.retain(|(key, _)| {
                !session_key_requires_terminal(if *key == "⏎" {
                    Key::Enter
                } else {
                    Key::Char(key.chars().next().unwrap())
                })
            });
            if self.attention_indices(util::now_secs()).is_empty() {
                hints.retain(|(key, _)| *key != "n");
            }
        }
        let mut seen = std::collections::HashSet::new();
        hints.retain(|(key, _)| seen.insert(*key));
        let agent = selected.agent();
        hints.retain(|(key, _)| {
            key.chars()
                .next()
                .and_then(|key| key_feature(Key::Char(key), agent))
                .is_none_or(|(feature, _)| agent.can(feature))
        });
        hints
    }

    pub(super) fn sessions_help() -> Vec<(&'static str, &'static str)> {
        vec![
            ("⏎", "open the selected session"),
            ("n", "next session that needs you"),
            ("l", "later: hide this ask for 20 min (never approves)"),
            ("i", "details of the selected session"),
            ("I", "open the images from its latest reply in a viewer"),
            ("N", "new session: blank or Claude Design canvas"),
            ("R", "restart the session"),
            ("x", "end the session"),
            ("p", "pin: first inside its group"),
            ("r", "on a blocked card: run the todo it waits on"),
            (
                "⌘R",
                "restart every session that lacks a change (⟳), once it waits",
            ),
            (
                "B",
                "after a reboot: bring back the sessions that were running",
            ),
            (
                "u",
                "updates: unseen results, questions and archived return notes",
            ),
            ("Q", "queue: what is waiting and why"),
            (
                "v",
                "mark displayed updates reviewed; never answer or approve",
            ),
            (
                "",
                "its other keys (effort, push, note, sleep…) are listed in i and work on the card too",
            ),
        ]
    }

    pub(crate) fn handle_sessions(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        use crate::teams::sidebar::{self, SidebarRow};
        match key {
            Key::Char('u') => return self.run_continuity_action("updates", screen),
            Key::Char('Q') => return self.run_continuity_action("queue", screen),
            Key::Char('v') => return self.run_continuity_action("reviewed", screen),
            _ => {}
        }
        if key == Key::Char('n') {
            return self.next_sidebar_attention();
        }
        if !self.sidebar_selection_lost
            && let Some(SidebarRow::Team { task } | SidebarRow::PlannedQa { task }) =
                self.sidebar_rows.get(self.panels[1].selected).cloned()
        {
            if key == Key::Char('i') {
                return self.open_teams_tab_on(&task);
            }
            if key == Key::Enter
                && matches!(
                    self.sidebar_rows.get(self.panels[1].selected),
                    Some(SidebarRow::Team { .. })
                )
            {
                let key = sidebar::task_key(&task);
                if !self.collapsed_teams.remove(&key) {
                    self.collapsed_teams.insert(key);
                }
                self.build_sessions();
                return String::new();
            }
        }
        let session = self.selected_session();
        if let Some(s) = session.as_ref().filter(|s| s.restarting)
            && matches!(
                key,
                Key::Enter | Key::Char('r' | 'R' | 'e' | '$' | 'C' | 'x' | 'z')
            )
        {
            return format!("{} is restarting: wait for it to come back", s.project);
        }
        if session
            .as_ref()
            .is_some_and(|session| session.is_external())
            && session_key_requires_terminal(key.clone())
        {
            return "external session: no terminal target; use its own window".into();
        }
        if let Some(s) = session.as_ref()
            && let Some((feature, what)) = key_feature(key.clone(), s.agent())
        {
            let agent = s.agent();
            if !agent.can(feature) {
                return format!(
                    "{what} is not {}'s to do: {} runs {}, do it in its pane",
                    agent.label, s.project, agent.name
                );
            }
        }
        // On a blocked card `r` runs the todo it waits on; on the others it still restarts (as `R`).
        let blocker = if key == Key::Char('r') {
            session
                .as_ref()
                .filter(|s| {
                    self.session_dependency_activity(s)
                        .is_some_and(|(badge, _)| badge != "Waiting on TODO")
                })
                .and_then(|s| self.stopped_blocker(s))
        } else {
            None
        };
        if let Some(item) = blocker {
            if item.status() == todo::PROPOSED {
                return "its todo is still a proposal: approve it in Ideas first".into();
            }
            let title = util::collapse(&item.text());
            if !self.confirm(
                screen,
                &format!("run its todo \"{title}\" now? a new session starts on it"),
            ) {
                return String::new();
            }
            return self.hand_to_new_session(&Entry::Typed(item), screen);
        }
        let known = session.as_ref().filter(|s| s.state != "unknown").cloned();
        match key {
            Key::Enter => session
                .map(|s| {
                    let opened = self.manager_async(&["open", &s.session]);
                    self.mark_result_seen(&s.session).unwrap_or(opened)
                })
                .unwrap_or_default(),
            Key::Char('i') => {
                self.open_session_details();
                String::new()
            }
            Key::Char('I') => session.map(|s| open_reply_images(&s)).unwrap_or_default(),
            Key::Char('c') => match session {
                Some(s) => self.open_chief_answers(Some(&s.session)),
                None => String::new(),
            },
            Key::Char('p') => {
                if let Some(s) = known {
                    self.view.toggle_pin(&s.session);
                }
                String::new()
            }
            Key::Char('l') => self.toggle_later(),
            Key::Char('t') => match session {
                Some(s) => self.open_task_trail_of_session(&s.session, screen),
                None => String::new(),
            },
            Key::Char('w') => match session {
                Some(s) => self.write_note(&s, screen),
                None => String::new(),
            },
            Key::Char('m') => self.toggle_remote(),
            Key::Char('d') => self.drop_queued(screen),
            Key::Char('k') => self.open_team_view(),
            Key::Char('r') => {
                "R restarts the session; r only runs the todo a blocked card waits on".into()
            }
            Key::Char('R') => match known {
                Some(s)
                    if self.confirm(
                        screen,
                        &format!("restart {}? same conversation, new process", s.project),
                    ) =>
                {
                    manager_in_background(&["restart", &s.session]);
                    format!("restarting {} …", s.project)
                }
                _ => String::new(),
            },
            Key::Ctrl('r') => self.restart_all(screen),
            Key::Char('e') => match session {
                Some(s) if s.agent().can(Feature::Variants) => {
                    self.manager_async(&["variants", &s.session])
                }
                Some(s) => self.open_effort_menu(s, None),
                None => String::new(),
            },
            Key::Char('$') => match known {
                Some(s) => {
                    let (billing, label) = if s.credits {
                        ("subscription", "the subscription")
                    } else {
                        ("credits", "API credits")
                    };
                    if !self.confirm(
                        screen,
                        &format!("restart {} on {label}? same conversation", s.project),
                    ) {
                        return String::new();
                    }
                    manager_in_background(&["billing", &s.session, billing]);
                    format!("restarting {} on {label} …", s.project)
                }
                None => String::new(),
            },
            Key::Char('B') if !self.restore.is_empty() => {
                let pairs: Vec<String> = self
                    .restore
                    .iter()
                    .flat_map(|r| [r.cwd.clone(), r.session.clone()])
                    .collect();
                let mut arguments = vec!["restore"];
                arguments.extend(pairs.iter().map(String::as_str));
                manager_in_background(&arguments);
                self.view.restored_boot = sessions::boot_time();
                self.view.save();
                let count = self.restore.len();
                self.restore.clear();
                format!(
                    "bringing back {count} session{} …",
                    if count != 1 { "s" } else { "" }
                )
            }
            Key::Char('+') => match session {
                Some(s) if s.cwd.is_empty() => "no directory known for this session".into(),
                Some(s) => {
                    let suggested = chrono::Local::now().format("parallel-%H%M").to_string();
                    let typed = input_line(
                        screen,
                        &format!(
                            "branch for another {} session (Enter: {suggested})> ",
                            s.project
                        ),
                    );
                    let branch = if typed.is_empty() { suggested } else { typed };
                    let agent = match settings::launch_agent() {
                        Ok(agent) => agent,
                        Err(error) => return error,
                    };
                    manager_in_background(&["--agent", &agent, "parallel", &s.cwd, &branch]);
                    format!("cloning {} for {branch} …", s.project)
                }
                None => String::new(),
            },
            Key::Char('C') => match known {
                Some(s) => {
                    let error = manager(&["compact", &s.session]);
                    if error.is_empty() {
                        format!("/compact sent to {}", s.project)
                    } else {
                        error
                    }
                }
                None => String::new(),
            },
            Key::Char('o') => match session.and_then(|s| s.design) {
                Some(design) => {
                    let _ = util::spawn_detached(Command::new("xdg-open").arg(&design.url));
                    format!("opening {} …", design.title)
                }
                None => "no canvas: N, then Claude Design canvas, starts a design session".into(),
            },
            Key::Char('g') => match session {
                Some(s) if s.cwd.is_empty() => "no directory known for this session".into(),
                Some(s) => {
                    manager_in_background(&["git", &s.cwd]);
                    String::new()
                }
                None => String::new(),
            },
            Key::Char('U') => match session {
                Some(s) if s.cwd.is_empty() => "no directory known for this session".into(),
                Some(s) => {
                    manager_in_background(&["push", &s.cwd]);
                    String::new()
                }
                None => String::new(),
            },
            Key::Char('x') => match session {
                Some(s) if self.confirm(screen, &format!("exit {}?", s.project)) => {
                    self.exit_session(&s)
                }
                _ => String::new(),
            },
            Key::Char('z') => match known {
                Some(s) => self.sleep_now(&s),
                None => String::new(),
            },
            _ => String::new(),
        }
    }
}
