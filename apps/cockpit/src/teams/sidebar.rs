//! Task-linked rows for the existing sessions panel. Only real session rows target terminals.

use super::{ErrorCode, Role, TaskKey, TeamError};
use crate::{approval::escape_hiding, sessions::{self, Session}, util};
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarRow {
    Team { task: TaskKey },
    Session { session_id: String },
    PlannedQa { task: TaskKey },
}

pub fn list<'a>(snapshot: &'a Value, key: &str) -> &'a [Value] {
    snapshot[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}

pub fn task_of(value: &Value) -> Option<TaskKey> {
    if let Some(key) = value["task_key"].as_str() { return serde_json::from_str(key).ok(); }
    Some(TaskKey { project: value["project"].as_str()?.into(), task_id: value["task_id"].as_str()?.into() })
}

pub fn task_key(task: &TaskKey) -> String {
    serde_json::to_string(task).expect("TaskKey contains only strings")
}

pub fn shown(text: &str) -> String {
    escape_hiding(text, "", "").replace('\n', "⏎")
}

pub fn matches_session(session: &Session, id: &str) -> bool {
    session.session == id || session.conversation.as_deref() == Some(id)
}

pub fn membership<'a>(snapshot: &'a Value, session: &Session) -> Option<&'a Value> {
    list(snapshot, "members").iter().find(|member| member["session_id"].as_str().is_some_and(|id| matches_session(session, id)))
}

pub fn member_task(snapshot: &Value, session: &Session) -> Option<TaskKey> {
    task_of(membership(snapshot, session)?)
}

pub fn role_label(role: Role) -> &'static str {
    match role { Role::Designer => "Designer", Role::Developer => "Developer", Role::Qa => "QA" }
}

pub fn role(member: &Value) -> Option<Role> {
    serde_json::from_value(member["role"].clone()).ok()
}

pub fn project_name(snapshot: &Value, task: &TaskKey, sessions: &[Session]) -> String {
    if let Some(session) = sessions.iter().find(|session| member_task(snapshot, session).as_ref() == Some(task)) {
        return session.project.clone();
    }
    list(snapshot, "sessions").iter().find(|session| session["project"] == task.project)
        .and_then(|session| session["project_name"].as_str()).filter(|name| !name.is_empty()).map(str::to_string)
        .unwrap_or_else(|| task.project.rsplit(['/', ':']).next().unwrap_or(&task.project).trim_end_matches(".git").into())
}

pub fn member_count(snapshot: &Value, task: &TaskKey, sessions: &[Session]) -> usize {
    sessions.iter().filter(|session| member_task(snapshot, session).as_ref() == Some(task)).count()
}

pub fn has_qa_plan(snapshot: &Value, task: &TaskKey) -> bool {
    list(snapshot, "qa_plans").iter().any(|plan| task_of(plan).as_ref() == Some(task))
}

pub fn attention(snapshot: &Value, task: &TaskKey) -> Option<String> {
    let scoped = |value: &&Value| task_of(value).as_ref() == Some(task);
    if list(snapshot, "proposals").iter().filter(scoped).any(|proposal| proposal["status"] == "pending") {
        return Some("Needs you: approve team links".into());
    }
    if let Some(escalation) = list(snapshot, "escalations").iter().filter(scoped).find(|entry| entry["status"] == "open") {
        return Some(format!("Needs you: {}", shown(escalation["reason"].as_str().unwrap_or("Open escalation"))));
    }
    if list(snapshot, "messages").iter().any(|message| matches!(message["state"].as_str(), Some("uncertain" | "failed"))
        && envelope(message).get("task").and_then(task_of).as_ref() == Some(task)) {
        return Some("Needs you: delivery uncertain or failed".into());
    }
    if let Some(spawn) = list(snapshot, "spawns").iter().filter(scoped).find(|spawn| matches!(spawn["status"].as_str(), Some("failed" | "refused"))) {
        return Some(format!("Needs you: {} start {}", shown(spawn["role"].as_str().unwrap_or("Teammate")), shown(spawn["status"].as_str().unwrap())));
    }
    for sleep in list(snapshot, "sleeps").iter().filter(|sleep| sleep["state"] == "failed") {
        let Some(id) = sleep["session_id"].as_str() else { continue };
        if let Some(member) = list(snapshot, "members").iter().filter(scoped).find(|member| member["session_id"] == id) {
            let role = role(member).map(role_label).unwrap_or("Teammate");
            return Some(format!("Needs you: {role} wake failed: {}", shown(sleep["error"].as_str().unwrap_or("Wake error unavailable"))));
        }
    }
    None
}

fn envelope(message: &Value) -> Value {
    serde_json::from_str(message["envelope"].as_str().unwrap_or("null")).unwrap_or(Value::Null)
}

pub fn handoff_summary(snapshot: &Value, task: &TaskKey) -> String {
    let Some(handoffs) = snapshot["handoffs"].as_array() else { return "Handoff evidence unavailable".into() };
    let Some(handoff) = handoffs.iter().find(|handoff| task_of(handoff).as_ref() == Some(task) && handoff["status"] == "current") else {
        return "No handoff recorded".into();
    };
    let findings = list(snapshot, "messages").iter().any(|message| {
        let envelope = envelope(message);
        envelope["kind"] == "findings" && task_of(&envelope["task"]).as_ref() == Some(task)
            && envelope["handoff"]["id"] == handoff["handoff_id"]
    });
    if findings { return "QA findings received".into(); }
    "Handoff submitted".into()
}

