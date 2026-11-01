//! A session's own OpenCode server: every launch starts one, on a loopback port of its own
//! and behind a password of its own (`OpenCode::launch`). Through it Cockpit discovers and
//! invokes OpenCode's native commands, delivers inbox messages without typing into the
//! terminal, picks a model variant for them, and aborts a turn. Paths and bodies follow
//! OpenCode 1.19.5's HTTP API (`/doc` on the server).

use std::fmt;
use std::time::Duration;

pub use cockpit_provider_api::{CommandResult as Invocation, NativeCommand};
use cockpit_provider_api::{ConversationId, LaunchCommand, ProviderError};

use crate::SERVER_USERNAME;
use crate::http::{self, Method, Request};

/// How long a request other than a command invocation may take.
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

/// The longest piece of an error answer kept in a [`ServerError`].
const ERROR_BODY_MAX_CHARS: usize = 200;

/// The server a launch started: its port and password.
#[derive(Clone, PartialEq, Eq)]
pub struct Server {
    port: u16,
    password: String,
}

impl fmt::Debug for Server {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Server")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// The server could not do what was asked; `reason` says why, for the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerError {
    pub reason: String,
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for ServerError {}

impl From<ServerError> for ProviderError {
    fn from(error: ServerError) -> Self {
        Self {
            reason: error.reason,
        }
    }
}

impl From<ProviderError> for ServerError {
    fn from(error: ProviderError) -> ServerError {
        ServerError {
            reason: error.reason,
        }
    }
}

impl Server {
    pub fn new(port: u16, password: String) -> Server {
        Server { port, password }
    }

    /// The server `launch` starts, read back from its words: its `--port` and its
    /// `OPENCODE_SERVER_PASSWORD`. `None` for a launch that is not OpenCode's.
    pub fn from_launch(launch: &LaunchCommand) -> Option<Server> {
        let hostname = launch
            .arguments
            .windows(2)
            .find(|words| words[0] == "--hostname")?;
        if hostname[1] != crate::SERVER_HOSTNAME {
            return None;
        }
        let port = launch
            .arguments
            .iter()
            .skip_while(|argument| *argument != "--port")
            .nth(1)?
            .to_str()?
            .parse()
            .ok()?;
        let password = launch
            .environment
            .iter()
            .find(|(name, _)| name == "OPENCODE_SERVER_PASSWORD")?
            .1
            .to_str()?
            .to_owned();
        let username = launch
            .environment
            .iter()
            .find(|(name, _)| name == "OPENCODE_SERVER_USERNAME")?;
        if port == 0 || password.is_empty() || username.1 != SERVER_USERNAME {
            return None;
        }
        Some(Server::new(port, password))
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The version the server reports (`GET /global/health`).
    pub fn health(&self) -> Result<String, ServerError> {
        let health = self.json(Method::Get, "/global/health", None, REQUEST_DEADLINE)?;
        let is_healthy = health.get("healthy").and_then(serde_json::Value::as_bool) == Some(true);
        match health.get("version").and_then(serde_json::Value::as_str) {
            Some(version) if is_healthy => Ok(version.to_owned()),
            _ => Err(unexpected("/global/health", &health)),
        }
    }

    /// The native commands OpenCode offers in this session's project (`GET /command`):
    /// its built-in ones, the project's and the owner's, MCP prompts and skills.
    pub fn commands(&self) -> Result<Vec<NativeCommand>, ServerError> {
        let listed = self.json(Method::Get, "/command", None, REQUEST_DEADLINE)?;
        let items = listed
            .as_array()
            .ok_or_else(|| unexpected("/command", &listed))?;
        items
            .iter()
            .map(|item| {
                let text = |field: &str| {
                    item.get(field)
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                };
                Ok(NativeCommand {
                    name: text("name").ok_or_else(|| unexpected("/command", item))?,
                    description: text("description"),
                    source: text("source"),
                })
            })
            .collect()
    }

    /// Runs the native command `command` with `arguments` in `session`, at `variant` when
    /// given (`POST /session/<id>/command`). OpenCode answers once the command's turn ends,
    /// so this blocks up to `deadline`; running it is not proof of what it did.
    pub fn invoke_command(
        &self,
        session: &ConversationId,
        command: &str,
        arguments: &str,
        variant: Option<&str>,
        deadline: Duration,
    ) -> Result<Invocation, ServerError> {
        if command.is_empty() || command.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(ServerError {
                reason: format!("{command:?} is not a command name"),
            });
        }
        let mut body = serde_json::json!({ "command": command, "arguments": arguments });
        with_variant(&mut body, variant)?;
        let path = format!("/session/{}/command", session_id(session)?);
        let answer = self.json(Method::Post, &path, Some(&body), deadline)?;
        let message = answer
            .pointer("/info/id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| unexpected(&path, &answer))?;
        Ok(Invocation {
            message: message.to_owned(),
        })
    }

    /// Delivers `text` to `session` as the owner's message, at `variant` when given
    /// (`POST /session/<id>/prompt_async`). OpenCode accepts it at once and queues it
    /// behind a turn under way; acceptance is not completion.
    pub fn deliver(
        &self,
        session: &ConversationId,
        text: &str,
        variant: Option<&str>,
    ) -> Result<(), ServerError> {
        if text.trim().is_empty() {
            return Err(ServerError {
                reason: "an empty message delivers nothing".to_owned(),
            });
        }
        let mut body = serde_json::json!({ "parts": [{ "type": "text", "text": text }] });
        with_variant(&mut body, variant)?;
        let path = format!("/session/{}/prompt_async", session_id(session)?);
        self.call(Method::Post, &path, Some(&body), REQUEST_DEADLINE)
            .map(drop)
    }

    /// Aborts the turn under way in `session` (`POST /session/<id>/abort`).
    pub fn abort(&self, session: &ConversationId) -> Result<(), ServerError> {
        let path = format!("/session/{}/abort", session_id(session)?);
        let answer = self.json(Method::Post, &path, None, REQUEST_DEADLINE)?;
        if answer == true {
            Ok(())
        } else {
            Err(unexpected(&path, &answer))
        }
    }

    /// The variants (provider-specific reasoning efforts) of `provider`'s `model`, sorted
    /// (`GET /config/providers`); none when the model has none.
    pub fn variants(&self, provider: &str, model: &str) -> Result<Vec<String>, ServerError> {
        let path = "/config/providers";
        let catalog = self.json(Method::Get, path, None, REQUEST_DEADLINE)?;
        let providers = catalog
            .get("providers")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| unexpected(path, &catalog))?;
        let found = providers
            .iter()
            .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(provider))
            .and_then(|entry| entry.get("models")?.get(model))
            .ok_or_else(|| ServerError {
                reason: format!("OpenCode has no model {provider}/{model}"),
            })?;
        let mut variants: Vec<String> = found
            .get("variants")
            .and_then(serde_json::Value::as_object)
            .map(|variants| variants.keys().cloned().collect())
            .unwrap_or_default();
        variants.sort();
        Ok(variants)
    }

