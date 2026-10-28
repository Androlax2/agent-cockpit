//! The 5 Chief tab (frames 3553, 3803, edge states 3973): the state card and its keys, what needs
//! Theo, what the Chief is waiting for now, and today's feed. Its keys act through the Chief view.
use super::*;
use crate::chief::view::{ChiefView, one_line};
use serde_json::json;

/// A row of the tab that acts: an open decision in NEEDS YOU, or in TODAY an answer or a question sent to Theo.
#[derive(Clone)]
pub(crate) enum ChiefRow {
    Decision(Value),
    Answer(Value),
    Sent(Value),
}

const TODAY_SHOWN: usize = 4;
const TEXT_LIMIT: usize = 200;

fn records<'a>(snapshot: &'a Value, kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
    snapshot["records"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(move |record| record["kind"] == kind)
}

fn now_ms() -> i64 {
    (util::now() * 1000.0) as i64
}

fn midnight_ms() -> i64 {
    chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| midnight.and_local_timezone(chrono::Local).earliest())
        .map(|midnight| midnight.timestamp_millis())
        .unwrap_or(0)
}

fn clock(milliseconds: i64) -> String {
    chrono::DateTime::from_timestamp_millis(milliseconds)
        .map(|moment| {
            moment
                .with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "--:--".into())
}

fn text(value: &Value) -> String {
    one_line(value.as_str().unwrap_or("?"), TEXT_LIMIT)
}

fn category(decision: &Value) -> &'static str {
    match decision["category"].as_str() {
        Some("security") => "Security",
        Some("scope") => "Scope",
        _ => "Not sure",
    }
}

fn is_escalation(decision: &Value) -> bool {
    decision["stop_id"].is_string()
}

fn chief(snapshot: &Value) -> Option<&Value> {
    records(snapshot, "chief").find(|chief| chief["state"] != "disabled")
}

/// Open decisions that need Theo now, oldest first: one put off with Later counts once it is back.
fn open_decisions(snapshot: &Value) -> Vec<&Value> {
    let now = now_ms();
    let mut open: Vec<&Value> = records(snapshot, "decision")
        .filter(|decision| {
            decision["state"] == "open" && !crate::chief::inbox::is_put_off(decision, now)
        })
        .collect();
    open.sort_by_key(|decision| decision["created_at_ms"].as_i64().unwrap_or(0));
    open
}

/// The tab's badge: a grant is on, as the team host's `chief_badge` reports it on the other tabs.
pub(super) fn has_active_grant(snapshot: &Value) -> bool {
    chief(snapshot).is_some_and(|chief| chief["state"] == "active")
}

pub(super) fn needs_you(snapshot: &Value) -> usize {
    ChiefView::attention(snapshot, now_ms())
}

fn is_live(chief: &Value) -> bool {
    chief["live_verified"] == true || chief["live_verified"] == 1
}

/// On: an active grant the host verified live, with nothing making its session unavailable.
pub(super) fn is_on(snapshot: &Value) -> bool {
    ChiefView::new(snapshot.clone()).status() == "Active"
}

fn today_counts(snapshot: &Value) -> Option<(u64, u64)> {
    Some((
        snapshot["today"]["answered"].as_u64()?,
        snapshot["today"]["escalated"].as_u64()?,
    ))
}

/// Status's one Chief line: `◆ On · 5 answered today · 1 needs you`.
pub(super) fn status_line(snapshot: &Value) -> String {
    let needs = needs_you(snapshot);
    let needs = if needs > 0 {
        format!(" · {needs} needs you")
    } else {
        String::new()
    };
    if !is_on(snapshot) {
        return format!("◇ {}{needs}", ChiefView::new(snapshot.clone()).status());
    }
    match today_counts(snapshot) {
        Some((answered, _)) => format!("◆ On · {answered} answered today{needs}"),
        None => format!("◆ On · today's counts unavailable{needs}"),
    }
}

