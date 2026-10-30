use super::*;

impl App {
    /// One view at full height under the header: the focused tab (Sessions, Todo, Ideas), or the
    /// Status overlay, whose pull requests take the lower part.
    pub fn layout(&self, height: u16) -> Vec<u16> {
        let available = height.saturating_sub(HEADER_LINES + 1);
        let mut heights = vec![0; PANEL_NAMES.len()];
        if self.is_status_open() {
            let reviews = (self.panels[2].content_height() as u16 + 2)
                .clamp(MIN_PANEL_LINES, (available / 3).max(MIN_PANEL_LINES));
            heights[2] = reviews.min(available / 2);
            heights[0] = available - heights[2];
        } else {
            heights[self.focus] = available;
        }
        heights
    }

    pub fn render(&mut self, buffer: &mut Buffer) {
        self.hit_lines.clear();
        self.hit_actions.clear();
        self.tab_ranges.clear();
        let (width, height) = (buffer.area.width, buffer.area.height);
        // The Teams tab's divider is as wide as the column.
        let is_resized = self.width != width;
        self.width = width;
        if is_resized && self.focus == TEAMS_PANEL {
            self.build_teams();
        }
        self.panels[1].fixed_top = self.session_summary_lines();
        self.panels[1].fixed_bottom = self.selected_session_detail_lines();
        self.todo_fixed_lines(self.layout(height)[3]);
        self.draw_header(buffer, width);
        let mut top = HEADER_LINES;
        for (index, panel_height) in self.layout(height).into_iter().enumerate() {
            self.draw_panel(buffer, index, top, panel_height, width);
            top += panel_height;
        }
        self.draw_tabs(buffer, HEADER_LINES, width);
        self.draw_footer(buffer, height.saturating_sub(1), width);
        if let Err(error) = &self.continuity {
            put(
                buffer,
                height.saturating_sub(1),
                0,
                &format!("Continuity unavailable: {error}"),
                bold(RED),
            );
        } else if let Some(error) = &self.continuity_error {
            put(
                buffer,
                height.saturating_sub(1),
                0,
                &format!("Continuity unavailable: {error}"),
                bold(RED),
            );
        }
        if let Some(menu) = self.menu.as_mut() {
            menu.draw(buffer);
        }
        if let Some(menu) = self.settings_menu.as_mut() {
            menu.draw(buffer);
        }
        if let Some(view) = self.chief_view.as_mut().filter(|view| !view.suspended) {
            chief_ui::draw(view, buffer);
            self.hit_lines.clear();
            self.hit_actions.clear();
        }
        if let Some(view) = self.team_view.as_ref().filter(|view| !view.hidden) {
            draw_team_view(view, buffer);
            self.hit_lines.clear();
            self.hit_actions.clear();
        }
        if let Some(pairing) = self.pairing.as_ref() {
            pairing.draw(buffer);
        }
        if self.help_open {
            self.draw_help(buffer);
        }
        if let Some(details) = self.session_details.as_mut() {
            details.draw(buffer);
            self.hit_lines.clear();
            self.hit_actions.clear();
        }
        if let Some(composer) = self.todo_composer.as_ref() {
            composer.draw(buffer);
            self.hit_lines.clear();
            self.hit_actions.clear();
        }
        if let Some(form) = self.new_session.as_ref() {
            form.draw(buffer);
            self.hit_lines.clear();
            self.hit_actions.clear();
        }
        if !self.has_pane_focus {
            fade(buffer);
        }
    }

