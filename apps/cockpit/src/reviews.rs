//! Pull requests and issues from GitHub through gh: the viewed session's repository, or every
//! running session's, or the review requests when no session runs in one. Fetched in a thread.

use crate::paths;
use crate::util::{self, current_session, now, parse_time};
use chrono::{Duration, Utc};
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

const REFRESH_S: f64 = 300.0;
/// A turn's end refetches, but never sooner than this after the last fetch: sessions ending turns
/// one after another would otherwise call gh for each.
const TURN_END_REFETCH_S: f64 = 30.0;
const LIMIT: usize = 30;
const RECENT_DAYS: i64 = 180; // older pull requests are dead repositories, not work
const ISSUES_PER_REPOSITORY: &str = "20";
const LIST_FIELDS: &str = "number,title,url,updatedAt,author,isDraft,reviewDecision,statusCheckRollup,headRefName,headRefOid";
const ISSUE_FIELDS: &str = "number,title,url,updatedAt,labels";
/// Bot bumps are merged or closed without a review; they only bury real work.
const BOT_AUTHORS: [&str; 6] = ["dependabot", "renovate", "app/dependabot", "app/renovate", "dependabot[bot]", "renovate[bot]"];
const CHECK_FAILED: [&str; 6] = ["FAILURE", "ERROR", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED", "STARTUP_FAILURE"];
const CHECK_PENDING: [&str; 6] = ["PENDING", "QUEUED", "IN_PROGRESS", "WAITING", "REQUESTED", "EXPECTED"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Review {
    pub title: String,
    pub url: String,
    pub number: i64,
    pub repository: String,
    pub full_repository: String,
    pub author: String,
    pub updated: String,
    pub draft: bool,
    pub review: String,
    pub ci: String,
    pub head_branch: String,
    pub head_oid: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Issue {
    pub number: i64,
    pub title: String,
    pub url: String,
    pub repository: String,
    pub full_repository: String,
    pub updated: String,
    pub labels: Vec<String>,
}

impl Issue {
    pub fn key(&self) -> String {
        format!("{}#{}", self.full_repository, self.number)
    }
}

fn text(row: &Value, key: &str) -> String {
    row.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// failing, pending, passing, or empty when the pull request has no checks.
pub fn ci_state(checks: Option<&Value>) -> String {
    let Some(checks) = checks.and_then(Value::as_array).filter(|checks| !checks.is_empty()) else { return String::new() };
    let states: Vec<String> = checks.iter().filter(|check| check.is_object()).map(|check| {
        ["conclusion", "state", "status"].iter()
            .filter_map(|key| check.get(*key).and_then(Value::as_str))
            .find(|value| !value.is_empty()).unwrap_or("").to_uppercase()
    }).collect();
    if states.iter().any(|state| CHECK_FAILED.contains(&state.as_str())) {
        "failing".into()
    } else if states.iter().any(|state| CHECK_PENDING.contains(&state.as_str()) || state.is_empty()) {
        "pending".into()
    } else {
        "passing".into()
    }
}

/// Rows from gh search prs (repository in each row) or gh pr list (repository given).
pub fn parse_reviews(raw: &str, repository: &str) -> Vec<Review> {
    let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(raw) else { return Vec::new() };
    rows.iter().map(|row| {
        let full = if !repository.is_empty() {
            repository.to_string()
        } else {
            let found = row.get("repository");
            found.and_then(|r| r.get("nameWithOwner")).or_else(|| found.and_then(|r| r.get("name")))
                .and_then(Value::as_str).unwrap_or("").to_string()
        };
        Review {
            title: text(row, "title"), url: text(row, "url"), number: row.get("number").and_then(Value::as_i64).unwrap_or(0),
            repository: full.rsplit('/').next().unwrap_or("").to_lowercase(), full_repository: full.to_lowercase(),
            head_branch: text(row, "headRefName"), head_oid: text(row, "headRefOid"),
            author: row.get("author").and_then(|author| author.get("login")).and_then(Value::as_str).unwrap_or("").into(),
            updated: text(row, "updatedAt"), draft: row.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
            review: text(row, "reviewDecision"), ci: ci_state(row.get("statusCheckRollup")),
        }
    }).collect()
}

pub fn parse_issues(raw: &str, repository: &str) -> Vec<Issue> {
    let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(raw) else { return Vec::new() };
    rows.iter().filter(|row| row.is_object()).map(|row| Issue {
        number: row.get("number").and_then(Value::as_i64).unwrap_or(0), title: text(row, "title"), url: text(row, "url"),
        repository: repository.rsplit('/').next().unwrap_or("").to_string(), full_repository: repository.to_string(),
        updated: text(row, "updatedAt"),
        labels: row.get("labels").and_then(Value::as_array).into_iter().flatten()
            .filter_map(|label| label.get("name").and_then(Value::as_str).map(str::to_string)).collect(),
    }).collect()
}

fn since() -> String {
    (Utc::now() - Duration::days(RECENT_DAYS)).format("%Y-%m-%d").to_string()
}

/// gh's output, or the reason it failed (its last error line).
fn gh(arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("gh").args(arguments).stdin(Stdio::null()).output().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound { "gh is not installed".to_string() } else { error.to_string() }
    })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr.trim().lines().last().map(str::to_string).unwrap_or_else(|| format!("exit {}", output.status.code().unwrap_or(-1))))
}

/// The pull requests, or an empty list and the reason (gh missing, not logged in, offline).
pub fn fetch_reviews(repositories: &[String]) -> (Vec<Review>, String) {
    if let Some(fixture) = paths::reviews_fixture() {
        return (parse_reviews(&std::fs::read_to_string(fixture).unwrap_or_default(), ""), String::new());
    }
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    if repositories.is_empty() {
        let updated = format!("--updated=>={}", since());
        let limit = LIMIT.to_string();
        match gh(&["search", "prs", "--state=open", "--sort=updated", "--limit", &limit, "--json", "url,title,repository,updatedAt,author,number",
                   "--review-requested=@me", &updated, "--", "-author:app/dependabot", "-author:app/renovate"]) {
            Ok(raw) => rows.extend(parse_reviews(&raw, "")),
            Err(reason) if reason == "gh is not installed" => return (Vec::new(), reason),
            Err(reason) => errors.push(format!("gh: {reason}")),
        }
        // Search's JSON omits head metadata; enrich only its requested PRs in this fetch thread.
        // A PR whose details fail keeps its search row without branch or revision (so it links
        // to no task): failing the whole list would make the phone refetch it on every request.
        for row in &mut rows {
            match gh(&["pr", "view", &row.number.to_string(), "--repo", &row.full_repository, "--json", LIST_FIELDS]) {
                Ok(raw) => {
                    if let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(&raw) {
                        if let Some(detail) = parse_reviews(&serde_json::json!([value]).to_string(), &row.full_repository).into_iter().find(|detail| detail.number == row.number) { *row = detail; }
                    } else { util::log_event("WARN", &format!("pull request {}#{} metadata response is not an object", row.full_repository, row.number)); }
                }
                Err(reason) => util::log_event("WARN", &format!("pull request {}#{} metadata: {reason}", row.full_repository, row.number)),
            }
        }
    }
    for repository in repositories {
        match gh(&["pr", "list", "--repo", repository, "--state", "open", "--limit", "20", "--json", LIST_FIELDS]) {
            Ok(raw) => rows.extend(parse_reviews(&raw, repository)),
            Err(reason) if reason == "gh is not installed" => return (Vec::new(), reason),
            Err(reason) => errors.push(format!("{repository}: {reason}")),
        }
    }
    let since = since();
    rows.retain(|row| !BOT_AUTHORS.contains(&row.author.as_str()) && row.updated.get(..10).unwrap_or("") >= since.as_str());
    rows.sort_by(|a, b| b.updated.cmp(&a.updated));
    rows.truncate(LIMIT);
    (rows, errors.join("; "))
}

/// Open issues of the sessions' repositories (none when no session runs in one).
pub fn fetch_issues(repositories: &[String]) -> (Vec<Issue>, String) {
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    for repository in repositories {
        match gh(&["issue", "list", "--repo", repository, "--state", "open", "--limit", ISSUES_PER_REPOSITORY, "--json", ISSUE_FIELDS]) {
            Ok(raw) => rows.extend(parse_issues(&raw, repository)),
            Err(reason) => errors.push(format!("{repository}: {reason}")),
        }
    }
    rows.sort_by(|a, b| b.updated.cmp(&a.updated));
    (rows, errors.join("; "))
}

pub fn review_age(updated: &str) -> String {
    let Some(moment) = parse_time(updated) else { return String::new() };
    let seconds = (Utc::now() - moment).num_seconds();
    if seconds < 3600 {
        format!("{}m", seconds.max(0) / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    }
}

/// Each directory's origin URL (None: no origin). `git remote` per session on every snapshot
/// change slowed the column's refresh; a remote rarely changes, and r (refresh) forgets them.
static REMOTES: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

fn origin_of(directory: &str) -> Option<String> {
    let mut cache = REMOTES.lock().unwrap();
    cache.get_or_insert_with(HashMap::new).entry(directory.to_string())
        .or_insert_with(|| util::output_of("git", &["-C", directory, "remote", "get-url", "origin"]).map(|url| url.trim().to_string()))
        .clone()
}

/// owner/repo of the directory's github.com origin.
fn github_repository(directory: &str) -> Option<String> {
    static GITHUB_REMOTE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(?:https://github\.com/|git@github\.com:)([\w.-]+/[\w.-]+?)(?:\.git)?$").unwrap());
    let url = origin_of(directory)?;
    GITHUB_REMOTE.captures(&url).map(|found| found[1].to_string())
}

fn snapshot_sessions() -> Vec<Value> {
    util::read_json(&paths::snapshot()).and_then(|snapshot| snapshot.get("sessions").and_then(Value::as_array).cloned()).unwrap_or_default()
}

/// Each session working now -> its repository: a pull request opened during its turn is on
/// GitHub once the turn ends.
fn working_sessions() -> HashMap<String, String> {
    snapshot_sessions().iter()
        .filter(|s| s.get("state").and_then(Value::as_str) == Some("working"))
        .filter_map(|s| Some((s.get("session")?.as_str()?.to_string(), github_repository(s.get("cwd")?.as_str()?)?)))
        .collect()
}

/// owner/repo -> local directory, for every running session whose directory has a github.com
/// origin, or for the one session given (the one in the session pane) when it is known.
pub fn session_checkouts(only_session: &str) -> BTreeMap<String, String> {
    let sessions = snapshot_sessions();
    let viewed: Vec<_> = sessions.iter().filter(|s| !only_session.is_empty() && s.get("session").and_then(Value::as_str) == Some(only_session)).cloned().collect();
    let chosen = if viewed.is_empty() { sessions } else { viewed };
    let mut directories: Vec<String> = chosen.iter().filter_map(|s| s.get("cwd").and_then(Value::as_str)).filter(|cwd| !cwd.is_empty()).map(str::to_string).collect();
    directories.sort();
    directories.dedup();
    let mut checkouts = BTreeMap::new();
    for directory in directories {
        if let Some(repository) = github_repository(&directory) {
            checkouts.entry(repository).or_insert(directory);
        }
    }
    checkouts
}

#[derive(Clone, Default)]
struct Fetched {
    rows: Vec<Review>,
    issues: Vec<Issue>,
    error: String,
    fetched_at: f64,
}

/// What GitHub said for each set of repositories the panel has shown, so a switch to another
/// session shows that session's pull requests (or nothing while they load), never the previous ones.
#[derive(Default)]
struct State {
    by_scope: HashMap<Vec<String>, Fetched>,
    loading: HashSet<Vec<String>>,
}

/// Pull requests fetched in a thread, so a slow GitHub never freezes the column.
#[derive(Default)]
pub struct Reviews {
    state: Arc<Mutex<State>>,
    pub repositories: Vec<String>,
    pub checkouts: BTreeMap<String, String>,
    scope_key: Option<(Option<std::time::SystemTime>, String)>,
    working: HashMap<String, String>,
    turn_ended: bool,
}

impl Reviews {
    /// Whether a fetch started (false: one is already under way for these repositories).
    pub fn refresh(&self) -> bool {
        let scope = self.repositories.clone();
        if !self.state.lock().unwrap().loading.insert(scope.clone()) {
            return false;
        }
        let shared = self.state.clone();
        std::thread::spawn(move || {
            let (rows, error) = fetch_reviews(&scope);
            let (issues, issue_error) = if paths::reviews_fixture().is_some() { (Vec::new(), String::new()) } else { fetch_issues(&scope) };
            let mut state = shared.lock().unwrap();
            let previous_error = state.by_scope.get(&scope).map(|fetched| fetched.error.clone()).unwrap_or_default();
            if !error.is_empty() && error != previous_error {
                util::log_event("WARN", &format!("pull requests: {error}"));
            }
            if !issue_error.is_empty() {
                util::log_event("WARN", &format!("issues: {issue_error}"));
            }
            state.loading.remove(&scope);
            state.by_scope.insert(scope, Fetched { rows, issues, error, fetched_at: now() });
        });
        true
    }

    /// Re-reads the repositories when the snapshot or the viewed session changes; refetches when
    /// they were never fetched or have aged, or when a session in them ended a turn (the pull
    /// request it opened shows then, not minutes later). In a session, only its repository;
    /// otherwise every running one.
    pub fn tick(&mut self) {
        let viewed = current_session();
        let mtime = std::fs::metadata(paths::snapshot()).and_then(|m| m.modified()).ok();
        let key = (mtime, viewed.clone());
        if self.scope_key.as_ref() != Some(&key) {
            self.scope_key = Some(key);
            self.checkouts = session_checkouts("");
            self.repositories = session_checkouts(&viewed).into_keys().collect();
            let working = working_sessions();
            self.turn_ended |= self.working.iter()
                .any(|(session, repository)| !working.contains_key(session) && self.repositories.contains(repository));
            self.working = working;
        }
        let fetched_at = self.state.lock().unwrap().by_scope.get(&self.repositories).map_or(0.0, |fetched| fetched.fetched_at);
        let age = now() - fetched_at;
        let is_due = age > REFRESH_S || (self.turn_ended && age > TURN_END_REFETCH_S);
        if is_due && self.refresh() {
            self.turn_ended = false;
        }
    }

    /// The shown repositories' pull requests, the fetch error, and whether a fetch is under way.
    pub fn current(&self) -> (Vec<Review>, String, bool) {
        let state = self.state.lock().unwrap();
        let fetched = state.by_scope.get(&self.repositories).cloned().unwrap_or_default();
        (fetched.rows, fetched.error, state.loading.contains(&self.repositories))
    }

    pub fn issues(&self) -> Vec<Issue> {
        self.state.lock().unwrap().by_scope.get(&self.repositories).map(|fetched| fetched.issues.clone()).unwrap_or_default()
    }

    pub fn cached(&self) -> Vec<Review> {
        let state = self.state.lock().unwrap();
        let mut fetched: Vec<_> = state.by_scope.values().collect();
        fetched.sort_by(|left, right| left.fetched_at.total_cmp(&right.fetched_at));
        let mut rows = BTreeMap::new();
        for fetched in fetched { for row in &fetched.rows { rows.insert((row.full_repository.clone(), row.number), row.clone()); } }
        rows.into_values().collect()
    }

    pub fn availability(&self, repository: &str) -> Result<(), String> {
        let state = self.state.lock().unwrap();
        let latest = state.by_scope.iter().filter(|(scope, _)| scope.iter().any(|repo| repo.eq_ignore_ascii_case(repository)))
            .map(|(_, fetched)| fetched).max_by(|left, right| left.fetched_at.total_cmp(&right.fetched_at));
        let Some(fetched) = latest else { return Err("PR data unavailable (repository not loaded)".into()) };
        if !fetched.error.is_empty() { return Err(format!("PR data unavailable: {}", fetched.error)); }
        if now() - fetched.fetched_at > REFRESH_S { return Err("PR data stale; showing last known links".into()); }
        Ok(())
    }

    #[cfg(test)]
    pub fn age_fetch(&self, seconds: f64) {
        if let Some(fetched) = self.state.lock().unwrap().by_scope.get_mut(&self.repositories) {
            fetched.fetched_at -= seconds;
        }
    }

    #[cfg(test)]
    pub fn set_rows(&self, rows: Vec<Review>) {
        self.state.lock().unwrap().by_scope.entry(self.repositories.clone()).or_default().rows = rows;
    }

    #[cfg(test)]
    pub fn set_issues(&self, issues: Vec<Issue>) {
        self.state.lock().unwrap().by_scope.entry(self.repositories.clone()).or_default().issues = issues;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ci_is_failing_pending_or_passing() {
        assert_eq!(ci_state(Some(&json!([{"conclusion": "SUCCESS"}, {"conclusion": "FAILURE"}]))), "failing");
        assert_eq!(ci_state(Some(&json!([{"status": "IN_PROGRESS"}]))), "pending");
        assert_eq!(ci_state(Some(&json!([{"conclusion": "SUCCESS"}]))), "passing");
        assert_eq!(ci_state(Some(&json!([]))), "");
    }

    #[test]
    fn a_listed_pull_request_takes_the_repository_it_was_listed_for() {
        let raw = r#"[{"number": 61, "title": "VAT", "author": {"login": "sam"}, "updatedAt": "2026-10-18T10:00:00Z", "isDraft": true, "reviewDecision": "REVIEW_REQUIRED"}]"#;
        let rows = parse_reviews(raw, "acme/shop");
        assert_eq!((rows[0].repository.as_str(), rows[0].full_repository.as_str(), rows[0].draft), ("shop", "acme/shop", true));
    }
}
