//! #416's daemon handoff: the real adapter receives its original host-retained launch.
#![cfg(target_os = "linux")]
mod common;
mod messaging_fixture;

use cockpit_opencode::{Bridge, CONFORMANCE_VERSION, OpenCode};
use cockpit_protocol::Command;
use common::GENEROUS;
use messaging_fixture::Home;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn provider(program: PathBuf) -> Arc<OpenCode> {
    Arc::new(OpenCode::new(
        program,
        Some(CONFORMANCE_VERSION.into()),
        Some(Bridge {
            plugin: "/unused-plugin".into(),
            bridge_program: "/unused-bridge".into(),
            cockpit_program: "/unused-cli".into(),
        }),
    ))
}

#[tokio::test]
async fn owner_delivery_retains_opencodes_authenticated_endpoint_across_daemon_restart() {
    let home = Home::new();
    let program = home.path().join("opencode");
    std::fs::write(&program, SERVER).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let daemon = home.start(vec![provider(program)]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "opencode").await;
    let deadline = Instant::now() + GENEROUS;
    let ready: Value = loop {
        if let Ok(text) = std::fs::read_to_string(home.path().join("ready")) {
            break serde_json::from_str(&text).unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "the fixture server never listened"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    {
        let database = home.database.lock().unwrap().clone().unwrap();
        let write = database.write().await.unwrap();
        cockpit_sessions::report(
            &write,
            &cockpit_agent_channel::ancestry(ready["pid"].as_u64().unwrap() as u32),
            ready["token"].as_str(),
            cockpit_sessions::Report {
                state: Some(cockpit_sessions::AgentState::Waiting),
                conversation: Some("ses_431".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
    }
    let message = "/help @friend\n!literal";
    client
        .request(
            Command::SessionSend {
                session: session.id.clone(),
                message: message.into(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    drop(client);
    home.stop(daemon).await;

    // A fresh adapter would generate a different endpoint/password if asked to launch.
    // The running agent and its original endpoint must survive even this program change.
    let _daemon = home.start(vec![provider("/changed-opencode".into())]).await;
    let client = home.connect().await;
    for command in [
        Command::SessionSend {
            session: session.id.clone(),
            message: "after restart".into(),
        },
        Command::SessionCancel {
            session: session.id,
        },
    ] {
        client.request(command, GENEROUS).await.unwrap();
    }
    let calls: Vec<Value> = std::fs::read_to_string(home.path().join("requests"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        calls,
        vec![
            json!(["/session/ses_431/prompt_async", {"parts":[{"type":"text","text":message}]}]),
            json!(["/session/ses_431/prompt_async", {"parts":[{"type":"text","text":"after restart"}]}]),
            json!(["/session/ses_431/abort", null]),
        ]
    );
}

// Isolated server in the launched agent's process tree. It accepts only the launch's
// original password; neither credentials nor endpoint are written to the ready file.
const SERVER: &str = r#"#!/usr/bin/python3
import base64, json, os, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
folder = Path(__file__).parent
auth = 'Basic ' + base64.b64encode(('opencode:' + os.environ['OPENCODE_SERVER_PASSWORD']).encode()).decode()
class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def do_POST(self):
        if self.headers.get('Authorization') != auth:
            self.send_response(401)
            self.end_headers()
            return
        raw = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        with (folder / 'requests').open('a') as log:
            log.write(json.dumps([self.path, json.loads(raw) if raw else None]) + '\n')
        body = b'true' if self.path.endswith('/abort') else b''
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
server = HTTPServer(('127.0.0.1', int(sys.argv[sys.argv.index('--port') + 1])), Handler)
(folder / 'ready.partial').write_text(json.dumps({'pid': os.getpid(), 'token': os.environ['COCKPIT_LAUNCH_TOKEN']}))
(folder / 'ready.partial').rename(folder / 'ready')
server.serve_forever()
"#;