    pub(super) fn draw_panel(
        &mut self,
        buffer: &mut Buffer,
        index: usize,
        top: u16,
        height: u16,
        width: u16,
    ) {
        if height < 2 || width < 4 {
            return;
        }
        let focused = self.has_pane_focus && index == self.focus;
        let inner_height = (height - 2) as usize;
        let subtitle_text = self.subtitle(index);
        let counts = if index == 2 {
            self.review_counts()
        } else {
            Vec::new()
        };
        let inner = (width - 3) as usize;
        // No boxes: the tabs row sits over a tab's first row; pull requests get a section heading in Status.
        let label = if index == 2 {
            format!("PULL REQUESTS · {}", self.panels[2].items)
        } else {
            String::new()
        };
        if index == 2 {
            put(
                buffer,
                top,
                2,
                &label,
                bold(if focused { BLUE } else { FG_DARK }),
            );
            self.hit_lines.insert(top, (index, None));
        }
        let panel = &mut self.panels[index];
        let fixed_top_height = panel.fixed_top.len().min(if inner_height >= 4 {
            2
        } else if inner_height >= 2 {
            1
        } else {
            0
        });
        let fixed_bottom_height = if inner_height
            >= fixed_top_height + panel.fixed_bottom.len() + if index == 3 { 4 } else { 2 }
        {
            panel.fixed_bottom.len()
        } else {
            0
        };
        let scrolling_height = inner_height.saturating_sub(fixed_top_height + fixed_bottom_height);
        panel.keep_selected_visible(scrolling_height);
        let mut title_end = 2 + width_of(&label);
        let counts_width: usize = counts.iter().map(|(text, _)| 1 + width_of(text)).sum();
        if index == 2 && !counts.is_empty() && title_end + 2 + counts_width + 2 <= width as usize {
            put(buffer, top, title_end as u16, " ·", dim(FG_DARK));
            title_end += 2;
            for (text, count_style) in &counts {
                put(
                    buffer,
                    top,
                    title_end as u16,
                    &format!(" {text}"),
                    *count_style,
                );
                title_end += 1 + width_of(text);
            }
        }
        let subtitle = fit(
            &subtitle_text,
            (width as usize).saturating_sub(6 + title_end),
        );
        if index == 2 && !subtitle.is_empty() {
            put(
                buffer,
                top,
                title_end as u16,
                &format!(" · {subtitle} "),
                dim(FG_DARK),
            );
        }
        for offset in 0..inner_height {
            let row = top + 1 + offset as u16;
            let line = if offset < fixed_top_height {
                panel.fixed_top.get(offset)
            } else if offset >= fixed_top_height + scrolling_height {
                panel
                    .fixed_bottom
                    .get(offset - fixed_top_height - scrolling_height)
            } else {
                panel.lines.get(panel.scroll + offset - fixed_top_height)
            };
            let Some(line) = line else { continue };
            let mut issue_line;
            let line = if index == 3
                && let Some(Entry::Issue { issue, .. }) =
                    line.item.and_then(|item| self.todo_entries.get(item))
            {
                // Reserve a title excerpt and Q before allocating repository identity space.
                let marker_room = if line.right.iter().any(|part| part.text == "Q") {
                    3
                } else {
                    0
                };
                let identity_room = (width as usize).saturating_sub(4 + 7 + 1 + 12 + marker_room);
                let number = format!("#{}", issue.number);
                let identity = format!(
                    "{}{}",
                    fit(
                        &issue.repository,
                        identity_room.saturating_sub(width_of(&number))
                    ),
                    number
                );
                issue_line = line.clone();
                issue_line.segments[2].1 = identity.clone();
                issue_line.segments[3].0 = 8 + width_of(&identity) as u16;
                &issue_line
            } else {
                line
            };
            let highlighted = focused && line.item.is_some() && line.item == Some(panel.selected);
            let recolor = |style: Style, fg: u8| -> Style {
                if !highlighted || line.fill.is_some() {
                    return style;
                }
                if index == 0 {
                    return style.bg(color(BG_HIGHLIGHT));
                }
                let linked_member = index == 1 && line.item.and_then(|item| self.sidebar_rows.get(item)).is_some_and(|row| match row {
                    crate::teams::sidebar::SidebarRow::Session { session_id } => matches!(&self.team_links, TeamLinks::Ready(snapshot)
                        if self.sessions.iter().find(|session| session.session == *session_id).is_some_and(|session| crate::teams::sidebar::membership(snapshot, session).is_some())),
                    _ => false,
                });
                if linked_member {
                    return style
                        .fg(color(FG))
                        .bg(color(BG_HIGHLIGHT))
                        .remove_modifier(Modifier::DIM);
                }
                let preserve_state = matches!(index, 1 | TEAMS_PANEL | CHIEF_PANEL)
                    && (style.fg == Some(color(RED))
                        || style.fg == Some(color(BLUE))
                        || style.fg == Some(color(YELLOW)));
                if preserve_state {
                    style.bg(color(BG_HIGHLIGHT))
                } else {
                    style.fg(color(fg)).bg(color(BG_HIGHLIGHT))
                }
            };
            if highlighted {
                put(buffer, row, 1, &" ".repeat(inner), on(FG, BG_HIGHLIGHT));
            }
            if let Some((column, fill)) = line.fill {
                put(
                    buffer,
                    row,
                    2 + column,
                    &" ".repeat(inner.saturating_sub(2 + column as usize)),
                    fill,
                );
            }
            if let Some(accent) = line.accent {
                put(buffer, row, 1, "▌", recolor(accent, BLUE));
            }
            let mut right_width = 0;
            let right_room = if index == 1 && line.item.is_some() {
                let title_column = line
                    .segments
                    .iter()
                    .map(|(column, _, _)| *column as usize)
                    .max()
                    .unwrap_or(0);
                session_card_right_room(width, title_column)
            } else if index == 3 && line.item.is_some() {
                let title_column = line
                    .segments
                    .last()
                    .map(|segment| segment.0 as usize)
                    .unwrap_or(0);
                (width as usize).saturating_sub(4 + title_column + 12 + 2)
            } else {
                inner.saturating_sub(1)
            };
            let right: Vec<_> = line
                .right
                .iter()
                .take_while(|part| {
                    let needed = width_of(&part.text) + if right_width == 0 { 0 } else { 2 };
                    if right_width + needed > right_room {
                        return false;
                    }
                    right_width += needed;
                    true
                })
                .collect();
            for (position, (column, text, tint)) in line.segments.iter().enumerate() {
                let room = (width as i64
                    - 4
                    - *column as i64
                    - if right_width > 0 {
                        right_width as i64 + 2
                    } else {
                        0
                    })
                .max(0) as usize;
                let room = line.segments.get(position + 1).map_or(room, |next| {
                    room.min(next.0.saturating_sub(*column) as usize)
                });
                put(
                    buffer,
                    row,
                    2 + column,
                    &fit(text, room),
                    recolor(*tint, FG),
                );
            }
            let mut column = (width as i64 - 3 - right_width as i64).max(2) as u16;
            for part in right {
                put(
                    buffer,
                    row,
                    column,
                    &part.text,
                    recolor(part.style, FG_DARK),
                );
                let end = column + width_of(&part.text) as u16;
                if let Some(action) = part.action {
                    self.hit_actions.insert(row, (column, end, action));
                }
                column = end + 2;
            }
            self.hit_lines.insert(row, (index, line.item));
        }
        let hidden_below = panel.lines.len() as i64 - panel.scroll as i64 - scrolling_height as i64;
        if hidden_below > 0 && scrolling_height > 0 {
            let note = format!(" ▼ {hidden_below} ");
            put(
                buffer,
                top + height - 1,
                width - 3 - width_of(&note) as u16,
                &note,
                dim(FG_DARK),
            );
        }
        if panel.scroll > 0 {
            let note = format!(" ▲ {} ", panel.scroll);
            put(
                buffer,
                top,
                width - 3 - width_of(&note) as u16,
                &note,
                dim(FG_DARK),
            );
        }
    }