pub(super) fn help() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "c",
            "pause or resume the Chief (start it when it is off); you review the change first",
        ),
        ("p", "pick another session to run as the Chief"),
        ("r", "a summary now"),
        ("l", "the answer log: every answer, overrule one with o"),
        ("h", "the habits it learned from you"),
        ("j k", "move across what needs you and today's feed"),
        (
            "y x i",
            "on an undelivered message: it arrived, stop safely, details",
        ),
        (
            "t a d",
            "a restarted Chief: send its new session a test message, then say whether it arrived",
        ),
        (
            "⏎",
            "on an escalation: answer it; on an answer: open that session",
        ),
        ("o", "on an answer: overrule it"),
        ("◆ ◇", "the Chief on · off, on its card"),
        (
            "● ○",
            "the tab's badge: a Chief grant on · none or paused; ? when the team host does not answer",
        ),
    ]
}

fn keys_line(keys: &[(&str, String)]) -> Line {
    let mut segments = Vec::new();
    let mut column = 0u16;
    for (key, label) in keys {
        segments.push(segment(
            column,
            format!(" {key} "),
            on(FG, BG_HIGHLIGHT).add_modifier(Modifier::BOLD),
        ));
        column += width_of(key) as u16 + 2;
        segments.push(segment(column, format!(" {label}"), dim(FG_DARK)));
        column += width_of(label) as u16 + 3;
    }
    Line::new(segments)
}

impl App {
    pub(super) fn build_chief_tab(&mut self) {
        self.chief_tab_rows.clear();
        let snapshot = match &self.chief_status {
            Ok(snapshot) => snapshot.clone(),
            Err(error) => {
                let panel = &mut self.panels[CHIEF_PANEL];
                panel.lines = vec![Line::new(vec![segment(
                    0,
                    format!("◇ Chief unavailable · {}", one_line(error, 160)),
                    style(RED),
                )])];
                panel.items = 0;
                return;
            }
        };
        let mut view = ChiefView::new(snapshot.clone());
        view.habits_seen_ms = self.chief_habits_seen_ms;
        view.check_answers = self.chief_check_answers.clone();
        let mut lines = self.chief_state_lines(&view, &snapshot);
        let mut rows = Vec::new();
        let open = open_decisions(&snapshot);
        let put_off = crate::chief::inbox::put_off_count(&snapshot, now_ms(), None);
        if !open.is_empty() || put_off > 0 {
            lines.push(Line::new(vec![segment(
                0,
                format!(
                    "NEEDS YOU · {}{}",
                    open.len(),
                    crate::chief::inbox::later_suffix(put_off)
                ),
                bold(YELLOW),
            )]));
            for decision in open {
                let item = Some(rows.len());
                rows.push(ChiefRow::Decision(decision.clone()));
                let accent = Some(bold(YELLOW));
                let title = if is_escalation(decision) {
                    format!(
                        "! {} · {}: {}",
                        view.session_name(decision["session_id"].as_str()),
                        category(decision),
                        text(&decision["question"])
                    )
                } else {
                    format!("! {}", text(&decision["question"]))
                };
                lines.push(Line {
                    segments: vec![segment(1, title, bold(YELLOW))],
                    item,
                    accent,
                    ..Line::default()
                });
                if let Some(why) = decision["recommendation"]
                    .as_str()
                    .filter(|why| !why.trim().is_empty())
                {
                    lines.push(Line {
                        segments: vec![segment(1, one_line(why, TEXT_LIMIT), dim(FG_DARK))],
                        item,
                        accent,
                        ..Line::default()
                    });
                }
                let keys: Vec<(&str, String)> = if view.is_recovery(decision) {
                    vec![
                        ("y", "it arrived".into()),
                        ("x", "stop safely".into()),
                        ("i", "details".into()),
                    ]
                } else if is_escalation(decision) {
                    vec![("⏎", "answer".into()), ("i", "details".into())]
                } else {
                    vec![("i", "details".into())]
                };
                let mut keys = keys_line(&keys);
                for (column, _, _) in &mut keys.segments {
                    *column += 1;
                }
                lines.push(Line {
                    item,
                    accent,
                    ..keys
                });
            }
        }
        let now = self.chief_now_lines(&view, &snapshot);
        if !now.is_empty() {
            lines.push(Line::new(vec![segment(
                0,
                format!("NOW · {}", now.len()),
                bold(FG_DARK),
            )]));
            lines.extend(now);
        }
        lines.push(Line::new(vec![segment(0, "TODAY", bold(FG_DARK))]));
        let today = self.chief_today(&view, &snapshot);
        if today.is_empty() {
            lines.push(Line::new(vec![segment(
                1,
                "Nothing decided yet today.",
                dim(FG_DARK),
            )]));
        }
        for (at, text, row) in today.iter().take(TODAY_SHOWN) {
            let item = Some(rows.len());
            rows.push(row.clone());
            lines.push(Line {
                segments: vec![
                    segment(1, clock(*at), dim(FG_DARK)),
                    segment(7, text.clone(), style(FG)),
                ],
                item,
                ..Line::default()
            });
        }
        if today.len() > TODAY_SHOWN {
            lines.push(Line::new(vec![segment(
                0,
                format!("+ {} earlier · l answer log", today.len() - TODAY_SHOWN),
                dim(FG_DARK),
            )]));
        }
        let panel = &mut self.panels[CHIEF_PANEL];
        panel.items = rows.len();
        panel.clamp();
        panel.lines = lines;
        self.chief_tab_rows = rows;
    }

