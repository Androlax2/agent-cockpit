//! The 4 Teams tab (frames 3409, 3670, 3973): the teams on top, the selected team below, nothing to
//! open. ⏎ goes into a team, where its members and what needs Theo can be acted on.
use super::*;
use crate::teams::{sidebar, view::{Row, TeamView}, Role, TaskKey};

#[derive(Default)]
pub(crate) struct TeamsTab {
    /// The badge: teams that need Theo, else the teams still running.
    pub needs_you: usize,
    pub live: usize,
    /// The list's teams in their order: the panel's items index it.
    teams: Vec<TaskKey>,
    /// Inside a team (⏎): that team; the panel's items then index `rows`.
    inside: Option<TaskKey>,
    rows: Vec<InsideRow>,
    show_ended: bool,
    /// When the team host last answered, for "has not answered since".
    answered_at: Option<i64>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group { NeedsYou, Working, Ended }

/// A row that acts inside a team: a member (its index in the snapshot) or what needs Theo.
#[derive(Clone, PartialEq)]
enum InsideRow { Member(usize), PlannedQa, Needs(Row) }

const RECENT_SHOWN: usize = 3;

fn key_cap(key: &str) -> String { format!(" {key} ") }

fn cap_style() -> Style { on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD) }

/// Key caps and their labels on one line, from `column`.
fn keys_line(column: u16, keys: &[(&str, &str)], item: Option<usize>) -> Line {
    let mut segments = Vec::new();
    let mut at = column;
    for (key, label) in keys {
        segments.push(segment(at, key_cap(key), cap_style()));
        at += width_of(key) as u16 + 2;
        segments.push(segment(at, format!(" {label}"), dim(FG_DARK)));
        at += width_of(label) as u16 + 3;
    }
    Line { segments, item, ..Line::default() }
}

fn role_name(role: &Value) -> String {
    serde_json::from_value::<Role>(role.clone()).map(sidebar::role_label).map(str::to_string)
        .unwrap_or_else(|_| sidebar::shown(role.as_str().unwrap_or("Teammate")))
}

