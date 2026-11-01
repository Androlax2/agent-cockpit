//! `cockpit mcp`: the instruction bundle as an MCP server on standard input and output
//! (#49), for harnesses that read MCP resources or call MCP tools. One JSON-RPC 2.0 message
//! per line. Every answer comes from the running daemon, like `cockpit docs`; a configured
//! server is no proof the model sees it (report 08), so each harness is verified on its own.

use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::process::ExitCode;

use cockpit_client::ClientError;
use cockpit_protocol::{Command, Reply};
use serde_json::{Value, json};

use crate::agent::{exchange, launch_token};
use crate::todo::socket;

/// Newest first; a client asking for another gets the newest.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const RESOURCE_PREFIX: &str = "cockpit://instructions/";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

pub(crate) fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    if let Some(argument) = arguments.first() {
        return Err(format!(
            "mcp takes no argument, not {}",
            argument.to_string_lossy()
        ));
    }
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                eprintln!("cockpit: cannot read the MCP client: {error}");
                return Ok(ExitCode::FAILURE);
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = answer(&line) {
            let mut stdout = std::io::stdout().lock();
            let written = writeln!(stdout, "{reply}").and_then(|()| stdout.flush());
            if written.is_err() {
                // The client closed its end: nothing is left to serve.
                return Ok(ExitCode::SUCCESS);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The reply to one message; `None` for a notification, which gets none.
fn answer(line: &str) -> Option<Value> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Some(error(Value::Null, PARSE_ERROR, "parse error"));
    };
    let id = message.get("id").cloned()?;
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(error(id, INVALID_REQUEST, "a request names its method"));
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "resources/list" => resources(),
        "resources/read" => read_resource(&params),
        "tools/list" => Ok(tools()),
        "tools/call" => call_tool(&params),
        _ => Err((METHOD_NOT_FOUND, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => error(id, code, &message),
    })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|known| Some(**known) == asked)
        .unwrap_or(&PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": {"resources": {}, "tools": {}},
        "serverInfo": {"name": "cockpit", "version": env!("CARGO_PKG_VERSION")},
        "instructions": "Cockpit's manual for agents. Read cockpit://instructions/overview first, \
                         or call cockpit_brief from a Cockpit session."
    })
}

type Answered = Result<Value, (i64, String)>;

fn resources() -> Answered {
    match daemon(Command::InstructionCatalog)? {
        Reply::InstructionCatalog { topics, .. } => Ok(json!({
            "resources": topics.iter().map(|topic| json!({
                "uri": format!("{RESOURCE_PREFIX}{}", topic.name),
                "name": topic.name,
                "description": topic.summary,
                "mimeType": "text/markdown",
            })).collect::<Vec<_>>()
        })),
        other => Err(unexpected(&other)),
    }
}

fn read_resource(params: &Value) -> Answered {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "resources/read needs a uri".to_owned()))?;
    let topic = uri
        .strip_prefix(RESOURCE_PREFIX)
        .ok_or_else(|| (INVALID_PARAMS, format!("no resource {uri}")))?;
    let text = instructions(Some(topic.to_owned())).map_err(|refused| (INVALID_PARAMS, refused))?;
    Ok(json!({"contents": [{"uri": uri, "mimeType": "text/markdown", "text": text}]}))
}

fn tools() -> Value {
    json!({"tools": [
        {
            "name": "cockpit_docs",
            "description": "Cockpit's manual for agents: one topic (overview, workflow, checks, \
                            approvals, coordination, sandbox), or all of them without a topic.",
            "inputSchema": {
                "type": "object",
                "properties": {"topic": {"type": "string", "description": "The topic to read."}},
            },
        },
        {
            "name": "cockpit_brief",
            "description": "This Cockpit session's launch brief: its rules, workspace, task step \
                            and grants. Only from a session Cockpit launched.",
            "inputSchema": {"type": "object", "properties": {}},
        },
    ]})
}

/// A tool's refusal is its result with `isError`, so the model reads why.
fn call_tool(params: &Value) -> Answered {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "tools/call needs a name".to_owned()))?;
    let outcome = match name {
        "cockpit_docs" => {
            let topic = params
                .pointer("/arguments/topic")
                .and_then(Value::as_str)
                .map(str::to_owned);
            instructions(topic)
        }
        "cockpit_brief" => brief(),
        _ => return Err((INVALID_PARAMS, format!("no tool {name}"))),
    };
    let (text, is_error) = match outcome {
        Ok(text) => (text, false),
        Err(refused) => (refused, true),
    };
    Ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
}

fn instructions(topic: Option<String>) -> Result<String, String> {
    match daemon(Command::InstructionsRead { topic }) {
        Ok(Reply::Instructions { text, .. }) => Ok(text),
        Ok(other) => Err(unexpected(&other).1),
        Err((_, refused)) => Err(refused),
    }
}

fn brief() -> Result<String, String> {
    match daemon(Command::AgentBrief {
        launch_token: launch_token(),
    }) {
        Ok(Reply::AgentBrief { brief, .. }) => Ok(brief),
        Ok(other) => Err(unexpected(&other).1),
        Err((_, refused)) => Err(refused),
    }
}

/// The daemon's reply, or the error to give the client: a refusal in the daemon's words.
fn daemon(command: Command) -> Result<Reply, (i64, String)> {
    let socket = socket(launch_token().is_some()).map_err(|_| {
        (
            INTERNAL_ERROR,
            "cannot place the daemon's socket".to_owned(),
        )
    })?;
    exchange(&socket, command).map_err(|error| match error {
        ClientError::Daemon(refused) => {
            let mut message = refused.message;
            if let Some(unblock) = refused.unblock {
                message.push_str(&format!("; {}", unblock.action));
            }
            (INVALID_PARAMS, message)
        }
        ClientError::Connect { source, .. } => (
            INTERNAL_ERROR,
            format!("cannot reach the daemon at {}: {source}", socket.display()),
        ),
        error => (INTERNAL_ERROR, error.to_string()),
    })
}

fn unexpected(reply: &Reply) -> (i64, String) {
    (
        INTERNAL_ERROR,
        format!("the daemon answered with an unexpected {reply:?}"),
    )
}
