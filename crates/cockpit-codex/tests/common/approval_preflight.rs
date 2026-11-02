//! Read-only checks before a probe creates a thread or requests a model turn.

use std::path::Path;

use cockpit_codex::app_server::AppServer;
use serde_json::{Value, json};

pub fn check(
    server: &mut AppServer,
    home: &Path,
    work: &Path,
    fixture_url: Option<&str>,
) -> Result<(), &'static str> {
    let response = server
        .call("config/read", json!({"includeLayers":true,"cwd":work}))
        .map_err(|_| "cannot check signed-in configuration; no turn started")?;
    let config = response["config"]
        .as_object()
        .ok_or("malformed signed-in configuration; no turn started")?;
    if let Some(servers) = config.get("mcp_servers").filter(|v| !v.is_null())
        && !servers
            .as_object()
            .is_some_and(|servers| servers.values().all(|s| s["enabled"] == false))
    {
        return Err("signed-in probe refuses enabled MCP servers; no turn started");
    }
    if config.get("notify") != Some(&json!([]))
        || config.get("approval_policy") != Some(&json!("on-request"))
        || config.get("approvals_reviewer") != Some(&json!("user"))
        || config.get("sandbox_mode") != Some(&json!("read-only"))
    {
        return Err("signed-in probe overrides are not effective; no turn started");
    }
    if config
        .get("tui")
        .is_some_and(|tui| !tui["keymap"].is_null())
    {
        return Err("custom TUI keymaps need a separate conformance plan; no turn started");
    }

    // 0.166.0 loads *.rules from each layer's config folder. Refuse all inherited
    // rule files, including allow rules that could bypass this test's approval.
    no_rules(home)?;
    let layers = response["layers"]
        .as_array()
        .filter(|layers| !layers.is_empty())
        .ok_or("cannot enumerate execution policy layers; no turn started")?;
    for layer in layers {
        let source = &layer["name"];
        // config/read can retain a project layer as disabled even when CLI trust was
        // supplied. Refuse nonempty layers instead of assuming thread/TUI loading agrees.
        if source["type"] == "project"
            && !layer["config"]
                .as_object()
                .is_some_and(|config| config.is_empty())
        {
            return Err("project configuration needs a separate conformance plan; no turn started");
        }
        let folder = match source["type"].as_str() {
            Some("packagedDefaults" | "system" | "user") => Path::new(
                source["file"]
                    .as_str()
                    .ok_or("missing config path; no turn started")?,
            )
            .parent()
            .ok_or("invalid config path; no turn started")?,
            Some("project") => Path::new(
                source["dotCodexFolder"]
                    .as_str()
                    .ok_or("missing project policy path; no turn started")?,
            ),
            Some("sessionFlags") => continue,
            _ => return Err("unsupported managed configuration layer; no turn started"),
        };
        if !folder.is_absolute() {
            return Err("non-absolute policy path; no turn started");
        }
        no_rules(folder)?;
    }
    // Managed rules in this version can only tighten policy (prompt/forbidden).
    // Other managed requirements could change provider routing; refuse those too.
    let requirements = server
        .call("configRequirements/read", json!({}))
        .map_err(|_| "cannot check managed requirements; no turn started")?;
    if requirements.get("requirements") != Some(&Value::Null) {
        return Err("managed requirements need a separate conformance plan; no turn started");
    }

    if let Some(url) = fixture_url {
        if config.get("model_provider") != Some(&json!("fixture"))
            || config.get("model") != Some(&json!("mock-model"))
            || response["config"]["model_providers"]["fixture"]["base_url"] != url
        {
            return Err("isolated probe did not select its local fixture model; no turn started");
        }
        return Ok(());
    }

    if config
        .get("model_provider")
        .is_some_and(|v| !v.is_null() && v != "openai")
        || [
            ("openai_base_url", "https://api.openai.com/v1"),
            ("chatgpt_base_url", "https://chatgpt.com/backend-api"),
        ]
        .iter()
        .any(|(key, official)| {
            config.get(*key).is_some_and(|v| {
                !v.is_null()
                    && v.as_str()
                        .is_none_or(|url| url.trim_end_matches('/') != *official)
            })
        })
        || config
            .get("model_providers")
            .is_some_and(|v| !v["openai"].is_null())
    {
        return Err("custom provider or endpoint is not signed-in conformance; no turn started");
    }
    let account = server
        .call("account/read", json!({"refreshToken":false}))
        .map_err(|_| "cannot check signed-in account; no turn started")?;
    if account["requiresOpenaiAuth"] != true || !account["account"].is_object() {
        return Err("a signed-in OpenAI account is required; no turn started");
    }
    Ok(())
}

fn no_rules(folder: &Path) -> Result<(), &'static str> {
    let entries = match std::fs::read_dir(folder.join("rules")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("cannot inspect execution policy; no turn started"),
    };
    for entry in entries {
        let path = entry
            .map_err(|_| "cannot inspect execution policy; no turn started")?
            .path();
        if path
            .extension()
            .is_some_and(|extension| extension == "rules")
        {
            return Err("signed-in probe refuses inherited execution rules; no turn started");
        }
    }
    Ok(())
}
