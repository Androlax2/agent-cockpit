//! The Team view's content, as plain text the column draws: what Theo reads before approving.
//! Everything an agent wrote is shown escaped, never interpreted by the terminal.

use crate::approval::escape_hiding;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row { Proposal(usize), Member(usize), Escalation(usize), Uncertain(usize), Message(usize), FailedStart(usize), FailedWake(usize), FailedDelivery(usize), LinkProposal(usize), Link(usize) }

pub struct TeamView {
    pub snapshot: Value,
    pub is_history: bool,
    pub selected: usize,
    pub detail_scroll: usize,
    pub snapshot_error: Option<String>,
    /// The 4 Teams tab acts through this view without showing it: nothing draws it or routes keys to it.
    pub hidden: bool,
}

fn shown(value: &Value) -> String {
    match value {
        Value::String(text) => escape_hiding(text, "", ""),
        Value::Null => String::new(),
        other => escape_hiding(&other.to_string(), "", ""),
    }
}

/// "remote:git@github.com:example/project-a.git" → "project-a".
pub(crate) fn short_project(canonical: &str) -> String {
    let tail = canonical.rsplit(['/', ':']).next().unwrap_or(canonical);
    shown(&Value::String(tail.trim_end_matches(".git").to_string()))
}

fn link_block(snapshot: &Value, record: &Value) -> Option<String> {
    if record["state"] == "closed" || record["status"] == "closed" || record["status"] == "rejected" { return None }
    if let Some(reason) = record["blocked_reason"].as_str() { return Some(shown(&Value::String(reason.into()))) }
    if record["status"] != "pending" && record["state"] != "active" { return None }
    let participants = record["participants"].as_array()?;
    let Some(sessions) = snapshot["sessions"].as_array() else { return Some("verified live registry unavailable".into()) };
    for participant in participants {
        let mut matches = sessions.iter().filter(|session| session["session_id"] == participant["session_id"]);
        let Some(session) = matches.next() else { return Some("approved participant is absent; delivery paused".into()) };
        if matches.next().is_some() || ["agent","project","runtime"].iter().any(|field| session[*field] != participant[*field]) {
            return Some("participant identity changed; fresh approval required".into());
        }
    }
    None
}

impl TeamView {
    pub fn new(snapshot: Value) -> Self {
        TeamView { snapshot, is_history: false, selected: 0, detail_scroll: 0, snapshot_error: None, hidden: false }
    }