    /// One chip per plan with a 4-cell bar of what is left ("Claude Max ▮▮▯▯ 34%", red when it runs
    /// low), then ≈ $ today at API prices for all usage, then the clock. Chips give way first.
    pub(super) fn draw_header(&mut self, buffer: &mut Buffer, width: u16) {
        let mut chips: Vec<(String, Style)> = Vec::new();
        for (provider, status) in [
            ("Claude", &self.claude_status),
            ("Codex", &self.codex_status),
        ] {
            let (service, error) = status.snapshot();
            let has_outage = service.as_ref().is_some_and(|summary| {
                summary.indicator != "none"
                    || !summary.degraded.is_empty()
                    || !summary.incidents.is_empty()
            }) || !error.is_empty();
            let rows: Vec<&usage::QuotaRow> = self
                .quota_rows
                .iter()
                .filter(|row| row.label.split(" · ").next() == Some(provider))
                .collect();
            let Some(tightest) = rows.iter().min_by_key(|row| {
                (
                    match row.state.as_str() {
                        "Blocked" | "Exhausted" => 0,
                        "Low" => 1,
                        "Stale" | "Unknown" => 2,
                        _ => 3,
                    },
                    row.remaining.unwrap_or(100),
                )
            }) else {
                continue;
            };
            let name = if provider == "Claude" && !self.plan.is_empty() {
                format!("Claude {}", self.plan)
            } else {
                provider.to_string()
            };
            let is_low = matches!(tightest.state.as_str(), "Low" | "Exhausted" | "Blocked");
            // A fuel gauge of what is left, as Status's LEFT column.
            let chip = match tightest.remaining {
                Some(remaining) if matches!(tightest.state.as_str(), "Available" | "Low") => {
                    let left = remaining.clamp(0, 100);
                    let filled = ((left as f64 / 25.0).ceil() as usize).min(4);
                    format!(
                        "{name} {}{} {left}%",
                        "▮".repeat(filled),
                        "▯".repeat(4 - filled)
                    )
                }
                _ => format!("{name} {}", tightest.state.to_lowercase()),
            };
            let chip = if has_outage {
                format!("! {chip}")
            } else {
                chip
            };
            chips.push((
                chip,
                if is_low || has_outage {
                    bold(RED)
                } else {
                    style(FG)
                },
            ));
        }
        let billing = self.costs.billing_totals();
        let total_today = billing.api.today + billing.subscription.today + billing.unknown.today;
        let cost = if billing.recorded_replies > billing.unpriced_replies && total_today > 0.0 {
            format!("≈ {} today", money(total_today))
        } else {
            String::new()
        };
        let clock = chrono::Local::now().format("%H:%M").to_string();
        let mut right = width.saturating_sub(2);
        let mut right_parts: Vec<(String, Style)> = vec![(clock, style(FG_DARK))];
        if !cost.is_empty() {
            right_parts.push((cost, bold(MAGENTA)));
        }
        let mut column = 2u16;
        let mut chips_end = column;
        for (text, tint) in &chips {
            let needed = width_of(text) as u16;
            let room_for_right: u16 = right_parts
                .iter()
                .map(|(text, _)| width_of(text) as u16 + 2)
                .sum();
            if column + needed + room_for_right > width.saturating_sub(1) {
                break;
            }
            put(buffer, 0, column, text, *tint);
            column += needed + 2;
            chips_end = column.saturating_sub(2);
        }
        if chips_end > 2 {
            self.hit_actions.insert(0, (2, chips_end, "status"));
        }
        for (text, tint) in right_parts {
            let needed = width_of(&text) as u16;
            if right < column + needed {
                break;
            }
            right -= needed;
            put(buffer, 0, right, &text, tint);
            right = right.saturating_sub(2);
        }
    }

