//! Provider status pages: the overall indicator, each product not
//! operational, the open incidents. Fetched in a thread every 2 minutes; an empty provider status URL
//! turns it off, and a file:// address is read from disk (tests, the demo cockpit).

use crate::paths;
use crate::util::{self, now};
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub const PAGE: &str = "https://status.claude.com/";
pub const CODEX_PAGE: &str = "https://status.openai.com/";
const POLL_S: f64 = 120.0;

#[derive(Clone, Copy, Default)]
pub enum Provider {
    #[default]
    Claude,
    Codex,
}

impl Provider {
    pub fn url(self) -> String {
        match self { Self::Claude => paths::claude_status_url(), Self::Codex => paths::codex_status_url() }
    }

    pub fn host(self) -> &'static str {
        match self { Self::Claude => "status.claude.com", Self::Codex => "status.openai.com" }
    }

    pub fn label(self) -> &'static str {
        match self { Self::Claude => "Claude Status", Self::Codex => "Codex Status" }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub indicator: String,
    pub description: String,
    pub degraded: Vec<(String, String)>,
    pub incidents: Vec<String>,
}

pub fn parse(summary: &Value) -> Summary {
    let text = |value: Option<&Value>| value.and_then(Value::as_str).unwrap_or("").to_string();
    let status = summary.get("status");
    let components = summary.get("components").and_then(Value::as_array).cloned().unwrap_or_default();
    let incidents = summary.get("incidents").and_then(Value::as_array).cloned().unwrap_or_default();
    Summary {
        indicator: text(status.and_then(|s| s.get("indicator"))),
        description: text(status.and_then(|s| s.get("description"))),
        degraded: components.iter()
            .filter(|c| !c.get("group").and_then(Value::as_bool).unwrap_or(false))
            .filter(|c| c.get("status").and_then(Value::as_str) != Some("operational"))
            .map(|c| (c.get("name").and_then(Value::as_str).unwrap_or("?").to_string(),
                      c.get("status").and_then(Value::as_str).unwrap_or("?").replace('_', " ")))
            .collect(),
        incidents: incidents.iter()
            .filter(|i| i.get("status").and_then(Value::as_str) != Some("resolved"))
            .map(|i| i.get("name").and_then(Value::as_str).unwrap_or("?").to_string())
            .collect(),
    }
}

#[derive(Default)]
struct State {
    current: Option<Summary>,
    error: String,
    fetched_at: f64,
    fetching: bool,
}

#[derive(Clone, Default)]
pub struct ProviderStatus {
    pub provider: Provider,
    state: Arc<Mutex<State>>,
}

fn fetch(url: &str) -> Result<Value, String> {
    let body = if let Some(path) = url.strip_prefix("file://") {
        std::fs::read_to_string(path).map_err(|error| error.kind().to_string())?
    } else {
        let mut response = ureq::get(url).header("User-Agent", "cockpit").call().map_err(|error| error.to_string())?;
        response.body_mut().read_to_string().map_err(|error| error.to_string())?
    };
    serde_json::from_str(&body).map_err(|_| "not JSON".to_string())
}

impl ProviderStatus {
    pub fn new(provider: Provider) -> Self {
        Self { provider, ..Self::default() }
    }

    pub fn tick(&self) {
        let url = self.provider.url();
        let provider = self.provider;
        {
            let mut state = self.state.lock().unwrap();
            if url.is_empty() || state.fetching || now() - state.fetched_at < POLL_S {
                return;
            }
            state.fetching = true;
        }
        let shared = self.state.clone();
        std::thread::spawn(move || {
            let (current, error) = match fetch(&url) {
                Ok(summary) => (Some(parse(&summary)), String::new()),
                Err(reason) => (None, format!("{} unreachable ({reason})", provider.host())),
            };
            let previous = {
                let mut state = shared.lock().unwrap();
                let previous = state.current.clone();
                if current.is_some() {
                    state.current = current.clone();
                }
                if error != state.error {
                    if error.is_empty() { util::log_event("INFO", &format!("{} reachable again", provider.host())) } else { util::log_event("WARN", &error) }
                }
                state.error = error;
                state.fetched_at = now();
                state.fetching = false;
                previous
            };
            if let Some(current) = current {
                if previous.map(|p| p.indicator) != Some(current.indicator.clone()) {
                    // Provider outages are not cockpit bugs for the error hand-off.
                    let level = if current.indicator == "none" { "INFO" } else { "WARN" };
                    util::log_event(level, &format!("{}: {}", provider.label(), current.description));
                }
            }
        });
    }

    pub fn snapshot(&self) -> (Option<Summary>, String) {
        let state = self.state.lock().unwrap();
        (state.current.clone(), state.error.clone())
    }

    /// Fetches right now, in this thread (tests).
    #[cfg(test)]
    pub fn fetch_now(&self) {
        self.tick();
        for _ in 0..200 {
            if !self.state.lock().unwrap().fetching {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_summary_keeps_products_not_operational_and_open_incidents() {
        let summary = json!({"status": {"indicator": "minor", "description": "Partially Degraded Service"},
            "components": [{"name": "Claude API", "status": "operational"}, {"name": "Claude Code", "status": "partial_outage"},
                           {"name": "A group", "status": "major_outage", "group": true}],
            "incidents": [{"name": "Elevated errors", "status": "investigating"}, {"name": "Old", "status": "resolved"}]});
        let parsed = parse(&summary);
        assert_eq!(parsed.indicator, "minor");
        assert_eq!(parsed.degraded, vec![("Claude Code".to_string(), "partial outage".to_string())]);
        assert_eq!(parsed.incidents, vec!["Elevated errors".to_string()]);
    }
}