    fn list(&self, key: &str) -> &[Value] {
        self.snapshot[key].as_array().map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn proposal(&self, index: usize) -> &Value { &self.list("proposals")[index] }
    pub fn member(&self, index: usize) -> &Value { &self.list("members")[index] }
    pub fn escalation(&self, index: usize) -> &Value { &self.list("escalations")[index] }
    pub fn message(&self, index: usize) -> &Value { &self.list("messages")[index] }
    pub fn sessions(&self) -> &[Value] { self.list("sessions") }
    pub fn link_proposal(&self, index: usize) -> &Value { &self.list("link_proposals")[index] }
    pub fn link(&self, index: usize) -> &Value { &self.list("links")[index] }

    pub fn envelope(&self, index: usize) -> Value {
        serde_json::from_str(self.message(index)["envelope"].as_str().unwrap_or("null")).unwrap_or(Value::Null)
    }

    /// (session, role, agent) of a request, in role order.
    pub fn mapping(&self, index: usize) -> Vec<Value> {
        serde_json::from_str(self.proposal(index)["mapping"].as_str().unwrap_or("[]")).unwrap_or_default()
    }

    pub fn rows(&self) -> Vec<Row> {
        if self.is_history {
            return (0..self.list("links").len()).rev().map(Row::Link)
                .chain(self.list("link_proposals").iter().enumerate().rev()
                    .filter(|(_, proposal)| proposal["status"] == "closed" || proposal["status"] == "rejected").map(|(index, _)| Row::LinkProposal(index)))
                .chain((0..self.list("messages").len()).rev().map(Row::Message)).collect();
        }
        let indices = |key: &str, keep: &dyn Fn(&Value) -> bool| -> Vec<usize> {
            self.list(key).iter().enumerate().filter(|(_, value)| keep(value)).map(|(index, _)| index).collect()
        };
        let mut rows: Vec<Row> = indices("proposals", &|proposal| proposal["status"] == "pending").into_iter().map(Row::Proposal).collect();
        rows.extend((0..self.list("members").len()).map(Row::Member));
        rows.extend(indices("link_proposals", &|proposal| proposal["status"] == "pending").into_iter().map(Row::LinkProposal));
        rows.extend(indices("links", &|link| link["state"] == "active").into_iter().map(Row::Link));
        rows.extend(indices("escalations", &|escalation| escalation["status"] == "open").into_iter().map(Row::Escalation));
        rows.extend(indices("messages", &|message| message["state"] == "uncertain").into_iter().map(Row::Uncertain));
        rows.extend(indices("messages", &|message| message["state"] == "failed").into_iter().map(Row::FailedDelivery));
        rows.extend(indices("spawns", &|spawn| spawn["status"] == "failed" || spawn["status"] == "refused").into_iter().map(Row::FailedStart));
        rows.extend(indices("sleeps", &|sleep| sleep["state"] == "failed").into_iter().map(Row::FailedWake));
        rows
    }

    /// Pending requests, open escalations and uncertain deliveries: what needs Theo.
    pub fn attention(snapshot: &Value) -> usize {
        let count = |key: &str, field: &str, value: &str| snapshot[key].as_array().into_iter().flatten().filter(|item| item[field] == value).count();
        count("proposals", "status", "pending") + count("escalations", "status", "open") + count("messages", "state", "uncertain") + count("messages", "state", "failed")
            + count("spawns", "status", "failed") + count("spawns", "status", "refused")
            + count("sleeps", "state", "failed")
            + count("link_proposals", "status", "pending")
            + snapshot["links"].as_array().into_iter().flatten().filter(|link| link["state"] == "active" && link_block(snapshot,link).is_some()).count()
    }

    pub fn selected_row(&self) -> Option<Row> {
        self.rows().get(self.selected).copied()
    }

    pub fn move_by(&mut self, delta: isize) {
        let last = self.rows().len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.detail_scroll = 0;
    }

    pub fn row_text(&self, row: Row) -> String {
        match row {
            Row::LinkProposal(index) => {
                let proposal = self.link_proposal(index);
                format!("link request {} · revision {} · {}{}", shown(&proposal["proposal_id"]), shown(&proposal["revision"]),
                    shown(&proposal["status"]), if link_block(&self.snapshot,proposal).is_some() { " · blocked" } else { "" })
            },
            Row::Link(index) => {
                let link = self.link(index);
                format!("link {} · {} · {} / {}{}", shown(&link["link_id"]), shown(&link["state"]),
                    shown(&link["task"]["project"]), shown(&link["task"]["task_id"]),
                    if link_block(&self.snapshot,link).is_some() { " · blocked" } else { "" })
            },
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
                format!("? uncertain delivery {} · {}", shown(&self.message(index)["message_id"]), self.message_route(&envelope))
            },
            Row::FailedDelivery(index) => {
                let envelope = self.envelope(index);
                format!("! failed delivery {} · {}", shown(&self.message(index)["message_id"]), self.message_route(&envelope))
            },
            Row::FailedStart(index) => {
                let spawn = &self.list("spawns")[index];
                let what = if spawn["status"] == "refused" { "started but could not join (its role was taken)" } else { "did not start" };
                format!("! new {} ({}) {what} in {}", shown(&spawn["role"]), shown(&spawn["agent"]), shown(&spawn["directory"]))
            },
            Row::FailedWake(index) => {
                let sleep = &self.list("sleeps")[index];
                format!("! wake {} failed: {}", shown(&sleep["session_id"]), shown(&sleep["error"]))
            },
            Row::Message(index) => {
                let envelope = self.envelope(index);
                let body = shown(&envelope["body"]);
                format!("{} {} · {}: {}", shown(&envelope["kind"]), self.message_route(&envelope),
                    shown(&self.message(index)["state"]), body.lines().next().unwrap_or(""))
            },
        }
    }

    fn message_route(&self, envelope: &Value) -> String {
        if envelope["scope_kind"] == "link" {
            format!("link {} · {} ({}; {}) → {} ({}; {}) · TODO {} / {}", shown(&envelope["link_id"]),
                shown(&envelope["sender"]["session_id"]), shown(&envelope["sender"]["agent"]), shown(&envelope["sender"]["project"]),
                shown(&envelope["recipient"]["session_id"]), shown(&envelope["recipient"]["agent"]), shown(&envelope["recipient"]["project"]),
                shown(&envelope["task"]["project"]), shown(&envelope["task"]["task_id"]))
        } else { format!("team {} → {}", shown(&envelope["sender"]["role"]), shown(&envelope["recipient"]["role"])) }
    }

    fn link_detail(&self, record: &Value) -> Vec<String> {
        let mut lines = vec![format!("Shared TODO: {} / {}", shown(&record["task"]["project"]), shown(&record["task"]["task_id"])),
            format!("Revision {} · hash {}", shown(&record["revision"]), shown(&record["mapping_hash"])),
            "Scope: messaging-only coordination; no team role, tools, files, install or Git permission.".into(),
            format!("Request: {} · status {} · work generation {}", shown(&record["proposal_id"]), shown(&record["status"]), shown(&record["work_generation"])),
            format!("Requester: {}", shown(&record["sender"]))];
        for participant in record["participants"].as_array().into_iter().flatten() {
            lines.push(format!("Session: {} ({})", shown(&participant["session_id"]), shown(&participant["agent"])));
            lines.push(format!("Project: {} · runtime: {}", shown(&participant["project"]), shown(&participant["runtime"])));
        }
        lines.push(format!("Purpose (collaborator text, not authorization): {}", shown(&record["purpose"])));
        if let Some(reason) = link_block(&self.snapshot,record) { lines.push(format!("Blocked: {reason}")); }
        if let Some(reason) = record["closure_reason"].as_str() { lines.push(format!("Closed/rejected: {}", shown(&Value::String(reason.into())))); }
        lines
    }