    /// The five tabs' badges: amber only for what needs you; otherwise Todo's open items, Ideas'
    /// proposals, Teams' live teams and the Chief on (green ●) or off (○).
    pub(super) fn tab_badges(&self) -> [(String, Style); 5] {
        let amber = on(BLACK, YELLOW).add_modifier(Modifier::BOLD);
        let count = |count: usize, tint: Style| {
            if count > 0 {
                (count.to_string(), tint)
            } else {
                (String::new(), tint)
            }
        };
        let needing = self
            .running_sessions
            .iter()
            .filter(|session| self.is_actionable(session, util::now_secs()))
            .count()
            + usize::from(!self.held.is_empty());
        let open_todos = self.todo_counts[TODO_TAB_TO_DO] + self.todo_counts[TODO_TAB_IN_PROGRESS];
        let teams = if self.teams_tab.needs_you > 0 {
            count(self.teams_tab.needs_you, amber)
        } else {
            count(self.teams_tab.live, style(FG_DARK))
        };
        // None: neither snapshot answered, which is not the same as off.
        let chief_badge = match (&self.team_links, &self.chief_status) {
            (_, Ok(snapshot)) if matches!(self.focus, 0 | CHIEF_PANEL) => Some((
                chief_tab::needs_you(snapshot),
                chief_tab::has_active_grant(snapshot),
            )),
            (TeamLinks::Ready(snapshot), _) if snapshot["chief_badge"].is_object() => Some((
                snapshot["chief_badge"]["needs_you"].as_u64().unwrap_or(0) as usize,
                snapshot["chief_badge"]["state"] == "active",
            )),
            _ => None,
        };
        let chief = match chief_badge {
            Some((needs_you, _)) if needs_you > 0 => count(needs_you, amber),
            Some((_, true)) => ("●".into(), style(GREEN)),
            Some((_, false)) => ("○".into(), style(FG_DARK)),
            None => ("?".into(), dim(FG_DARK)),
        };
        [
            count(needing, amber),
            count(open_todos, style(FG_DARK)),
            count(
                self.idea_entries.len(),
                on(BLACK, MAGENTA).add_modifier(Modifier::BOLD),
            ),
            teams,
            chief,
        ]
    }