pub fn qa_status(snapshot: &Value, task: &TaskKey) -> String {
    let current = list(snapshot, "handoffs").iter().find(|handoff| task_of(handoff).as_ref() == Some(task) && handoff["status"] == "current");
    let spawn = list(snapshot, "spawns").iter().filter(|spawn| task_of(spawn).as_ref() == Some(task) && spawn["role"] == "qa"
        && current.is_none_or(|handoff| spawn["commit_id"] == handoff["payload"]["commit"]))
        .max_by_key(|spawn| spawn["created_at"].as_i64().unwrap_or(0));
    match spawn.and_then(|spawn| spawn["status"].as_str()) {
        Some("pending" | "started") => "QA starting · not yet linked".into(),
        Some("bound") => "QA unavailable · waiting reason unavailable".into(),
        Some("failed") => "QA start failed".into(),
        Some("refused") => "QA start refused".into(),
        _ => "QA planned · Fresh reviewer starts at handoff".into(),
    }
}

pub fn activity(session: &Session) -> String {
    let request = sessions::activity_text(session);
    if !request.is_empty() {
        let age = if session.since > 0 && session.since <= util::now_secs() { format!("state {}", util::age(session.since)) }
            else { "State age unavailable".into() };
        return format!("{} · {age}", shown(&request));
    }
    if !session.detail.trim().is_empty() {
        return format!("Reported: {} · state {}", shown(&session.detail), util::age(session.since));
    }
    if let Some(result) = &session.last_result {
        if !result.outcome.trim().is_empty() { return format!("Latest: {} · {} ago", shown(&result.outcome), util::age(result.completed_at)); }
    }
    "No activity update".into()
}

pub fn status(session: &Session) -> String {
    if let Some(slept_at) = session.asleep { return format!("Asleep {}", util::age(slept_at)); }
    match sessions::activity_label(session) {
        "Ready" => "Turn ended".into(),
        other => other.into(),
    }
}

pub fn grouped_rows(snapshot: &Value, sessions: &[Session], collapsed: &HashSet<String>) -> Result<Vec<SidebarRow>, TeamError> {
    if !snapshot["members"].is_array() || !snapshot["sessions"].is_array() {
        return Err(TeamError::new(ErrorCode::Invalid, "team snapshot lacks membership or verified sessions"));
    }
    let mut member_ids = HashSet::new();
    for member in list(snapshot, "members") {
        if role(member).is_none() || task_of(member).is_none_or(|task| task.project.is_empty() || task.task_id.is_empty())
            || member["session_id"].as_str().is_none_or(|id| id.is_empty() || !member_ids.insert(id)) {
            return Err(TeamError::new(ErrorCode::Invalid, "invalid team member identity, role or task"));
        }
    }
    let mut tasks = Vec::new();
    for field in ["members", "proposals", "escalations", "spawns", "handoffs", "qa_plans"] {
        for entry in list(snapshot, field) {
            let Some(task) = task_of(entry) else { continue };
            if !tasks.contains(&task) { tasks.push(task); }
        }
    }
    for message in list(snapshot, "messages") {
        if let Some(task) = task_of(&envelope(message)["task"]) {
            if !tasks.contains(&task) { tasks.push(task); }
        }
    }
    tasks.retain(|task| sessions.iter().any(|session| member_task(snapshot, session).as_ref() == Some(task))
        || attention(snapshot, task).is_some());
    tasks.sort_by_key(|task| sessions.iter().position(|session| member_task(snapshot, session).as_ref() == Some(task)).unwrap_or(usize::MAX));
    let mut projects: Vec<String> = Vec::new();
    for project in sessions.iter().map(|session| session.project.clone()).chain(tasks.iter().map(|task| project_name(snapshot, task, sessions))) {
        if !projects.contains(&project) { projects.push(project); }
    }
    let mut rows = Vec::new();
    for project in projects {
        for task in tasks.iter().filter(|task| project_name(snapshot, task, sessions) == project) {
            rows.push(SidebarRow::Team { task: task.clone() });
            if collapsed.contains(&task_key(task)) { continue; }
            for role in [Role::Designer, Role::Developer, Role::Qa] {
                for member in list(snapshot, "members").iter().filter(|member| task_of(member).as_ref() == Some(task) && self::role(member) == Some(role)) {
                    let id = member["session_id"].as_str().ok_or_else(|| TeamError::new(ErrorCode::Invalid, "team member lacks session identity"))?;
                    if let Some(session) = sessions.iter().find(|session| matches_session(session, id)) {
                        rows.push(SidebarRow::Session { session_id: session.session.clone() });
                    }
                }
            }
            if has_qa_plan(snapshot, task) && !list(snapshot, "members").iter().any(|member| task_of(member).as_ref() == Some(task) && role(member) == Some(Role::Qa)) {
                rows.push(SidebarRow::PlannedQa { task: task.clone() });
            }
        }
        rows.extend(sessions.iter().filter(|session| session.project == project && membership(snapshot, session).is_none())
            .map(|session| SidebarRow::Session { session_id: session.session.clone() }));
    }
    Ok(rows)
}