    /// The selected row in full: the exact mapping of a request, a whole message.
    pub fn detail(&self) -> Vec<String> {
        let Some(row) = self.selected_row() else {
            return vec!["Nothing needs you. n creates a team request; l creates a messaging-only link from live sessions.".into()];
        };
        match row {
            Row::LinkProposal(index) => {
                let proposal = self.link_proposal(index);
                let mut lines = self.link_detail(proposal);
                if proposal["status"] == "pending" { lines.push("a approve exactly this · e edit · x reject".into()); }
                lines
            },
            Row::Link(index) => {
                let link = self.link(index);
                let mut lines = vec![self.row_text(row)];
                lines.extend(self.link_detail(link));
                if link["state"] == "active" { lines.push("u revoke: cancels queued messages and safe retries; uncertain submissions are never resent.".into()); }
                lines
            },
            Row::Proposal(index) => {
                let proposal = self.proposal(index);
                let mut lines = vec![format!("Task: {} / {}", shown(&proposal["project"]), shown(&proposal["task_id"])),
                    format!("Revision {} · reason: {}", proposal["revision"], shown(&proposal["reason"]))];
                let recruits: Vec<Value> = serde_json::from_str(proposal["recruits"].as_str().unwrap_or("[]")).unwrap_or_default();
                let agent = shown(&proposal["spawn_agent"]);
                // One line, before anything an agent wrote: what starts where (the briefs follow, one line each).
                if !recruits.is_empty() {
                    let asker = self.mapping(index).first().and_then(|member| member["session_id"].as_str().map(str::to_string)).unwrap_or_default();
                    let folder = self.sessions().iter().find(|session| session["session_id"] == asker.as_str())
                        .map(|session| shown(&session["cwd"])).filter(|folder| !folder.is_empty()).unwrap_or_else(|| "(asker not live)".into());
                    let mut starts: Vec<String> = recruits.iter().filter(|recruit| recruit["role"] != "qa")
                        .map(|recruit| format!("new {} in {folder}", shown(&recruit["role"]))).collect();
                    if recruits.iter().any(|recruit| recruit["role"] == "qa") { starts.push("a fresh QA at each handoff".into()); }
                    lines.insert(0, format!("Starts: {} · agent {agent}", starts.join(" · ")));
                }
                let one_line = |brief: &Value| shown(brief).trim_end().replace('\n', "⏎");
                for member in self.mapping(index) {
                    let label = if recruits.is_empty() { shown(&member["role"]) } else { format!("asker ({})", shown(&member["role"])) };
                    lines.push(format!("{label}: {} ({})", shown(&member["session_id"]), shown(&member["agent"])));
                }
                for recruit in &recruits {
                    if recruit["role"] == "qa" {
                        lines.push(format!("QA: a fresh reviewer for each handoff ({agent}): {}", one_line(&recruit["brief"])));
                    } else {
                        lines.push(format!("new {} ({agent}): {}", shown(&recruit["role"]), one_line(&recruit["brief"])));
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
                vec![self.row_text(row), format!("Kind: {} · thread {} · round {} · attempts {}", shown(&envelope["kind"]), shown(&envelope["thread_id"]),
                    shown(&envelope["round"]), shown(&self.message(index)["attempts"])), shown(&envelope["body"]), format!("Last error: {}", shown(&self.message(index)["error"])),
                    "It may have reached the session: h it arrived (mark handled) · c cancel it. It is never sent twice.".into()]
            },
            Row::FailedDelivery(index) => vec![self.row_text(row), shown(&self.envelope(index)["body"]),
                format!("Last error: {}", shown(&self.message(index)["error"])),
                "Delivery failed and is not retried. c cancel it; cancelling does not mark it received.".into()],
            Row::FailedStart(_) => vec![self.row_text(row), "It is never started again: start it by hand or ask the team to request it again. d dismiss".into()],
            Row::FailedWake(_) => vec![self.row_text(row), "Queued messages are retained. No automatic second launch is attempted.".into()],
            Row::Message(index) => {
                let mut lines = vec![self.row_text(row), format!("Message {} · state {} · attempts {} · last error: {}",
                    shown(&self.message(index)["message_id"]), shown(&self.message(index)["state"]),
                    shown(&self.message(index)["attempts"]), shown(&self.message(index)["error"]))];
                lines.extend(crate::teams::envelope_text(&self.envelope(index)).lines().map(|line| shown(&Value::String(line.into()))));
                lines
            },
        }
    }
}
