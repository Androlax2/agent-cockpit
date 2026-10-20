use super::*;
use crate::{reviews::Review, todo::{Entry, TodoList}, sessions::{valid_repository, valid_turn_id}};
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub key: String,
    pub repository: String,
    pub title: String,
    pub notes: String,
    pub status: String,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl TaskSnapshot {
    pub fn from_entry(entry: &Entry, repository: &str) -> Self {
        let (title, notes) = match entry { Entry::Typed(item) => (item.text(), item.notes()), Entry::Issue { issue, .. } => (issue.title.clone(), String::new()) };
        Self { key: entry.key(), repository: repository.into(), title, notes, status: entry.status(), extra: Map::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskAssignment {
    pub task: String,
    pub session: String,
    pub assigned_at: i64,
    pub branch: String,
    pub repository: String,
    pub baseline_result: Option<String>,
    pub active: bool,
    pub result: Option<TurnResult>,
    #[serde(default)]
    pub prs: BTreeSet<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskRecord {
    pub snapshot: TaskSnapshot,
    pub assignments: Vec<TaskAssignment>,
    pub pending: bool,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

impl TaskRecord {
    fn update_snapshot(&mut self, snapshot: &TaskSnapshot) {
        let extra = self.snapshot.extra.clone();
        self.snapshot = snapshot.clone();
        self.snapshot.extra.extend(extra);
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PullRequestMemory {
    pub repository: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub branch: String,
    pub head_oid: String,
    pub decision: String,
    pub last_seen: i64,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

pub enum TrailTarget { Task(String), Session(String), PullRequest { repository: String, number: i64 } }

fn valid_task_key(key: &str) -> bool {
    (key.len() == 8 && key.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        || key.rsplit_once('#').is_some_and(|(repository, number)| valid_repository(repository) && number.parse::<u64>().is_ok_and(|number| number > 0))
}

pub fn valid_pr_url(repository: &str, number: i64, url: &str) -> bool {
    url.strip_prefix("https://github.com/").and_then(|path| path.strip_suffix(&format!("/pull/{number}")))
        .is_some_and(|path| path.eq_ignore_ascii_case(repository))
}

impl State {
    pub(super) fn validate_tasks(&self) -> Result<(), String> {
        if self.tasks.len() > MAX_RECORDS || self.prs.len() > MAX_RECORDS { return Err("too many continuity tasks or PRs".into()); }
        for (key, task) in &self.tasks {
            let snapshot = &task.snapshot;
            if !valid_task_key(key) || snapshot.key != *key || !valid_metadata_text(&snapshot.title, 300)
                || !valid_question(&snapshot.notes) || snapshot.notes.chars().count() > 4000
                || !(snapshot.repository.is_empty() || valid_repository(&snapshot.repository))
                || !["todo", "doing", "done", "proposed"].contains(&snapshot.status.as_str()) || task.assignments.len() > MAX_RECORDS {
                return Err("invalid saved task".into());
            }
            for assignment in &task.assignments {
                if assignment.task != *key || !agents::is_session_id(&assignment.session) || assignment.assigned_at <= 0
                    || !valid_metadata_text(&assignment.branch, 300)
                    || !(assignment.repository.is_empty() || valid_repository(&assignment.repository))
                    || assignment.baseline_result.as_ref().is_some_and(|id| !valid_turn_id(id))
                    || assignment.prs.iter().any(|id| !self.prs.contains_key(id)) {
                    return Err("invalid saved task assignment".into());
                }
                if let Some(result) = &assignment.result { result.validate()?; }
            }
        }
        for (key, pr) in &self.prs {
            if *key != format!("{}#{}", pr.repository, pr.number) || !valid_repository(&pr.repository) || pr.number <= 0
                || !valid_metadata_text(&pr.title, 300) || !valid_metadata_text(&pr.branch, 300)
                || !valid_metadata_text(&pr.head_oid, 64) || !valid_metadata_text(&pr.decision, 40) || pr.last_seen <= 0
                || (!pr.url.is_empty() && !valid_pr_url(&pr.repository, pr.number, &pr.url)) {
                return Err("invalid saved PR".into());
            }
        }
        Ok(())
    }

    fn link_task(&mut self, task: &TaskSnapshot, session: &Session, now: i64, active: bool) {
        let record = self.tasks.entry(task.key.clone()).or_insert_with(|| TaskRecord {
            snapshot: task.clone(), assignments: Vec::new(), pending: false, extra: Map::new(),
        });
        record.update_snapshot(task);
        record.pending = false;
        if let Some(assignment) = record.assignments.iter_mut().find(|assignment| assignment.session == session.session
            && assignment.branch == session.branch && assignment.repository == session.repository) {
            if !assignment.active && active {
                assignment.baseline_result = session.last_result.as_ref().map(|result| result.id.clone());
                assignment.assigned_at = now;
            }
            assignment.active = active;
        } else {
            record.assignments.push(TaskAssignment { task: task.key.clone(), session: session.session.clone(), assigned_at: now,
                branch: session.branch.clone(), repository: session.repository.clone(), baseline_result: session.last_result.as_ref().map(|result| result.id.clone()),
                active, result: None, prs: BTreeSet::new(), extra: Map::new() });
        }
        let memory = self.sessions.entry(session.session.clone()).or_default();
        memory.cwd = session.cwd.clone();
        memory.title = util::collapse(if session.label.is_empty() { &session.title } else { &session.label });
    }
}

impl Continuity {
    pub fn tasks(&self) -> &BTreeMap<String, TaskRecord> { &self.state.tasks }
    pub fn prs(&self) -> &BTreeMap<String, PullRequestMemory> { &self.state.prs }

    pub fn prepare_task(&mut self, task: &TaskSnapshot) -> Result<bool, String> {
        self.change(|state| {
            let record = state.tasks.entry(task.key.clone()).or_insert_with(|| TaskRecord {
                snapshot: task.clone(), assignments: Vec::new(), pending: true, extra: Map::new(),
            });
            record.update_snapshot(task);
            record.pending = true;
            Ok(())
        })
    }

    pub fn assign(&mut self, task: &TaskSnapshot, session: &Session, now: i64) -> Result<bool, String> {
        self.change(|state| {
            for record in state.tasks.values_mut() { for assignment in &mut record.assignments {
                if assignment.session == session.session && (assignment.task != task.key
                    || assignment.branch != session.branch || assignment.repository != session.repository) { assignment.active = false; }
            } }
            state.link_task(task, session, now, true);
            Ok(())
        })
    }

    pub fn remember_task(&mut self, task: &TaskSnapshot) -> Result<bool, String> {
        self.change(|state| {
            if let Some(record) = state.tasks.get_mut(&task.key) { record.update_snapshot(task); }
            Ok(())
        })
    }

    pub fn observe_tasks(&mut self, todos: &TodoList, sessions: &[Session], reviews: &[Review], now: i64) -> Result<bool, String> {
        self.change(|state| {
            let mut incoming: HashMap<String, Vec<TaskSnapshot>> = HashMap::new();
            for item in &todos.items {
                let stored = state.tasks.get(&item.id());
                let linked: Vec<_> = sessions.iter().filter(|session| session.session == item.session()
                    || (session.branch == format!("todo-{}", item.id()) && !session.repository.is_empty()
                        && stored.is_some_and(|record| record.snapshot.repository == session.repository))).collect();
                let repository = linked.first().map(|session| session.repository.as_str()).filter(|repository| !repository.is_empty())
                    .or_else(|| stored.map(|record| record.snapshot.repository.as_str())).unwrap_or("");
                let snapshot = TaskSnapshot::from_entry(&Entry::Typed(item.clone()), repository);
                if let Some(record) = state.tasks.get_mut(&item.id()) { record.update_snapshot(&snapshot); }
                if item.status() == "doing" {
                    for session in linked { incoming.entry(session.session.clone()).or_default().push(snapshot.clone()); }
                }
            }
            for (key, status) in &todos.issues {
                let Some((repository, number)) = key.rsplit_once('#') else { continue };
                let session_id = status.get("session").and_then(Value::as_str).unwrap_or("");
                let linked = sessions.iter().filter(|session| session.repository == repository
                    && (session.session == session_id || session.branch == format!("issue-{number}")));
                let current_status = status.get("status").and_then(Value::as_str).unwrap_or("todo");
                if let Some(record) = state.tasks.get_mut(key) { record.snapshot.status = current_status.into(); }
                if current_status == "doing" {
                    for session in linked {
                        let snapshot = state.tasks.get(key).map(|record| record.snapshot.clone()).unwrap_or_else(|| TaskSnapshot {
                            key: key.clone(), repository: repository.into(), title: key.clone(), notes: String::new(), status: current_status.into(), extra: Map::new(),
                        });
                        incoming.entry(session.session.clone()).or_default().push(snapshot);
                    }
                }
            }
            for session in sessions {
                let candidates = incoming.get(&session.session).cloned().unwrap_or_default();
                let sole = if candidates.len() == 1 { Some(candidates[0].key.as_str()) } else { None };
                let previous: Vec<_> = state.tasks.values().flat_map(|record| record.assignments.iter())
                    .filter(|assignment| assignment.session == session.session && assignment.active).map(|assignment| assignment.task.clone()).collect();
                let matching_previous = previous.first().and_then(|key| state.tasks[key].assignments.iter().find(|assignment|
                    assignment.session == session.session && assignment.active && assignment.branch == session.branch && assignment.repository == session.repository));
                let same_assignment = previous.len() == 1 && matching_previous.is_some() && (sole == Some(previous[0].as_str()) || candidates.is_empty());
                if same_assignment {
                    if let Some(result) = &session.last_result {
                        let assignment = state.tasks.get_mut(&previous[0]).unwrap().assignments.iter_mut().find(|assignment| assignment.session == session.session && assignment.active).unwrap();
                        if assignment.baseline_result.as_deref() != Some(&result.id) && result.completed_at >= assignment.assigned_at
                            && result.branch == assignment.branch && result.repository == assignment.repository {
                            assignment.result = Some(result.clone());
                        }
                    }
                }
                for record in state.tasks.values_mut() { for assignment in &mut record.assignments {
                    if assignment.session == session.session && (sole != Some(assignment.task.as_str())
                        || assignment.branch != session.branch || assignment.repository != session.repository) { assignment.active = false; }
                } }
                for snapshot in &candidates { state.link_task(snapshot, session, now, sole.is_some()); }
            }
            for review in reviews {
                if !valid_repository(&review.full_repository) || review.number <= 0 || review.head_branch.is_empty()
                    || (!review.url.is_empty() && !valid_pr_url(&review.full_repository, review.number, &review.url)) { continue; }
                let key = format!("{}#{}", review.full_repository, review.number);
                let mut linked = false;
                for record in state.tasks.values_mut() { for assignment in &mut record.assignments {
                    if assignment.repository == review.full_repository && assignment.branch == review.head_branch {
                        assignment.prs.insert(key.clone());
                        linked = true;
                    }
                } }
                if linked {
                    let previous = state.prs.get(&key);
                    let last_seen = previous.filter(|pr| pr.head_oid == review.head_oid && pr.decision == review.review && pr.title == review.title)
                        .map(|pr| pr.last_seen).unwrap_or(now);
                    state.prs.insert(key, PullRequestMemory { repository: review.full_repository.clone(), number: review.number,
                        title: review.title.clone(), url: review.url.clone(), branch: review.head_branch.clone(), head_oid: review.head_oid.clone(), decision: review.review.clone(), last_seen,
                        extra: previous.map(|pr| pr.extra.clone()).unwrap_or_default() });
                }
            }
            Ok(())
        })
    }
}