    /// The state card (on/off, today's numbers, the session it runs as) and its keys.
    fn chief_state_lines(&self, view: &ChiefView, snapshot: &Value) -> Vec<Line> {
        let mut lines = Vec::new();
        let off_note = || {
            Line::new(vec![segment(
                0,
                "While it is off, sessions ask you directly.",
                dim(FG_DARK),
            )])
        };
        let chief = chief(snapshot);
        let mut keys: Vec<(&str, String)> = Vec::new();
        let status = view.status();
        match chief {
            Some(chief) if status == "Active" => {
                let counts = match today_counts(snapshot) {
                    Some((answered, escalated)) => {
                        format!("answered {answered} today · {escalated} sent to you")
                    }
                    None => "today's counts unavailable".into(),
                };
                lines.push(Line::new(vec![segment(
                    0,
                    format!("◆ Chief · On · {counts}"),
                    bold(FG),
                )]));
                lines.push(Line::new(vec![segment(
                    0,
                    format!(
                        "Runs as {} · {}",
                        view.session_name(chief["session"]["session_id"].as_str()),
                        one_line(chief["session"]["agent"].as_str().unwrap_or("?"), 40)
                    ),
                    dim(FG_DARK),
                )]));
                keys.push(("c", "pause".into()));
            }
            Some(_) if status.starts_with("Paused") => {
                lines.push(Line::new(vec![segment(
                    0,
                    "◇ Chief · Paused · c resume",
                    style(FG),
                )]));
                lines.push(off_note());
                keys.push(("c", "resume".into()));
            }
            // c runs the first of the Chief controls: pause or resume while live, else disable.
            Some(chief) => {
                lines.push(Line::new(vec![segment(
                    0,
                    format!("◇ Chief · {status}"),
                    style(FG),
                )]));
                lines.push(Line::new(vec![segment(
                    0,
                    format!(
                        "Runs as {}",
                        view.session_name(chief["session"]["session_id"].as_str())
                    ),
                    dim(FG_DARK),
                )]));
                lines.extend(
                    view.recovery_lines()
                        .into_iter()
                        .map(|line| Line::new(vec![segment(0, line, style(YELLOW))])),
                );
                let control = if !is_live(chief) {
                    "disable"
                } else if chief["state"] == "paused" {
                    "resume"
                } else {
                    "pause"
                };
                keys.push(("c", control.into()));
            }
            None if snapshot.get("startup").is_some() => {
                let action = view.startup_action_label().map(str::to_lowercase);
                let state = match &action {
                    Some(action) => format!("◇ Chief · {} · c {action}", view.status()),
                    None => format!("◇ Chief · {}", view.status()),
                };
                lines.push(Line::new(vec![segment(0, state, style(FG))]));
                lines.push(off_note());
                if let Some(action) = action {
                    keys.push(("c", action));
                }
            }
            None => {
                lines.push(Line::new(vec![segment(
                    0,
                    "◇ No chief yet · p pick the session that runs as chief",
                    style(FG),
                )]));
                lines.push(off_note());
            }
        }
        if chief.is_some() || snapshot.get("startup").is_none() {
            keys.push((
                "p",
                if chief.is_some() {
                    "pick another chief"
                } else {
                    "pick a chief"
                }
                .into(),
            ));
        }
        keys.push(("r", "summary now".into()));
        lines.push(keys_line(&keys));
        let new_habits = view.new_habits();
        lines.push(keys_line(&[
            ("l", "answer log".into()),
            (
                "h",
                if new_habits > 0 {
                    format!("habits ({new_habits} new)")
                } else {
                    "habits".into()
                },
            ),
        ]));
        lines
    }