fn clock_of_ms(milliseconds: i64) -> String {
    chrono::DateTime::from_timestamp_millis(milliseconds).map(|moment| moment.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".into())
}

fn envelope(message: &Value) -> Value {
    serde_json::from_str(message["envelope"].as_str().unwrap_or("null")).unwrap_or(Value::Null)
}

fn short_commit(commit: &Value) -> String {
    sidebar::shown(&commit.as_str().unwrap_or("").chars().take(7).collect::<String>())
}

/// Every task the snapshot names, in its own order.
fn team_tasks(snapshot: &Value) -> Vec<TaskKey> {
    let mut tasks = Vec::new();
    for field in ["members", "proposals", "escalations", "spawns", "handoffs", "qa_plans"] {
        for entry in sidebar::list(snapshot, field) {
            if let Some(task) = sidebar::task_of(entry) && !tasks.contains(&task) { tasks.push(task); }
        }
    }
    for message in sidebar::list(snapshot, "messages") {
        if let Some(task) = sidebar::task_of(&envelope(message)["task"]) && !tasks.contains(&task) { tasks.push(task); }
    }
    tasks
}

pub(super) fn help() -> Vec<(&'static str, &'static str)> {
    vec![("j k", "move across the teams, or inside a team across its members and rows"),
         ("⏎", "into the selected team; inside, on a member: open its session"),
         ("esc", "back to the list of teams"),
         ("D", "show or fold the teams that ended today"),
         ("y n e", "inside a team, on a request: approve, reject, edit it"),
         ("c", "inside a team: cancel a failed delivery or a failed start"),
         ("l", "inside a team: the full log of requests, links and messages"),
         ("u", "inside a team, on a member: unlink it from the team"),
         ("R", "when the team data is unavailable: ask again"),
         ("● ◌ ✕ ○", "member live · asleep · gone · planned")]
}

impl App {
    fn team_members<'a>(snapshot: &'a Value, task: &TaskKey) -> Vec<(usize, &'a Value)> {
        let mut members: Vec<(usize, &Value)> = sidebar::list(snapshot, "members").iter().enumerate()
            .filter(|(_, member)| sidebar::task_of(member).as_ref() == Some(task)).collect();
        members.sort_by_key(|(_, member)| sidebar::role(member));
        members
    }

    fn member_session(&self, member: &Value) -> Option<&Session> {
        let id = member["session_id"].as_str()?;
        self.running_sessions.iter().find(|session| sidebar::matches_session(session, id))
    }

    fn has_planned_qa(snapshot: &Value, task: &TaskKey) -> bool {
        sidebar::has_qa_plan(snapshot, task) && !Self::team_members(snapshot, task).iter().any(|(_, member)| sidebar::role(member) == Some(Role::Qa))
    }

    /// What needs Theo in a team, as the Team view's rows: requests, link requests, escalations,
    /// uncertain or failed deliveries, failed starts and wakes.
    fn team_needs(view: &TeamView, task: &TaskKey) -> Vec<Row> {
        let snapshot = &view.snapshot;
        let member_ids: Vec<&str> = Self::team_members(snapshot, task).iter().filter_map(|(_, member)| member["session_id"].as_str()).collect();
        view.rows().into_iter().filter(|row| match *row {
            Row::Proposal(index) => sidebar::task_of(view.proposal(index)).as_ref() == Some(task),
            Row::LinkProposal(index) => {
                let proposal = view.link_proposal(index);
                sidebar::task_of(&proposal["task"]).as_ref() == Some(task) || proposal["sender"].as_str().is_some_and(|sender| member_ids.contains(&sender))
            },
            Row::Escalation(index) => sidebar::task_of(view.escalation(index)).as_ref() == Some(task),
            Row::Uncertain(index) | Row::FailedDelivery(index) => sidebar::task_of(&view.envelope(index)["task"]).as_ref() == Some(task),
            Row::FailedStart(index) => sidebar::task_of(&snapshot["spawns"][index]).as_ref() == Some(task),
            Row::FailedWake(index) => snapshot["sleeps"][index]["session_id"].as_str().is_some_and(|id| member_ids.contains(&id)),
            Row::Member(_) | Row::Link(_) | Row::Message(_) => false,
        }).collect()
    }

    fn ended_today(&self, task: &TaskKey) -> Option<i64> {
        let today = chrono::Local::now().date_naive();
        self.todos.items.iter().find(|item| item.id() == task.task_id && item.status() == "done")
            .map(|item| item.updated()).filter(|updated| chrono::DateTime::from_timestamp(*updated, 0)
                .is_some_and(|moment| moment.with_timezone(&chrono::Local).date_naive() == today))
    }

    fn team_group(&self, view: &TeamView, task: &TaskKey) -> Option<Group> {
        let snapshot = &view.snapshot;
        let now = util::now_secs();
        let members = Self::team_members(snapshot, task);
        if !Self::team_needs(view, task).is_empty()
            || members.iter().any(|(_, member)| self.member_session(member).is_some_and(|session| self.is_actionable(session, now))) {
            return Some(Group::NeedsYou);
        }
        if self.team_is_inactive(snapshot, task, &self.running_sessions) {
            return self.ended_today(task).map(|_| Group::Ended);
        }
        (!members.is_empty()).then_some(Group::Working)
    }

    /// The team's own ask for its card: what needs Theo in the team, else a member's ask.
    fn team_ask(&self, view: &TeamView, task: &TaskKey) -> Option<String> {
        let snapshot = &view.snapshot;
        if let Some(row) = Self::team_needs(view, task).first() {
            return Some(Self::needs_title(view, *row));
        }
        let now = util::now_secs();
        Self::team_members(snapshot, task).iter().find_map(|(_, member)| {
            let session = self.member_session(member).filter(|session| self.is_actionable(session, now))?;
            let ask = self.session_card(session, now).ask.map(|(ask, _)| ask)?;
            Some(format!("{}: {ask}", role_name(&member["role"])))
        })
    }

    fn in_flight<'a>(snapshot: &'a Value, task: &TaskKey) -> Vec<&'a Value> {
        sidebar::list(snapshot, "messages").iter().filter(|message| matches!(message["state"].as_str(), Some("queued" | "ready" | "submitting"))
            && sidebar::task_of(&envelope(message)["task"]).as_ref() == Some(task)).collect()
    }

    pub(super) fn build_teams(&mut self) {
        let previous_team = self.teams_tab.teams.get(self.panels[TEAMS_PANEL].selected).cloned();
        let previous_row = self.teams_tab.rows.get(self.panels[TEAMS_PANEL].selected).cloned();
        let snapshot = match &self.team_links {
            TeamLinks::Ready(snapshot) => snapshot.clone(),
            other => {
                let is_loading = matches!(other, TeamLinks::Loading);
                self.teams_tab.teams.clear();
                self.teams_tab.rows.clear();
                self.teams_tab.needs_you = 0;
                self.teams_tab.live = 0;
                self.panels[TEAMS_PANEL].items = 0;
                self.panels[TEAMS_PANEL].lines = if is_loading { vec![Line::new(vec![segment(0, "Loading teams…", dim(FG_DARK))])] } else { self.teams_unavailable_lines() };
                return;
            },
        };
        self.teams_tab.answered_at = Some(util::now_secs());
        let view = TeamView::new(snapshot);
        let snapshot = &view.snapshot;
        let mut grouped: Vec<(Group, TaskKey)> = team_tasks(snapshot).into_iter()
            .filter_map(|task| self.team_group(&view, &task).map(|group| (group, task))).collect();
        grouped.sort_by_key(|(group, _)| *group);
        self.teams_tab.needs_you = grouped.iter().filter(|(group, _)| *group == Group::NeedsYou).count();
        self.teams_tab.live = grouped.iter().filter(|(group, _)| *group != Group::Ended).count();
        if let Some(task) = self.teams_tab.inside.clone() {
            if grouped.iter().any(|(_, candidate)| *candidate == task) {
                self.build_inside_team(&view, &task, previous_row);
                return;
            }
            self.teams_tab.inside = None;
            self.message = "That team ended or left the list".into();
        }
        let show_ended = self.teams_tab.show_ended;
        self.teams_tab.teams = grouped.iter().filter(|(group, _)| show_ended || *group != Group::Ended).map(|(_, task)| task.clone()).collect();
        let panel = &mut self.panels[TEAMS_PANEL];
        if let Some(index) = previous_team.and_then(|task| self.teams_tab.teams.iter().position(|candidate| *candidate == task)) { panel.selected = index; }
        panel.items = self.teams_tab.teams.len();
        panel.clamp();
        let selected = self.teams_tab.teams.get(panel.selected).cloned();
        let mut lines = Vec::new();
        let ended = grouped.iter().filter(|(group, _)| *group == Group::Ended).count();
        if self.teams_tab.live == 0 {
            lines.push(Line::new(vec![segment(0, "No teams right now. A team starts when a session recruits a teammate.", dim(FG_DARK))]));
        }
        let mut shown_group = None;
        for (group, task) in &grouped {
            if *group == Group::Ended && !show_ended {
                continue;
            }
            if shown_group != Some(*group) {
                shown_group = Some(*group);
                let count = grouped.iter().filter(|(candidate, _)| candidate == group).count();
                lines.push(match group {
                    Group::NeedsYou => Line::new(vec![segment(0, format!("NEEDS YOU · {count}"), bold(YELLOW))]),
                    Group::Working => Line::new(vec![segment(0, format!("WORKING · {count}"), bold(FG_DARK))]),
                    Group::Ended => Line::new(vec![segment(0, format!("▾ Ended today · {count}"), dim(FG_DARK))]),
                });
            }
            let item = self.teams_tab.teams.iter().position(|candidate| candidate == task);
            lines.extend(self.team_card(&view, task, *group, item));
        }
        if ended > 0 && !show_ended {
            lines.push(Line::new(vec![segment(0, format!("▸ Ended today · {ended}"), dim(FG_DARK))]));
        }
        if let Some(task) = selected {
            lines.push(Line::new(vec![segment(0, "─".repeat((self.width as usize).saturating_sub(5)), dim(FG_DARK))]));
            lines.extend(self.team_detail(&view, &task, false));
        }
        self.panels[TEAMS_PANEL].lines = lines;
    }

    fn teams_unavailable_lines(&self) -> Vec<Line> {
        let since = self.teams_tab.answered_at.and_then(|at| chrono::DateTime::from_timestamp(at, 0))
            .map(|moment| format!(" since {}", moment.with_timezone(&chrono::Local).format("%H:%M"))).unwrap_or_default();
        let text = format!("! Team data unavailable: cockpit-teams has not answered{since}. Sessions still show in 1. R retry");
        wrap_detail_text(&text, (self.width as usize).saturating_sub(5).max(8)).into_iter()
            .map(|line| Line::new(vec![segment(0, line, style(RED))])).collect()
    }

    /// Line 1 glyph, task, badges and age; line 2 `project › task · Role ● Role ○`; the amber bar names the ask.
    fn team_card(&self, view: &TeamView, task: &TaskKey, group: Group, item: Option<usize>) -> Vec<Line> {
        let snapshot = &view.snapshot;
        let members = Self::team_members(snapshot, task);
        let (glyph, tint) = match group { Group::NeedsYou => ("!", YELLOW), Group::Working => ("◐", BLUE), Group::Ended => ("✓", GREEN) };
        let mut right = Vec::new();
        let in_flight = Self::in_flight(snapshot, task).len();
        if in_flight > 0 { right.push(part(format!("✉{in_flight}"), style(FG_DARK))); }
        if members.iter().any(|(_, member)| self.member_session(member).is_some_and(|session| !sessions::changed_since_start(session, &self.start_files).is_empty())) {
            right.push(part("⟳", style(FG_DARK)));
        }
        let age = match group {
            Group::Ended => self.ended_today(task).and_then(|at| chrono::DateTime::from_timestamp(at, 0))
                .map(|moment| moment.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default(),
            _ => members.iter().filter_map(|(_, member)| self.member_session(member)).map(|session| session.started.unwrap_or(session.since)).min()
                .map(util::age).unwrap_or_default(),
        };
        right.push(part(age, dim(FG_DARK)));
        let title_style = if group == Group::Ended { style(FG_DARK) } else { bold(FG) };
        let mut lines = vec![Line { segments: vec![segment(0, glyph, bold(tint)), segment(2, self.team_task_title(snapshot, task), title_style)], right, item, ..Line::default() }];
        let project = sidebar::project_name(snapshot, task, &self.running_sessions);
        let mut why = why_segments(&project, &sidebar::shown(&task.task_id));
        let mut column = why.last().map(|(column, text, _)| column + width_of(text) as u16).unwrap_or(2);
        let mut add = |text: String, tint: Style, why: &mut Vec<(u16, String, Style)>| {
            let width = width_of(&text) as u16;
            why.push(segment(column, text, tint));
            column += width;
        };
        if group == Group::Ended {
            add(format!(" · {}", sidebar::handoff_summary(snapshot, task)), dim(FG_DARK), &mut why);
        } else {
            add(" ·".into(), dim(FG_DARK), &mut why);
            for (_, member) in &members {
                let (mark, mark_style) = self.member_mark(member);
                add(format!(" {} ", role_name(&member["role"])), style(FG_DARK), &mut why);
                add(mark.into(), mark_style, &mut why);
            }
            if Self::has_planned_qa(snapshot, task) {
                add(" QA ".into(), style(FG_DARK), &mut why);
                add("○".into(), dim(FG_DARK), &mut why);
            }
        }
        lines.push(Line { segments: why, item, ..Line::default() });
        if group == Group::NeedsYou && let Some(ask) = self.team_ask(view, task) {
            lines.push(ask_bar(item, &format!("! {ask}"), "", "⏎ see"));
        }
        lines.push(Line::default());
        lines
    }

    /// Live (blue ●), asleep (◌, dim), gone (red ✕).
    fn member_mark(&self, member: &Value) -> (&'static str, Style) {
        match self.member_session(member) {
            Some(session) if session.asleep.is_some() => ("◌", dim(FG_DARK)),
            Some(_) => ("●", style(BLUE)),
            None => ("✕", style(RED)),
        }
    }

    /// The selected team: MEMBERS, NEEDS YOU, IN FLIGHT and RECENT. Inside the team its members and
    /// needs-you rows become the panel's items.
    fn team_detail(&self, view: &TeamView, task: &TaskKey, inside: bool) -> Vec<Line> {
        let snapshot = &view.snapshot;
        let mut item = 0usize;
        let mut next_item = || { let current = item; item += 1; inside.then_some(current) };
        let title = self.team_task_title(snapshot, task);
        let mut lines = vec![Line::new(vec![segment(0, if inside { format!("‹ {title}") } else { title }, bold(CYAN))])];
        let project = sidebar::shown(&sidebar::project_name(snapshot, task, &self.running_sessions));
        let is_todo = self.todos.items.iter().any(|candidate| candidate.id() == task.task_id);
        lines.push(Line::new(vec![segment(0, format!("{project} › {}{}", if is_todo { "todo " } else { "" }, sidebar::shown(&task.task_id)), dim(FG_DARK))]));
        let members = Self::team_members(snapshot, task);
        let planned = Self::has_planned_qa(snapshot, task);
        lines.push(Line::new(vec![segment(0, format!("MEMBERS · {}", members.len() + usize::from(planned)), bold(FG_DARK))]));
        for (_, member) in &members {
            lines.push(self.member_line(member, next_item()));
        }
        if planned {
            let (text, status) = match sidebar::qa_status(snapshot, task).as_str() {
                "QA planned · Fresh reviewer starts at handoff" => ("Starts at the next handoff".to_string(), "planned"),
                other => (other.trim_start_matches("QA ").to_string(), ""),
            };
            lines.push(Line { segments: vec![segment(1, "○", dim(FG_DARK)), segment(3, "QA", style(FG_DARK)), segment(14, text, dim(FG_DARK))],
                right: vec![part(status, dim(FG_DARK))], item: next_item(), ..Line::default() });
        }
        let needs = Self::team_needs(view, task);
        if !needs.is_empty() {
            lines.push(Line::new(vec![segment(0, format!("NEEDS YOU · {}", needs.len()), bold(YELLOW))]));
            for row in needs {
                let item = next_item();
                lines.push(Line { segments: vec![segment(1, format!("! {}", Self::needs_title(view, row)), bold(YELLOW))], item, accent: Some(bold(YELLOW)), ..Line::default() });
                let explanation = Self::needs_explanation(view, row);
                if !explanation.is_empty() {
                    lines.push(Line { segments: vec![segment(1, explanation, dim(FG_DARK))], item, accent: Some(bold(YELLOW)), ..Line::default() });
                }
                let keys = Self::needs_keys(row);
                if !keys.is_empty() {
                    let mut line = keys_line(1, &keys, item);
                    line.accent = Some(bold(YELLOW));
                    lines.push(line);
                }
            }
        }
        let in_flight = Self::in_flight(snapshot, task);
        if !in_flight.is_empty() {
            lines.push(Line::new(vec![segment(0, format!("IN FLIGHT · {}", in_flight.len()), bold(FG_DARK))]));
            for message in in_flight {
                let age = message["created_at"].as_i64().map(|at| util::age(at / 1000)).unwrap_or_default();
                lines.push(Line { segments: vec![segment(1, "✉", style(BLUE)), segment(3, self.message_words(message), style(FG))],
                    right: vec![part(age, dim(FG_DARK))], ..Line::default() });
            }
        }
        let recent = self.team_recent(snapshot, task);
        if !recent.is_empty() {
            lines.push(Line::new(vec![segment(0, "RECENT", bold(FG_DARK))]));
            for (at, text) in recent.iter().take(RECENT_SHOWN) {
                lines.push(Line::new(vec![segment(1, clock_of_ms(*at), dim(FG_DARK)), segment(7, text.clone(), style(FG))]));
            }
            if recent.len() > RECENT_SHOWN {
                let how = if inside { "l full log" } else { "⏎ then l for the full log" };
                lines.push(Line::new(vec![segment(0, format!("+ {} earlier · {how}", recent.len() - RECENT_SHOWN), dim(FG_DARK))]));
            }
        }
        lines
    }

    /// Role, activity and state: `● Developer  Grouping the session list  working · 30s`.
    fn member_line(&self, member: &Value, item: Option<usize>) -> Line {
        let role = role_name(&member["role"]);
        let now = util::now_secs();
        let (glyph, role_style, text, status) = match self.member_session(member) {
            Some(session) if session.asleep.is_some() => {
                let since = session.asleep.and_then(|at| chrono::DateTime::from_timestamp(at, 0))
                    .map(|moment| moment.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default();
                (("◌", dim(FG_DARK)), style(FG_DARK), format!("Asleep since {since}"), part("wakes on a message", dim(FG_DARK)))
            },
            Some(session) => {
                let card = self.session_card(session, now);
                let text = if card.group == Attention::Working || card.ask.is_some() { card.doing.clone() } else { sidebar::shown(self.session_display_title(session)) };
                let status = match card.group {
                    Attention::NeedsYou => part(format!("needs you · {}", card.age), style(YELLOW)),
                    Attention::Working => part(format!("working · {}", card.age), style(BLUE)),
                    Attention::Quiet => part(format!("{} · {}", sidebar::status(session).to_lowercase(), card.age), dim(FG_DARK)),
                };
                let glyph = if card.ask.is_some() { ("!", bold(YELLOW)) } else { ("●", style(BLUE)) };
                (glyph, bold(FG), text, status)
            },
            None => (("✕", style(RED)), style(FG), "Not running · not back in the team".to_string(), part("", dim(FG_DARK))),
        };
        Line { segments: vec![segment(1, glyph.0, glyph.1), segment(3, role, role_style), segment(14, text, style(FG))], right: vec![status], item, ..Line::default() }
    }

    fn needs_title(view: &TeamView, row: Row) -> String {
        let snapshot = &view.snapshot;
        let session_role = |id: &Value| sidebar::list(snapshot, "members").iter().find(|member| member["session_id"] == *id)
            .map(|member| role_name(&member["role"])).unwrap_or_else(|| sidebar::shown(id.as_str().unwrap_or("a session")));
        match row {
            Row::Proposal(index) => {
                let roles: Vec<String> = view.mapping(index).iter().map(|member| role_name(&member["role"])).collect();
                format!("Team request: {}", roles.join(" ⇄ "))
            },
            Row::LinkProposal(index) => {
                let proposal = view.link_proposal(index);
                let peer = proposal["participants"].as_array().into_iter().flatten().find(|participant| participant["session_id"] != proposal["sender"])
                    .map(|participant| crate::teams::view::short_project(participant["project"].as_str().unwrap_or(""))).unwrap_or_else(|| "a session".into());
                format!("Link request: {} ⇄ {peer}", session_role(&proposal["sender"]))
            },
            Row::Escalation(index) => format!("Escalated: {}", sidebar::shown(view.escalation(index)["reason"].as_str().unwrap_or("a question"))),
            Row::Uncertain(index) => format!("Did the message reach {}?", role_name(&view.envelope(index)["recipient"]["role"])),
            Row::FailedDelivery(index) => format!("Message to {} failed", role_name(&view.envelope(index)["recipient"]["role"])),
            Row::FailedStart(index) => {
                let spawn = &snapshot["spawns"][index];
                if spawn["status"] == "refused" { format!("{} started but could not join", role_name(&spawn["role"])) }
                else { format!("{} failed to start", role_name(&spawn["role"])) }
            },
            Row::FailedWake(index) => format!("{} did not wake", session_role(&snapshot["sleeps"][index]["session_id"])),
            Row::Member(_) | Row::Link(_) | Row::Message(_) => String::new(),
        }
    }

    fn needs_explanation(view: &TeamView, row: Row) -> String {
        let snapshot = &view.snapshot;
        let text = |value: &Value| sidebar::shown(value.as_str().unwrap_or(""));
        match row {
            Row::Proposal(index) => text(&view.proposal(index)["reason"]),
            Row::LinkProposal(index) => text(&view.link_proposal(index)["purpose"]),
            Row::Escalation(_) => "Resolve it with a note for the team.".into(),
            Row::Uncertain(_) => "No receipt. It is never sent twice: say it arrived, or cancel it.".into(),
            Row::FailedDelivery(index) => format!("{} It is not retried.", text(&view.message(index)["error"])),
            Row::FailedStart(_) => "It is never started again by itself: cancel clears it.".into(),
            Row::FailedWake(index) => format!("{} Queued messages are kept.", text(&snapshot["sleeps"][index]["error"])),
            Row::Member(_) | Row::Link(_) | Row::Message(_) => String::new(),
        }
    }

    /// The keys a needs-you row takes inside the team, and the Team view key each one stands for.
    fn needs_actions(row: Row) -> &'static [(&'static str, &'static str, char)] {
        match row {
            Row::Proposal(_) | Row::LinkProposal(_) => &[("y", "approve", 'a'), ("n", "reject", 'x'), ("e", "edit purpose", 'e')],
            Row::Escalation(_) => &[("e", "resolve with a note", 's')],
            Row::Uncertain(_) => &[("y", "it arrived", 'h'), ("c", "cancel", 'c')],
            Row::FailedDelivery(_) => &[("c", "cancel", 'c')],
            Row::FailedStart(_) => &[("c", "cancel", 'd')],
            Row::FailedWake(_) | Row::Member(_) | Row::Link(_) | Row::Message(_) => &[],
        }
    }

    fn needs_keys(row: Row) -> Vec<(&'static str, &'static str)> {
        Self::needs_actions(row).iter().map(|(key, label, _)| (*key, *label)).collect()
    }

    fn message_words(&self, message: &Value) -> String {
        let envelope = envelope(message);
        let from = role_name(&envelope["sender"]["role"]);
        let to = role_name(&envelope["recipient"]["role"]);
        let kind = match envelope["kind"].as_str() {
            Some("qa_request") => format!("handoff {}", short_commit(&envelope["handoff"]["commit"])),
            Some(other) => sidebar::shown(other),
            None => "message".into(),
        };
        let recipient_runs = envelope["recipient"]["session_id"].as_str()
            .is_some_and(|id| self.running_sessions.iter().any(|session| sidebar::matches_session(session, id) && session.asleep.is_none()));
        let state = match message["state"].as_str() {
            Some("queued") if !recipient_runs => format!("queued until {to} starts"),
            Some("queued") => "queued".into(),
            Some("ready") => "ready to deliver".into(),
            _ => "delivering".into(),
        };
        format!("{from} → {to} · {kind} · {state}")
    }

    /// Today's dated events of the team, newest first: handoffs, starts, escalations and cancelled messages.
    fn team_recent(&self, snapshot: &Value, task: &TaskKey) -> Vec<(i64, String)> {
        let in_task = |value: &&Value| sidebar::task_of(value).as_ref() == Some(task);
        let mut events: Vec<(i64, String)> = Vec::new();
        for handoff in sidebar::list(snapshot, "handoffs").iter().filter(in_task) {
            events.push((handoff["created_at"].as_i64().unwrap_or(0), format!("Developer handed off (commit {})", short_commit(&handoff["payload"]["commit"]))));
        }
        for spawn in sidebar::list(snapshot, "spawns").iter().filter(in_task) {
            let role = role_name(&spawn["role"]);
            let text = match spawn["status"].as_str() {
                Some("failed") => format!("{role} failed to start"),
                Some("refused") => format!("{role} could not join"),
                _ => format!("{role} started"),
            };
            events.push((spawn["created_at"].as_i64().unwrap_or(0), text));
        }
        for escalation in sidebar::list(snapshot, "escalations").iter().filter(in_task) {
            events.push((escalation["created_at"].as_i64().unwrap_or(0), format!("Escalated: {}", sidebar::shown(escalation["reason"].as_str().unwrap_or("")))));
        }
        for message in sidebar::list(snapshot, "messages").iter().filter(|message| message["state"] == "cancelled") {
            let envelope = envelope(message);
            if sidebar::task_of(&envelope["task"]).as_ref() != Some(task) { continue; }
            events.push((message["created_at"].as_i64().unwrap_or(0), format!("Cancelled: {} → {} {}", role_name(&envelope["sender"]["role"]),
                role_name(&envelope["recipient"]["role"]), sidebar::shown(envelope["kind"].as_str().unwrap_or("message")))));
        }
        let today = chrono::Local::now().date_naive();
        events.retain(|(at, _)| chrono::DateTime::from_timestamp_millis(*at).is_some_and(|moment| moment.with_timezone(&chrono::Local).date_naive() == today));
        events.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        events
    }

    fn build_inside_team(&mut self, view: &TeamView, task: &TaskKey, previous: Option<InsideRow>) {
        let snapshot = &view.snapshot;
        let mut rows: Vec<InsideRow> = Self::team_members(snapshot, task).iter().map(|(index, _)| InsideRow::Member(*index)).collect();
        if Self::has_planned_qa(snapshot, task) { rows.push(InsideRow::PlannedQa); }
        rows.extend(Self::team_needs(view, task).into_iter().map(InsideRow::Needs));
        let lines = self.team_detail(view, task, true);
        let panel = &mut self.panels[TEAMS_PANEL];
        if let Some(index) = previous.and_then(|row| rows.iter().position(|candidate| *candidate == row)) { panel.selected = index; }
        panel.items = rows.len();
        panel.clamp();
        panel.lines = lines;
        self.teams_tab.rows = rows;
    }

    /// `i` on a team block in Sessions: the Teams tab with that team selected.
    pub(super) fn open_teams_tab_on(&mut self, task: &TaskKey) -> String {
        self.focus_panel(TEAMS_PANEL);
        self.teams_tab.inside = None;
        self.build_teams();
        if !self.teams_tab.teams.contains(task) {
            self.teams_tab.show_ended = true;
            self.build_teams();
        }
        match self.teams_tab.teams.iter().position(|candidate| candidate == task) {
            Some(index) => { self.panels[TEAMS_PANEL].selected = index; self.build_teams(); String::new() },
            None => "That team is not in the Teams tab".into(),
        }
    }

    /// The team a Sessions row belongs to: the team block itself or one of its members.
    pub(super) fn selected_sidebar_task(&self) -> Option<TaskKey> {
        use sidebar::SidebarRow;
        if self.sidebar_selection_lost { return None; }
        match self.sidebar_rows.get(self.panels[1].selected)? {
            SidebarRow::Team { task } | SidebarRow::PlannedQa { task } => Some(task.clone()),
            SidebarRow::Session { session_id } => self.sidebar_session_tasks.get(session_id).cloned(),
        }
    }

    pub(super) fn teams_hints(&self) -> Vec<(&'static str, &'static str)> {
        if !matches!(self.team_links, TeamLinks::Ready(_)) { return vec![("R", "retry")]; }
        if self.teams_tab.inside.is_none() {
            return vec![("j k", "move"), ("⏎", "into the team"), ("D", if self.teams_tab.show_ended { "fold ended" } else { "ended today" })];
        }
        let mut hints = vec![("j k", "move")];
        match self.teams_tab.rows.get(self.panels[TEAMS_PANEL].selected) {
            Some(InsideRow::Member(_)) => hints.extend([("⏎", "open member's session"), ("l", "full log"), ("u", "unlink")]),
            Some(InsideRow::Needs(row)) => { hints.extend(Self::needs_keys(*row)); hints.push(("l", "full log")); },
            _ => hints.push(("l", "full log")),
        }
        hints.push(("esc", "back to teams"));
        hints
    }

    pub(super) fn handle_teams(&mut self, key: Key, screen: &mut dyn Screen) -> String {
        if key == Key::Char('R') {
            self.team_checked = 0.0;
            self.check_team();
            return if matches!(self.team_links, TeamLinks::Ready(_)) { String::new() } else { "Team data still unavailable".into() };
        }
        let TeamLinks::Ready(snapshot) = &self.team_links else { return String::new() };
        let snapshot = snapshot.clone();
        let Some(task) = self.teams_tab.inside.clone() else {
            match key {
                Key::Enter => if let Some(task) = self.teams_tab.teams.get(self.panels[TEAMS_PANEL].selected).cloned() {
                    self.teams_tab.inside = Some(task);
                    self.teams_tab.rows.clear();
                    self.panels[TEAMS_PANEL].selected = 0;
                    self.panels[TEAMS_PANEL].scroll = 0;
                },
                Key::Char('D') => self.teams_tab.show_ended = !self.teams_tab.show_ended,
                _ => {},
            }
            return String::new();
        };
        let selected = self.teams_tab.rows.get(self.panels[TEAMS_PANEL].selected).cloned();
        match (key, selected) {
            (Key::Esc, _) => {
                self.teams_tab.inside = None;
                self.teams_tab.rows.clear();
                self.panels[TEAMS_PANEL].selected = self.teams_tab.teams.iter().position(|candidate| *candidate == task).unwrap_or(0);
                self.panels[TEAMS_PANEL].scroll = 0;
                String::new()
            },
            (Key::Char('l'), _) => {
                let mut view = TeamView::new(snapshot);
                view.is_history = true;
                self.team_view = Some(view);
                String::new()
            },
            (Key::Enter, Some(InsideRow::Member(index))) => {
                let Some(session) = self.member_session(&snapshot["members"][index]).cloned() else { return "That member's session is not running".into() };
                let opened = self.manager_async(&["open", &session.session]);
                self.mark_result_seen(&session.session).unwrap_or(opened)
            },
            (Key::Char('u'), Some(InsideRow::Member(index))) => self.act_on_team_row(Row::Member(index), Key::Char('u'), screen),
            (Key::Char(pressed), Some(InsideRow::Needs(row))) => match Self::needs_actions(row).iter().find(|(key, _, _)| key.starts_with(pressed)) {
                Some((_, _, action)) => self.act_on_team_row(row, Key::Char(*action), screen),
                None => String::new(),
            },
            _ => String::new(),
        }
    }

    /// Runs the Team view's action for a row without showing that view, then reads the teams again.
    fn act_on_team_row(&mut self, row: Row, key: Key, screen: &mut dyn Screen) -> String {
        let TeamLinks::Ready(snapshot) = &self.team_links else { return "Team data unavailable: R retry".into() };
        let mut view = TeamView::new(snapshot.clone());
        view.hidden = true;
        self.team_view = Some(view);
        let message = self.team_row_action(row, key, screen);
        if self.team_view.as_ref().is_some_and(|view| view.hidden) { self.team_view = None; }
        self.team_checked = 0.0;
        self.check_team();
        message
    }
}