    /// `1 Sessions 2  2 Todo 5  3 Ideas 3  4 Teams 1  5 Chief ●`: key caps and badges, the open tab
    /// on the selection background, its subtitle at the right when there is room. Too wide for the
    /// column, the inactive tabs fold to key cap and badge from the right; the open one never does.
    /// Amber badges go last: only below about 24 cells, the farthest from the open tab first.
    pub(super) fn draw_tabs(&mut self, buffer: &mut Buffer, row: u16, width: u16) {
        const NAMES: [&str; 5] = ["Sessions", "Todo", "Ideas", "Teams", "Chief"];
        let badges = self.tab_badges();
        let badge_width = |index: usize| {
            if badges[index].0.is_empty() {
                0
            } else {
                1 + 2 + width_of(&badges[index].0) as u16
            }
        };
        // How much of each tab shows: 3 in full, 2 key cap and badge, 1 key cap, 0 nothing.
        let mut shown = [3u8; 5];
        let tab_width = |index: usize, shown: u8| match shown {
            3 => 3 + 1 + width_of(NAMES[index]) as u16 + badge_width(index),
            2 => 3 + badge_width(index),
            1 => 3,
            _ => 0,
        };
        let total = |shown: &[u8; 5]| {
            (0..5)
                .filter(|index| shown[*index] > 0)
                .map(|index| tab_width(index, shown[index]) + 1)
                .sum::<u16>()
        };
        // Names go first, then plain badges, then tabs with nothing for Theo; an amber badge stays.
        let is_amber =
            |index: usize| !badges[index].0.is_empty() && badges[index].1.bg == Some(color(YELLOW));
        for (level, may_fold) in [
            (2, &(|_: usize| true) as &dyn Fn(usize) -> bool),
            (1, &|index| !is_amber(index)),
            (0, &|index| !is_amber(index)),
        ] {
            for index in (0..5).rev() {
                if total(&shown) <= width {
                    break;
                }
                if TAB_PANELS[index] != self.focus && may_fold(index) && shown[index] > level {
                    shown[index] = level;
                }
            }
        }
        // The open tab never goes: past that, amber tabs give way, the farthest from it first.
        let open = TAB_PANELS.iter().position(|panel| *panel == self.focus);
        let mut by_distance: Vec<usize> = (0..5).filter(|index| Some(*index) != open).collect();
        by_distance.sort_by_key(|index| {
            (
                std::cmp::Reverse(open.map_or(0, |open| index.abs_diff(open))),
                std::cmp::Reverse(*index),
            )
        });
        for index in by_distance {
            if total(&shown) <= width {
                break;
            }
            shown[index] = 0;
        }
        put(
            buffer,
            row,
            0,
            &" ".repeat(width.saturating_sub(1) as usize),
            Style::default(),
        );
        let mut column = 0u16;
        for index in (0..5).filter(|index| shown[*index] > 0) {
            let panel = TAB_PANELS[index];
            let is_open = self.focus == panel;
            let tab_width = tab_width(index, shown[index]);
            if column + tab_width >= width {
                break;
            }
            let ground = if is_open {
                on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD)
            } else {
                style(FG_DARK)
            };
            if is_open {
                put(buffer, row, column, &" ".repeat(tab_width as usize), ground);
            }
            put(
                buffer,
                row,
                column,
                &format!(" {} ", index + 1),
                on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD),
            );
            let mut end = column + 3;
            if shown[index] == 3 {
                put(buffer, row, end + 1, NAMES[index], ground);
                end += 1 + width_of(NAMES[index]) as u16;
            }
            let (badge, tint) = &badges[index];
            if !badge.is_empty() && shown[index] >= 2 {
                let tint = if is_open && tint.bg.is_none() {
                    tint.bg(color(BG_HIGHLIGHT))
                } else {
                    *tint
                };
                put(buffer, row, end + 1, &format!(" {badge} "), tint);
            }
            self.tab_ranges
                .push((column, column + tab_width + 1, panel));
            column += tab_width + 1;
        }
        // Todo shows its scope and filter on its own first lines.
        let subtitle = if self.is_status_open() {
            "Status".to_string()
        } else if self.focus == 3 {
            String::new()
        } else {
            self.subtitle(self.focus)
        };
        let has_scroll_note = self.panels[self.focus].scroll > 0;
        let room = (width as usize).saturating_sub(column as usize + 3);
        if !subtitle.is_empty() && !has_scroll_note && room > 6 {
            let text = fit(&subtitle, room);
            put(
                buffer,
                row,
                width.saturating_sub(2 + width_of(&text) as u16),
                &text,
                dim(FG_DARK),
            );
        }
        self.hit_lines.remove(&row);
    }

    pub(super) fn draw_footer(&self, buffer: &mut Buffer, row: u16, width: u16) {
        if !self.message.is_empty() {
            put(
                buffer,
                row,
                2,
                &self.message,
                style(if self.is_confirming { BLUE } else { FG }),
            );
            return;
        }
        let mut column = 2u16;
        let details = if self.focus == 3 && self.saved_todo_scope.is_some() {
            vec![("Esc", "viewed project"), ("Enter", "Details")]
        } else if self.focus == 3 {
            vec![("Enter", "Details")]
        } else {
            Vec::new()
        };
        for (key, label) in details
            .into_iter()
            .chain(self.panel_hints(self.focus))
            .chain(GLOBAL_HINTS)
        {
            if column as usize + width_of(key) + width_of(label) + 5 > width as usize {
                break;
            }
            // Every key drawn as a key cap.
            put(
                buffer,
                row,
                column,
                &format!(" {key} "),
                on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD),
            );
            column += width_of(key) as u16 + 3;
            put(buffer, row, column, label, dim(FG_DARK));
            column += width_of(label) as u16 + 2;
        }
    }

    /// `?`: the open tab's keys, then the keys that work everywhere (minus those the tab already
    /// lists), then, on Sessions, what the card signs mean.
    pub(crate) fn help_sections(&self) -> (String, Vec<HelpSection>) {
        let tab = if self.is_status_open() { 0 } else { self.focus };
        let this_tab = Self::panel_help(tab);
        let everywhere = GLOBAL_HELP
            .iter()
            .copied()
            .filter(|(key, _)| !this_tab.iter().any(|(own, _)| own == key))
            .collect();
        let mut sections = vec![("THIS TAB", this_tab), ("EVERYWHERE", everywhere)];
        if tab == 1 {
            sections.push(("WHAT THE SIGNS MEAN", LEGEND.to_vec()));
        }
        (format!("Help · {}", PANEL_NAMES[tab]), sections)
    }

    pub(super) fn draw_help(&mut self, buffer: &mut Buffer) {
        let (width, height) = (buffer.area.width, buffer.area.height);
        let (title, sections) = self.help_sections();
        let box_height = height.saturating_sub(HEADER_LINES + 1);
        let last = HEADER_LINES + box_height.saturating_sub(1);
        for line in 0..box_height {
            put(
                buffer,
                HEADER_LINES + line,
                0,
                &" ".repeat(width.saturating_sub(1) as usize),
                style(FG),
            );
        }
        let key_width = sections
            .iter()
            .flat_map(|(_, entries)| entries.iter())
            .map(|(key, _)| width_of(key))
            .max()
            .unwrap_or(1) as u16
            + 2;
        let label_width = width.saturating_sub(key_width + 5) as usize;
        let mut lines = vec![Line::new(vec![segment(2, title, bold(FG))])];
        for (heading, entries) in sections {
            lines.push(Line::new(vec![segment(2, heading, bold(FG_DARK))]));
            for (key, label) in entries {
                for (index, piece) in approval::wrap(label, label_width.max(8))
                    .into_iter()
                    .enumerate()
                {
                    let mut parts = Vec::new();
                    if index == 0 && !key.is_empty() {
                        parts.push(segment(
                            2,
                            format!(" {key} "),
                            on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD),
                        ));
                    }
                    parts.push(segment(2 + key_width + 1, piece, style(FG_DARK)));
                    lines.push(Line::new(parts));
                }
            }
        }
        let visible = last.saturating_sub(HEADER_LINES + 1) as usize;
        self.help_scroll = self.help_scroll.min(lines.len().saturating_sub(visible));
        for (index, line) in lines
            .iter()
            .skip(self.help_scroll)
            .take(visible)
            .enumerate()
        {
            for (column, text, tint) in &line.segments {
                put(
                    buffer,
                    HEADER_LINES + 1 + index as u16,
                    *column,
                    text,
                    *tint,
                );
            }
        }
        put(
            buffer,
            last,
            2,
            " esc ",
            on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD),
        );
        put(buffer, last, 8, "close · ↑↓ scroll", dim(FG_DARK));
    }
}
