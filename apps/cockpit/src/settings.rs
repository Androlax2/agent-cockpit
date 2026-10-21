//! Theo's on/off choices, kept by `claude-sessions setting` in settings.json. A setting missing
//! from the file is on: what the setup did before it had settings.

use crate::paths;
use crate::util::read_json;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::process::Command;

/// Every setting the S menu shows: its name in settings.json and claude-sessions (KNOWN_SETTINGS
/// there), and what it does.
pub const SETTINGS: [(&str, &str); 7] = [
    ("phone-on-lock", "Phone on lock"),
    ("todo-queue", "Todo queue (Q)"),
    ("error-handoff", "Errors to dotfiles"),
    ("freeze-while-gaming", "Freeze while gaming"),
    ("idle-lock", "Lock workspace 6 idle"),
    ("daily-digest", "Daily digest"),
    // One per todo::LINKS entry; off, what crosses the link waits for Theo's approval.
    ("todo-link-greywall-greyproxy", "Todos greywall↔greyproxy"),
];

pub const LAUNCH_SETTINGS: [(&str, &str); 3] = [
    ("default-agent", "Default agent"),
    ("opencode-model", "OpenCode model"),
    ("opencode-effort", "OpenCode effort"),
];

pub fn launch_value(name: &str) -> Result<String, String> {
    let fallback = if name == "default-agent" { "claude" } else { "" };
    let content = match fs::read_to_string(paths::settings()) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(fallback.into()),
        Err(error) => return Err(format!("cannot read settings: {error}")),
    };
    let settings: Value = serde_json::from_str(&content).map_err(|error| format!("invalid settings: {error}"))?;
    let settings = settings.as_object().ok_or("invalid settings: expected an object")?;
    match settings.get(name) {
        None => Ok(fallback.into()),
        Some(value) => value.as_str().map(str::to_string).ok_or_else(|| format!("invalid {name}: expected a string")),
    }
}

pub fn launch_agent() -> Result<String, String> {
    let name = launch_value("default-agent")?;
    crate::agents::find(&name).ok_or_else(|| format!("invalid default-agent: {name}"))?;
    Ok(name)
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelChoice {
    #[serde(rename = "providerID")]
    pub provider: String,
    pub id: String,
    pub enabled: bool,
    pub variants: Vec<ModelVariant>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ModelVariant {
    pub id: String,
}

impl ModelChoice {
    pub fn reference(&self) -> String {
        format!("{}/{}", self.provider, self.id)
    }
}

pub fn load_models(directory: &str) -> Result<Vec<ModelChoice>, String> {
    let output = Command::new("claude-sessions").args(["opencode-models", directory]).output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{}: {}", output.status, error.trim()));
    }
    #[derive(Deserialize)]
    struct Catalog { data: Vec<ModelChoice> }
    let catalog: Catalog = serde_json::from_slice(&output.stdout).map_err(|error| format!("invalid catalog: {error}"))?;
    let mut models = Vec::new();
    for model in catalog.data.into_iter().filter(|model| model.enabled) {
        let invalid = |id: &str| id.is_empty() || id.chars().any(|character| character.is_whitespace() || character.is_control() || character == '#');
        if invalid(&model.provider) || model.provider.contains('/') || invalid(&model.id)
            || model.variants.iter().any(|variant| invalid(&variant.id) || variant.id.contains('/')) {
            return Err("invalid catalog: malformed model or variant id".into());
        }
        models.push(model);
    }
    if models.is_empty() { return Err("no enabled models in this project".into()); }
    Ok(models)
}

pub fn is_on(name: &str) -> bool {
    read_json(&paths::settings()).and_then(|settings| settings.get(name)?.as_bool()).unwrap_or(true)
}