    fn json(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
        deadline: Duration,
    ) -> Result<serde_json::Value, ServerError> {
        let text = self.call(method, path, body, deadline)?;
        serde_json::from_str(&text).map_err(|error| ServerError {
            reason: format!("OpenCode's answer to {path} is not JSON: {error}"),
        })
    }

    /// The body of a successful answer.
    fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
        deadline: Duration,
    ) -> Result<String, ServerError> {
        let body = body.map(serde_json::Value::to_string);
        let request = Request {
            method,
            path,
            body: body.as_deref(),
        };
        let response = http::send(
            self.port,
            SERVER_USERNAME,
            &self.password,
            &request,
            deadline,
        )
        .map_err(|reason| ServerError {
            reason: format!("OpenCode's server on port {}: {reason}", self.port),
        })?;
        match response.status {
            200..=299 => Ok(response.body),
            401 => Err(ServerError {
                reason: format!(
                    "OpenCode's server on port {} refused the session's password",
                    self.port
                ),
            }),
            status => Err(ServerError {
                reason: format!(
                    "OpenCode answered {path} with {status}: {}",
                    response
                        .body
                        .chars()
                        .take(ERROR_BODY_MAX_CHARS)
                        .collect::<String>()
                ),
            }),
        }
    }
}

/// Adds `variant` to a request body, refusing one OpenCode's schema refuses (`#`) or that
/// is not a plain word.
fn with_variant(body: &mut serde_json::Value, variant: Option<&str>) -> Result<(), ServerError> {
    let Some(variant) = variant else {
        return Ok(());
    };
    let is_plain = !variant.is_empty()
        && variant
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !is_plain {
        return Err(ServerError {
            reason: format!("{variant:?} is not a variant name"),
        });
    }
    body["variant"] = variant.into();
    Ok(())
}

fn unexpected(path: &str, value: &serde_json::Value) -> ServerError {
    let shown: String = value
        .to_string()
        .chars()
        .take(ERROR_BODY_MAX_CHARS)
        .collect();
    ServerError {
        reason: format!("OpenCode's answer to {path} is not what 1.19.5 sends: {shown}"),
    }
}

/// `id` when it has the form of an OpenCode session id, `ses` then ASCII letters, digits and
/// `_`, so it is safe in a URL path and on OpenCode's command line.
pub(crate) fn session_id(id: &ConversationId) -> Result<ConversationId, ProviderError> {
    let text = id.as_str();
    let is_session = text.starts_with("ses")
        && text
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    if !is_session {
        return Err(ProviderError {
            reason: format!(
                "{text:?} is not an OpenCode session id (`ses` then letters, digits and `_`)"
            ),
        });
    }
    Ok(id.clone())
}
