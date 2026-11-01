//! #450: the owner reaches the real Codex adapter through the sandbox's mapped socket.
#![cfg(target_os = "linux")]
mod common;
mod messaging_fixture;
use cockpit_codex::{Bridge, CONFORMANCE_VERSION, Codex};
use cockpit_daemon::Wiring;
use cockpit_protocol::{Command, ErrorCode};
use cockpit_sandbox::{Greywall, GreywallPrograms, HostPaths, Platform, Sandbox};
use common::{GENEROUS, refused};
use messaging_fixture::Home;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn script(home: &Home, name: &str, body: &str) -> PathBuf {
    let path = home.path().join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn wiring(home: &Home, mounts: bool) -> Wiring {
    let server = script(home, "app-server", APP_SERVER);
    let provider = Codex::new(
        "/unused-codex".into(),
        Some(CONFORMANCE_VERSION.into()),
        Some(Bridge {
            session_program: server,
            hook_program: "/unused-hook".into(),
            cockpit_program: "/unused-cli".into(),
        }),
    );
    let greywall = script(
        home,
        "greywall",
        &format!(
            r#"#!/bin/sh
if [ "$1" = --version ]; then
  printf 'greywall - fixture\n  Version: 0.3.7+dotfiles\n  Commit: {}\n'
  for capability in bubblewrap landlock seccomp 'network isolation' 'D-Bus isolated'; do
    printf '  ✓ %s\n' "$capability"
  done
  exit 0
fi
if [ "$1" = --help ]; then printf '%s\n' '{}'; exit 0; fi
while [ "$1" != -- ]; do
  if [ "$1" = --mount ]; then export FIXTURE_MOUNT="$2"; fi
  shift
done
shift
exec "$@"
"#,
            cockpit_sandbox::PINNED_GREYWALL_COMMIT,
            if mounts {
                "      --mount stringArray"
            } else {
                "no folder support"
            }
        ),
    );
    let greyproxy = script(
        home,
        "greyproxy",
        &format!(
            "#!/bin/sh\nprintf 'greyproxy 0.4.5 (fixture)\\n  commit: {}\\n'\n",
            cockpit_sandbox::PINNED_GREYPROXY_COMMIT
        ),
    );
    let engine = Greywall::detect(
        GreywallPrograms {
            greywall,
            greyproxy,
        },
        Platform::Linux,
        home.path().join("settings"),
    )
    .unwrap();
    assert_eq!(engine.can_mount_folders, mounts);
    let mut wiring = common::wiring_in(home.path(), vec![Arc::new(provider)]);
    wiring.sessions.owner.sandbox = cockpit_config::AgentSandbox::Required;
    wiring.sessions.sandbox = Sandbox::new(
        Platform::Linux,
        Ok(engine),
        HostPaths {
            read_only: Vec::new(),
            denied: Vec::new(),
            owner_uid: 1000,
            mount_sources: home.path().join("mounts"),
        },
    );
    wiring
}

async fn reported(home: &Home, id: &str) -> PathBuf {
    let dir = home.options().session_dirs().join(id);
    let deadline = Instant::now() + GENEROUS;
    let ready: Value = loop {
        if let Ok(text) = std::fs::read_to_string(dir.join("ready")) {
            break serde_json::from_str(&text).unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "the fake app-server never listened"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let database = home.database.lock().unwrap().clone().unwrap();
    let write = database.write().await.unwrap();
    cockpit_sessions::report(
        &write,
        &cockpit_agent_channel::ancestry(ready["pid"].as_u64().unwrap() as u32),
        ready["token"].as_str(),
        cockpit_sessions::Report {
            state: Some(cockpit_sessions::AgentState::Waiting),
            conversation: Some("thread-431".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    dir
}
fn requests(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("requests"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn owner_send_and_cancel_reach_the_mapped_codex_socket_after_daemon_restart() {
    let home = Home::new();
    let daemon = home.start_wired(wiring(&home, true)).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "codex").await;
    let dir = reported(&home, &session.id).await;
    assert!(
        std::fs::read_link(dir.join("codex.sock"))
            .unwrap()
            .starts_with("/tmp/codex-daemon-1000")
    );
    client
        .request(
            Command::SessionSend {
                session: session.id.clone(),
                message: "first".into(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    drop(client);
    let database = Arc::downgrade(home.database.lock().unwrap().as_ref().unwrap());
    drop(daemon);
    home.database.lock().unwrap().take();
    let deadline = Instant::now() + GENEROUS;
    while database.upgrade().is_some() {
        assert!(
            Instant::now() < deadline,
            "the old daemon kept its database"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The next daemon sees an engine without --mount. The running launch retains its mapping.
    let _daemon = home.start_wired(wiring(&home, false)).await;
    let client = home.connect().await;
    client
        .request(
            Command::SessionSend {
                session: session.id.clone(),
                message: "second".into(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    client
        .request(
            Command::SessionCancel {
                session: session.id.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    let actions: Vec<Value> = requests(&dir)
        .into_iter()
        .filter(|request| {
            matches!(
                request[0].as_str(),
                Some("turn/start" | "turn/steer" | "turn/interrupt")
            )
        })
        .collect();
    assert_eq!(
        actions,
        vec![
            json!(["turn/start", {"threadId":"thread-431", "input":[{"type":"text", "text":"first"}]}]),
            json!(["turn/steer", {"threadId":"thread-431", "expectedTurnId":"turn-1", "input":[{"type":"text", "text":"second"}]}]),
            json!(["turn/interrupt", {"threadId":"thread-431", "turnId":"turn-1"}]),
        ]
    );
}

#[tokio::test]
async fn owner_delivery_without_the_mount_feature_is_refused_without_connecting() {
    let home = Home::new();
    let _daemon = home.start_wired(wiring(&home, false)).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "codex").await;
    let dir = reported(&home, &session.id).await;
    for command in [
        Command::SessionSend {
            session: session.id.clone(),
            message: "hello".into(),
        },
        Command::SessionCancel {
            session: session.id,
        },
    ] {
        let error = refused(client.request(command, GENEROUS).await);
        assert_eq!(error.code, ErrorCode::Refused);
        assert!(error.message.contains("--mount"), "{error:?}");
    }
    assert!(requests(&dir).is_empty());
}

// A listener in the launched process tree, speaking the adapter's WebSocket/JSON-RPC wire.
// The fake engine exports its requested mapping; no real sandbox or host /tmp folder is used.
const APP_SERVER: &str = r#"#!/usr/bin/python3
import json, os, socket, struct, sys
from pathlib import Path
link = Path(sys.argv[sys.argv.index('--socket') + 1])
folder = link.parent
mapping = os.environ.get('FIXTURE_MOUNT')
if mapping:
    host, sandbox = mapping.split(':', 1)
    os.chdir(host)
    entry = 'a' * 64
    link.symlink_to(sandbox + '/' + entry)
else:
    os.chdir(folder)
    entry = 'codex.sock'
listener = socket.socket(socket.AF_UNIX)
listener.bind(entry)
listener.listen()
(folder / 'ready.partial').write_text(json.dumps({'pid': os.getpid(), 'token': os.environ['COCKPIT_LAUNCH_TOKEN']}))
(folder / 'ready.partial').rename(folder / 'ready')
active = False

def exact(conn, size):
    data = b''
    while len(data) < size:
        chunk = conn.recv(size - len(data))
        if not chunk:
            raise EOFError()
        data += chunk
    return data

while True:
    conn, _ = listener.accept()
    with conn:
        try:
            header = b''
            while not header.endswith(b'\r\n\r\n'):
                header += exact(conn, 1)
            conn.sendall(b'HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: fake\r\n\r\n')
            while True:
                opcode, flags = exact(conn, 2)
                if opcode & 15 == 8:
                    break
                length = flags & 127
                if length == 126:
                    length = struct.unpack('!H', exact(conn, 2))[0]
                elif length == 127:
                    length = struct.unpack('!Q', exact(conn, 8))[0]
                mask = exact(conn, 4)
                payload = exact(conn, length)
                request = json.loads(bytes(value ^ mask[i % 4] for i, value in enumerate(payload)))
                if 'id' not in request:
                    continue
                method, params = request['method'], request.get('params', {})
                with (folder / 'requests').open('a') as log:
                    log.write(json.dumps([method, params]) + '\n')
                if method == 'thread/read':
                    result = {'thread': {'status': {'type': 'active' if active else 'idle'}}}
                elif method == 'thread/turns/list':
                    result = {'data': [{'id': 'turn-1', 'status': 'inProgress'}]}
                else:
                    result = {}
                if method == 'turn/start':
                    active = True
                if method == 'turn/interrupt':
                    active = False
                body = json.dumps({'id': request['id'], 'result': result}).encode()
                size = bytes([len(body)]) if len(body) < 126 else b'\x7e' + struct.pack('!H', len(body))
                conn.sendall(b'\x81' + size + body)
        except (EOFError, BrokenPipeError, ConnectionResetError):
            pass
"#;

#[tokio::test]
async fn unsandboxed_codex_delivery_uses_the_direct_socket_and_old_context_is_refused() {
    let home = Home::new();
    let mut setup = wiring(&home, false);
    setup.sessions.owner.sandbox = cockpit_config::AgentSandbox::Off;
    let _daemon = home.start_wired(setup).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "codex").await;
    let dir = reported(&home, &session.id).await;
    assert!(
        !std::fs::symlink_metadata(dir.join("codex.sock"))
            .unwrap()
            .is_symlink()
    );
    client
        .request(
            Command::SessionSend {
                session: session.id.clone(),
                message: "direct".into(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    assert!(
        requests(&dir)
            .iter()
            .any(|request| request[0] == "turn/start")
    );
    // An agent launched before the migration has no delivery row. Never guess its mapping.
    let database = home.database.lock().unwrap().clone().unwrap();
    let write = database.write().await.unwrap();
    write
        .execute("DELETE FROM session_delivery", ())
        .await
        .unwrap();
    write.commit().await.unwrap();
    let before = requests(&dir);
    let error = refused(
        client
            .request(
                Command::SessionCancel {
                    session: session.id,
                },
                GENEROUS,
            )
            .await,
    );
    assert!(error.message.contains("restart the session"), "{error:?}");
    assert_eq!(requests(&dir), before);
}
