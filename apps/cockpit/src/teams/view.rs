//! The Team view's content, as plain text the column draws: what Theo reads before approving.
//! Everything an agent wrote is shown escaped, never interpreted by the terminal.

use crate::approval::escape_hiding;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row { Proposal(usize), Member(usize), Escalation(usize), Uncertain(usize), Message(usize), FailedStart(usize) }

pub struct TeamView {
    pub snapshot: Value,
    pub is_history: bool,
    pub selected: usize,
}

fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => escape_hiding(text, "", ""),
        Value::Null => String::new(),
        other => escape_hiding(&other.to_string(), "", ""),
    }
}

/// "remote:git@github.com:example/project-a.git" → "project-a".
fn short_project(canonical: &str) -> String {
    let tail = canonical.rsplit(['/', ':']).next().unwrap_or(canonical);
    shown(&Value::String(tail.trim_end_matches(".git").to_string()))
}

impl TeamView {
    pub fn new(snapshot: Value) -> Self {
        TeamView { snapshot, is_history: false, selected: 0 }
    }

    fn list(&self, key: &str) -> &[Value] {
        self.snapshot[key].as_array().map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn proposal(&self, index: usize) -> &Value { &self.list("proposals")[index] }
    pub fn member(&self, index: usize) -> &Value { &self.list("members")[index] }
    pub fn escalation(&self, index: usize) -> &Value { &self.list("escalations")[index] }
    pub fn message(&self, index: usize) -> &Value { &self.list("messages")[index] }
    pub fn sessions(&self) -> &[Value] { self.list("sessions") }

    pub fn envelope(&self, index: usize) -> Value {
        serde_json::from_str(self.message(index)["envelope"].as_str().unwrap_or("null")).unwrap_or(Value::Null)
    }

    /// (session, role, agent) of a request, in role order.
    pub fn mapping(&self, index: usize) -> Vec<Value> {
        serde_json::from_str(self.proposal(index)["mapping"].as_str().unwrap_or("[]")).unwrap_or_default()
    }

    pub fn rows(&self) -> Vec<Row> {
        if self.is_history {
            return (0..self.list("messages").len()).rev().map(Row::Message).collect();
        }
        let indices = |key: &str, keep: &dyn Fn(&Value) -> bool| -> Vec<usize> {
            self.list(key).iter().enumerate().filter(|(_, value)| keep(value)).map(|(index, _)| index).collect()
        };
        let mut rows: Vec<Row> = indices("proposals", &|proposal| proposal["status"] == "pending").into_iter().map(Row::Proposal).collect();
        rows.extend((0..self.list("members").len()).map(Row::Member));
        rows.extend(indices("escalations", &|escalation| escalation["status"] == "open").into_iter().map(Row::Escalation));
        rows.extend(indices("messages", &|message| message["state"] == "uncertain").into_iter().map(Row::Uncertain));
        rows.extend(indices("spawns", &|spawn| spawn["status"] == "failed").into_iter().map(Row::FailedStart));
        rows
    }

    /// Pending requests, open escalations and uncertain deliveries: what needs Theo.
    pub fn attention(snapshot: &Value) -> usize {
        let count = |key: &str, field: &str, value: &str| snapshot[key].as_array().into_iter().flatten().filter(|item| item[field] == value).count();
        count("proposals", "status", "pending") + count("escalations", "status", "open") + count("messages", "state", "uncertain")
            + count("spawns", "status", "failed")
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.rows().get(self.selected).copied()
    }

    pub fn move_by(&mut self, delta: isize) {
        let last = self.rows().len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn row_text(&self, row: Row) -> String {
        match row {
            Row::Proposal(index) => {
                let proposal = self.proposal(index);
                let from = if proposal["created_by"] == "host" { "you".to_string() } else { shown(&proposal["sender"]) };
                format!("request {}/{} · revision {} · from {from}", short_project(proposal["project"].as_str().unwrap_or("")),
                    shown(&proposal["task_id"]), proposal["revision"])
            },
            Row::Member(index) => {
                let member = self.member(index);
                format!("{:<9} {} ({}) · team {}/{}", shown(&member["role"]), shown(&member["session_id"]), shown(&member["agent"]),
                    short_project(member["project"].as_str().unwrap_or("")), shown(&member["task_id"]))
            },
            Row::Escalation(index) => {
                let escalation = self.escalation(index);
                format!("! escalated {}: {}", shown(&escalation["thread_id"]), shown(&escalation["reason"]))
            },
            Row::Uncertain(index) => {
                let envelope = self.envelope(index);
                format!("? uncertain delivery {} → {}", shown(&self.message(index)["message_id"]), shown(&envelope["recipient"]["role"]))
            },
            Row::FailedStart(index) => {
                let spawn = &self.list("spawns")[index];
                format!("! new {} ({}) did not start in {}", shown(&spawn["role"]), shown(&spawn["agent"]), shown(&spawn["directory"]))
            },
            Row::Message(index) => {
                let envelope = self.envelope(index);
                let body = shown(&envelope["body"]);
                format!("{} {} → {} · {}: {}", shown(&envelope["kind"]), shown(&envelope["sender"]["role"]), shown(&envelope["recipient"]["role"]),
                    shown(&self.message(index)["state"]), body.lines().next().unwrap_or(""))
            },
        }
    }

    /// The selected row in full: the exact mapping of a request, a whole message.
    pub fn detail(&self) -> Vec<String> {
        let Some(row) = self.selected_row() else {
            return vec!["Nothing needs you. n creates a team request from live sessions.".into()];
        };
        match row {
            Row::Proposal(index) => {
                let proposal = self.proposal(index);
                let mut lines = vec![format!("Task: {} / {}", shown(&proposal["project"]), shown(&proposal["task_id"])),
                    format!("Revision {} · reason: {}", proposal["revision"], shown(&proposal["reason"]))];
                let recruits: Vec<Value> = serde_json::from_str(proposal["recruits"].as_str().unwrap_or("[]")).unwrap_or_default();
                let agent = shown(&proposal["spawn_agent"]);
                for member in self.mapping(index) {
                    let label = if recruits.is_empty() { shown(&member["role"]) } else { format!("asker ({})", shown(&member["role"])) };
                    lines.push(format!("{label}: {} ({})", shown(&member["session_id"]), shown(&member["agent"])));
                }
                for recruit in &recruits {
                    if recruit["role"] == "qa" {
                        lines.push(format!("QA: a fresh reviewer for each handoff ({agent}): {}", shown(&recruit["brief"])));
                    } else {
                        lines.push(format!("new {} ({agent}): {}", shown(&recruit["role"]), shown(&recruit["brief"])));
                    }
                }
                lines.push("Approving lets only these sessions message each other about this task. It grants no tool, scope, install or Git permission.".into());
                lines.push("a approve exactly this · e edit · x reject".into());
                lines
            },
            Row::Member(index) => vec![self.row_text(row),
                format!("u unlink {}: stops its messages and cancels pending deliveries and retries", shown(&self.member(index)["session_id"]))],
            Row::Escalation(_) => vec![self.row_text(row), "s resolve it with a note for the team".into()],
            Row::Uncertain(index) => {
                let envelope = self.envelope(index);
                vec![self.row_text(row), format!("{}", shown(&envelope["body"])), format!("Last error: {}", shown(&self.message(index)["error"])),
                    "It may have reached the session: h it arrived (mark handled) · c cancel it. It is never sent twice.".into()]
            },
            Row::FailedStart(_) => vec![self.row_text(row), "It was started once and never reported; it is not started again. Start it by hand or ask the team to request it again.".into()],
            Row::Message(index) => crate::teams::envelope_text(&self.envelope(index)).lines().map(|line| shown(&Value::String(line.into()))).collect(),
        }
    }
}