    /// What the Chief waits for now: its answers not read yet, a summary being written.
    fn chief_now_lines(&self, view: &ChiefView, snapshot: &Value) -> Vec<Line> {
        let mut lines = Vec::new();
        for answer in records(snapshot, "answer")
            .filter(|answer| answer["source"] == "chief" && answer["delivery"] == "queued")
        {
            let age = answer["created_at_ms"]
                .as_i64()
                .map(|at| util::age(at / 1000))
                .unwrap_or_default();
            lines.push(Line {
                segments: vec![
                    segment(1, "◐", style(BLUE)),
                    segment(
                        3,
                        format!(
                            "Waiting for {} to read its answer",
                            view.session_name(answer["session_id"].as_str())
                        ),
                        style(FG),
                    ),
                ],
                right: vec![part(age, dim(FG_DARK))],
                ..Line::default()
            });
        }
        if records(snapshot, "summary").any(|summary| {
            matches!(
                summary["request"]["state"].as_str(),
                Some("queued" | "pending")
            )
        }) {
            lines.push(Line::new(vec![
                segment(1, "◐", style(BLUE)),
                segment(3, "Writing the summary you asked for", style(FG)),
            ]));
        }
        lines
    }

    /// Today's answers and escalations, newest first, each with the row it acts as.
    fn chief_today(&self, view: &ChiefView, snapshot: &Value) -> Vec<(i64, String, ChiefRow)> {
        let since = midnight_ms();
        let mut today: Vec<(i64, String, ChiefRow)> = Vec::new();
        for answer in records(snapshot, "answer").filter(|answer| answer["delivery"] != "cancelled")
        {
            let Some(at) = answer["created_at_ms"].as_i64().filter(|at| *at >= since) else {
                continue;
            };
            let who = if answer["source"] == "theo" {
                "You answered"
            } else {
                "Answered"
            };
            let overruled = if answer["overrule"].is_null() {
                ""
            } else {
                " · overruled"
            };
            today.push((
                at,
                format!(
                    "{who} {}: {}{overruled}",
                    view.session_name(answer["session_id"].as_str()),
                    text(&answer["answer"])
                ),
                ChiefRow::Answer(answer.clone()),
            ));
        }
        for decision in
            records(snapshot, "decision").filter(|decision| decision["category"].is_string())
        {
            let Some(at) = decision["created_at_ms"].as_i64().filter(|at| *at >= since) else {
                continue;
            };
            today.push((
                at,
                format!(
                    "Sent to you: {} · {}",
                    category(decision),
                    text(&decision["question"])
                ),
                ChiefRow::Sent(decision.clone()),
            ));
        }
        today.sort_by_key(|(at, _, _)| std::cmp::Reverse(*at));
        today
    }

    pub(super) fn chief_tab_hints(&self) -> Vec<(&'static str, &'static str)> {
        let mut hints = vec![("j k", "move")];
        match self.chief_tab_rows.get(self.panels[CHIEF_PANEL].selected) {
            Some(ChiefRow::Decision(decision))
                if decision["state"] == "open"
                    && self.chief_status.as_ref().is_ok_and(|snapshot| {
                        ChiefView::new(snapshot.clone()).is_recovery(decision)
                    }) =>
            {
                hints.extend([("y", "it arrived"), ("x", "stop safely"), ("i", "details")])
            }
            Some(ChiefRow::Decision(decision)) if is_escalation(decision) => {
                hints.extend([("⏎", "answer"), ("i", "details")])
            }
            Some(ChiefRow::Decision(_)) => hints.push(("i", "details")),
            Some(ChiefRow::Answer(_)) => {
                hints.extend([("⏎", "open that session"), ("o", "overrule an answer")])
            }
            Some(ChiefRow::Sent(_)) => hints.push(("⏎", "open that session")),
            None => {}
        }
        hints
    }

