//! The project's `[[services]]` (#880): host services a sandboxed session of the project
//! reaches through a forward its launcher serves. A declaration only asks: the launcher
//! relays a connection once the owner allowed the service's whole record, and a changed
//! `listen` or `target` is another record the owner is asked about again. Only the project
//! file declares services; `cockpit-sandbox` validates each forward when a session launches.

use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

use crate::{ConfigError, Layer, Sourced};

/// One declared service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    /// What the owner's request and grant name it by, unique in the project.
    pub name: String,
    /// Where the agent connects, inside the sandbox.
    pub listen: ServiceEndpoint,
    /// What the launcher connects to, on the host.
    pub target: ServiceEndpoint,
}

/// `127.0.0.1:<port>` or `unix:<absolute path>`, as the project file writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceEndpoint {
    Port(u16),
    Socket(PathBuf),
}

impl fmt::Display for ServiceEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceEndpoint::Port(port) => write!(formatter, "127.0.0.1:{port}"),
            ServiceEndpoint::Socket(path) => write!(formatter, "unix:{}", path.display()),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServiceEntry {
    name: String,
    listen: String,
    target: String,
}

/// The most services one project declares: a launch serves at most this many forwards
/// (`cockpit_sandbox::MAX_FORWARDS`).
const MAX_SERVICES: usize = 32;

/// The session proxy's port inside the sandbox, which no service listens on.
const PROXY_PORT: u16 = 3128;

/// The project's `[[services]]`, in its order: one list, from the project file alone. The
/// rules the launch spec holds for every launch are checked here too, so a declaration no
/// launch could serve is refused when the file is read; where a socket may listen depends on
/// the launch's folders and is checked there.
pub(super) fn services(layers: &[Layer]) -> Result<Vec<Sourced<Service>>, ConfigError> {
    let mut services: Vec<Sourced<Service>> = Vec::new();
    for layer in layers {
        let Some(entries) = &layer.settings.services else {
            continue;
        };
        for entry in entries.get_ref() {
            let source = layer.source_at(entry.span());
            let declared = entry.get_ref();
            let invalid = |reason: String| ConfigError::Invalid {
                key: "services".to_string(),
                source: source.clone(),
                reason: format!("entry {reason}"),
            };
            if services.len() == MAX_SERVICES {
                return Err(invalid(format!(
                    "is one more than the {MAX_SERVICES} services a project declares"
                )));
            }
            if !is_service_name(&declared.name) {
                return Err(invalid(format!(
                    "has the name {:?}; write a lower-case letter, then letters, digits and \
                     `-`, at most 64 bytes",
                    declared.name
                )));
            }
            if declared.name == ENGINE_PORTS_SERVICE {
                return Err(invalid(format!(
                    "has the name {ENGINE_PORTS_SERVICE:?}, which the project engine's ports \
                     go by; name the service otherwise"
                )));
            }
            if services
                .iter()
                .any(|known| known.value.name == declared.name)
            {
                return Err(invalid(format!("repeats the name {:?}", declared.name)));
            }
            let endpoint = |field: &str, text: &str| {
                parse_endpoint(text).ok_or_else(|| {
                    invalid(format!(
                        "{:?} has the {field} {text:?}; write 127.0.0.1:<port> or \
                         unix:<absolute path>",
                        declared.name
                    ))
                })
            };
            let service = Service {
                name: declared.name.clone(),
                listen: endpoint("listen", &declared.listen)?,
                target: endpoint("target", &declared.target)?,
            };
            if let ServiceEndpoint::Port(port) = service.listen
                && (port < 1024 || port == PROXY_PORT)
            {
                return Err(invalid(format!(
                    "{:?} listens on port {port}; a service listens on a port from 1024 up, \
                     other than the session proxy's {PROXY_PORT}",
                    declared.name
                )));
            }
            if let Some(clash) = services
                .iter()
                .find(|known| known.value.listen == service.listen)
            {
                return Err(invalid(format!(
                    "{:?} listens at {}, where {:?} listens",
                    declared.name, service.listen, clash.value.name
                )));
            }
            services.push(Sourced::new(service, source));
        }
    }
    Ok(services)
}

/// `cockpit_sandbox::CONTAINERS_SERVICE`: the ports a project engine publishes are asked for
/// under it (#390), so no declared service takes it.
const ENGINE_PORTS_SERVICE: &str = "containers";

/// The launch spec's service name (`cockpit_sandbox::is_service_name`): it holds no space,
/// so the record a grant is bound to reads one way only.
fn is_service_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 64
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// A loopback port or an absolute socket path without spaces or control characters, as a
/// record holds it; anything else, a wildcard, a range or another address included, is None.
fn parse_endpoint(text: &str) -> Option<ServiceEndpoint> {
    if let Some(path) = text.strip_prefix("unix:") {
        if path
            .chars()
            .any(|character| character == ' ' || character.is_control())
        {
            return None;
        }
        let path = PathBuf::from(path);
        return path.is_absolute().then_some(ServiceEndpoint::Socket(path));
    }
    let port = text.strip_prefix("127.0.0.1:")?;
    // `u16::from_str` takes a leading `+`, which no port is written with.
    if !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    port.parse().ok().map(ServiceEndpoint::Port)
}
