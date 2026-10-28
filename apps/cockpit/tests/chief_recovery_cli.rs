use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::unix::{net::UnixListener, process::CommandExt};
use std::sync::{Arc, Mutex};
mod common;
use common::sandbox::{Sandbox, stderr, stdout};

const SESSION: &str = "ses_chief000000000000000000000";

fn snapshot() -> Value {
    let now = chrono::Utc::now().timestamp_millis();
    json!({"projection":"host_review","cursor_kind":"snapshot_offset","generation":"generation-1","has_more":false,
    "records":[
        {"kind":"chief","session":{"session_id":SESSION,"agent":"opencode","project":"path:/chief","runtime":"boot-a:10:100"},"epoch":"old-epoch","revision":3,"state":"paused","live_verified":false},
        {"kind":"worker","session":{"session_id":SESSION,"agent":"opencode","project":"path:/chief","runtime":"boot-b:20:200"},"observed_at_ms":now},
        {"kind":"admission","session":{"session_id":SESSION,"agent":"opencode","project":"path:/chief","runtime":"boot-b:20:200"},"adapter":"opencode-queued-v1","verified_at_ms":now}
    ]})
}

fn host(sandbox: &Sandbox, response: Value) -> Arc<Mutex<Vec<Value>>> {
    let directory = sandbox.runtime.join("claude-team-host");
    std::fs::create_dir_all(&directory).unwrap();
    let listener = UnixListener::bind(directory.join("host.sock")).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut input = String::new();
            stream.read_to_string(&mut input).unwrap();
            let request: Value = serde_json::from_str(&input).unwrap();
            recorded.lock().unwrap().push(request);
            stream
                .write_all(
                    json!({"request_id":"fixture","result":{"Ok":response}})
                        .to_string()
                        .as_bytes(),
                )
                .unwrap();
        }
    });
    requests
}

#[test]
fn recovery_review_emits_exact_host_request_without_submitting_or_verifying() {
    let sandbox = Sandbox::new();
    let requests = host(&sandbox, snapshot());
    let output = sandbox.run(
        env!("CARGO_BIN_EXE_cockpit-team"),
        &["recovery-review", SESSION],
        &[],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let result: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(result["request"], "chief");
    assert_eq!(result["action"]["action"], "recover");
    assert_eq!(result["action"]["session"]["session_id"], SESSION);
    assert_eq!(result["action"]["session"]["runtime"], "boot-b:20:200");
    assert_eq!(result["action"]["expected_epoch"], "old-epoch");
    assert_eq!(result["action"]["expected_revision"], 3);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["request"], "chief_snapshot");
}

#[test]
fn recovery_review_refuses_ambiguous_stale_or_retargeted_identity() {
    for change in [
        "duplicate-chief",
        "duplicate-worker",
        "missing-worker",
        "different-project",
        "same-runtime",
        "stale-worker",
        "missing-admission",
        "future-admission",
        "wrong-adapter",
        "malformed",
    ] {
        let sandbox = Sandbox::new();
        let mut snapshot = snapshot();
        match change {
            "duplicate-chief" => {
                let row = snapshot["records"][0].clone();
                snapshot["records"].as_array_mut().unwrap().push(row);
            }
            "duplicate-worker" => {
                let row = snapshot["records"][1].clone();
                snapshot["records"].as_array_mut().unwrap().push(row);
            }
            "missing-worker" => {
                snapshot["records"].as_array_mut().unwrap().remove(1);
            }
            "different-project" => {
                snapshot["records"][1]["session"]["project"] = json!("path:/other")
            }
            "same-runtime" => snapshot["records"][1]["session"]["runtime"] = json!("boot-a:10:100"),
            "stale-worker" => snapshot["records"][1]["observed_at_ms"] = json!(1),
            "missing-admission" => {
                snapshot["records"].as_array_mut().unwrap().pop();
            }
            "future-admission" => {
                snapshot["records"][2]["verified_at_ms"] =
                    json!(chrono::Utc::now().timestamp_millis() + 60_000)
            }
            "wrong-adapter" => snapshot["records"][2]["adapter"] = json!("other"),
            "malformed" => snapshot["records"][0]["revision"] = Value::Null,
            _ => unreachable!(),
        }
        let requests = host(&sandbox, snapshot);
        let output = sandbox.run(
            env!("CARGO_BIN_EXE_cockpit-team"),
            &["recovery-review", SESSION],
            &[],
        );
        assert!(!output.status.success(), "{change}: {}", stdout(&output));
        assert!(
            output.stdout.is_empty(),
            "{change}: no partial recovery request"
        );
        assert!(
            requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request["request"] == "chief_snapshot")
        );
    }
}

#[test]
fn recovery_review_never_falls_back_to_an_agent_socket() {
    let sandbox = Sandbox::new();
    let requests = sandbox.team_socket(Value::Null);
    let output = sandbox.run(
        env!("CARGO_BIN_EXE_cockpit-team"),
        &["recovery-review", SESSION],
        &[],
    );
    assert!(!output.status.success());
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn recovery_submission_requires_a_separate_host_terminal_confirmation() {
    let sandbox = Sandbox::new();
    let requests = host(&sandbox, json!({"state":"active"}));
    let input=json!({"request":"chief","action":{"action":"recover","request_id":"recover","expected_epoch":"old-epoch","expected_revision":3,"session":snapshot()["records"][1]["session"]}}).to_string();
    let mut command = sandbox.command(env!("CARGO_BIN_EXE_cockpit-team"), &[]);
    command
        .arg("chief-host")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Isolate the test from the test runner's controlling terminal: a piped request is not consent.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        !output.status.success(),
        "piped recovery must not bypass confirmation"
    );
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn declining_a_recovery_confirmation_does_not_submit_the_request() {
    let sandbox = Sandbox::new();
    let requests = host(&sandbox, json!({"state":"active"}));
    let input = sandbox.root.join("recovery.json");
    std::fs::write(&input,json!({"request":"chief","action":{"action":"recover","request_id":"recover","expected_epoch":"old-epoch","expected_revision":3,"session":snapshot()["records"][1]["session"]}}).to_string()).unwrap();
    let script = r#"
import errno, os, pty, select, signal, sys, time
pid, master = pty.fork()
if pid == 0:
    source = os.open(sys.argv[2], os.O_RDONLY)
    os.dup2(source, 0)
    os.close(source)
    os.execv(sys.argv[1], [sys.argv[1], 'chief-host'])
transcript = b''
deadline = time.monotonic() + 10
answered = False
while time.monotonic() < deadline:
    if not select.select([master], [], [], .05)[0]:
        continue
    try:
        chunk = os.read(master, 65536)
    except OSError as error:
        if error.errno == errno.EIO:
            break
        raise
    if not chunk:
        break
    transcript += chunk
    if b'Confirm exactly this Chief recovery?' in transcript and not answered:
        os.write(master, b'n\n')
        answered = True
else:
    os.kill(pid, signal.SIGKILL)
_, status = os.waitpid(pid, 0)
os.close(master)
sys.stdout.buffer.write(transcript)
assert answered, transcript
assert os.waitstatus_to_exitcode(status) != 0, transcript
"#;
    let output = sandbox
        .command("python3", &[])
        .args([
            "-c",
            script,
            env!("CARGO_BIN_EXE_cockpit-team"),
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        stdout(&output),
        stderr(&output)
    );
    assert!(stdout(&output).contains("host declined confirmation"));
    assert!(requests.lock().unwrap().is_empty());
}