    /// The Chief view on its root, as the tab's keys drive it; never its inbox first.
    fn open_chief_for_tab(&mut self) {
        if let Some(view) = self.chief_view.as_mut() {
            view.suspended = false;
            self.refresh_chief_view();
            return;
        }
        // What the tab shows is what its keys act on.
        let shown = self.chief_status.clone().map_err(|message| {
            crate::teams::TeamError::new(crate::teams::ErrorCode::Unavailable, message)
        });
        match shown.or_else(|_| crate::chief::host_snapshot()) {
            Ok(snapshot) => {
                let mut view = ChiefView::new(snapshot);
                view.habits_seen_ms = crate::chief::habits_seen_ms();
                view.check_answers = self.chief_check_answers.clone();
                self.chief_view = Some(view);
            }
            Err(error) => {
                let mut view = ChiefView::new(json!({"records": []}));
                view.read_failed(format!("Chief view unavailable: {}", error.message));
                view.results(format!("Host review unavailable: {}. No state or permission is inferred. Esc returns without any action.",
                    crate::approval::escape_hiding(&error.message, "", "")));
                self.chief_view = Some(view);
            }
        }
    }

    pub(super) fn handle_chief_tab(&mut self, key: Key) -> String {
        let selected = self
            .chief_tab_rows
            .get(self.panels[CHIEF_PANEL].selected)
            .cloned();
        match (&key, selected) {
            (Key::Char('c' | 'p' | 'r' | 'l' | 'h'), _) => {
                self.open_chief_for_tab();
                self.handle_chief_key(key);
            }
            (Key::Char('t' | 'a' | 'd'), _)
                if self.chief_status.as_ref().is_ok_and(|snapshot| {
                    ChiefView::new(snapshot.clone()).recovery_check_takes(&key)
                }) =>
            {
                self.open_chief_for_tab();
                self.handle_chief_key(key);
            }
            (Key::Char('y' | 'x' | 'i'), Some(ChiefRow::Decision(decision))) => {
                self.open_chief_for_tab();
                if let Some(view) = self.chief_view.as_mut() {
                    view.act_on_decision(&decision, &key);
                }
                if self.chief_view.as_ref().is_some_and(ChiefView::at_root) {
                    self.chief_view = None;
                }
            }
            (Key::Enter, Some(ChiefRow::Decision(decision))) if is_escalation(&decision) => {
                self.open_chief_for_tab();
                let id = decision["id"].as_str().unwrap_or_default();
                if !self
                    .chief_view
                    .as_mut()
                    .is_some_and(|view| view.open_inbox_on(id))
                {
                    if self.chief_view.as_ref().is_some_and(ChiefView::at_root) {
                        self.chief_view = None;
                    }
                    self.refresh_chief_status();
                    self.build_chief_tab();
                    return "That question no longer needs you".into();
                }
            }
            (Key::Enter, Some(ChiefRow::Answer(record) | ChiefRow::Sent(record))) => {
                let Some(session) = record["session_id"].as_str() else {
                    return "That row names no session".into();
                };
                if !self
                    .all_sessions()
                    .iter()
                    .any(|candidate| candidate.session == session)
                {
                    return "That session is no longer running".into();
                }
                return self.manager_async(&["open", session]);
            }
            (Key::Char('o'), Some(ChiefRow::Answer(answer))) => {
                self.open_chief_for_tab();
                if let Some(view) = self.chief_view.as_mut() {
                    view.open_answers(None);
                    view.select_record(&answer);
                }
                self.handle_chief_key(key);
            }
            _ => {}
        }
        String::new()
    }
}
