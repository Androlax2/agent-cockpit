#!/usr/bin/env python3
"""Native 1.19.5 fixture; also serves as its recording bridge and fake Cockpit command."""

import base64
import collections
import datetime
import hashlib
import json
import os
import pwd
import re
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

LIMIT = 2 * 1024 * 1024
BRIEF = "COCKPIT_FIXTURE_BRIEF: isolated provider conformance, no account access."


def append(path, value):
    path = Path(path)
    encoded = json.dumps(value, separators=(",", ":")) + "\n"
    assert len(encoded) <= LIMIT
    assert not path.exists() or path.stat().st_size + len(encoded) <= LIMIT
    with path.open("a") as output:
        output.write(encoded)


def records(path):
    if not path.exists():
        return []
    assert path.stat().st_size <= LIMIT
    # The writer may be between its write and newline; wait for a complete record.
    return [
        json.loads(line)
        for line in path.read_text().splitlines(keepends=True)
        if line.endswith("\n")
    ]


def bridge():
    payload = sys.stdin.buffer.read(LIMIT + 1)
    assert len(payload) <= LIMIT
    event = json.loads(payload)
    append(os.environ["FIXTURE_EVENTS"], event)
    result = subprocess.run(
        [os.environ["FIXTURE_BRIDGE"], *sys.argv[1:]],
        input=payload,
        capture_output=True,
        timeout=8,
        check=False,
    )
    assert result.returncode == 0, result.stderr.decode(errors="replace")
    assert result.stdout == b"", "the bridge must never answer a permission request"


def cockpit():
    if sys.argv[1:] == ["brief"]:
        print(BRIEF)
    elif sys.argv[1:] == ["mcp"]:
        for line in sys.stdin.buffer:
            assert len(line) <= LIMIT
            request = json.loads(line)
            if "id" not in request:
                continue
            method = request["method"]
            result = {
                "initialize": {
                    "protocolVersion": request.get("params", {}).get("protocolVersion"),
                    "capabilities": {},
                    "serverInfo": {"name": "cockpit-fixture", "version": "0"},
                },
                "tools/list": {"tools": []},
                "prompts/list": {"prompts": []},
                "resources/list": {"resources": []},
            }.get(method, {})
            print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
    else:
        assert sys.argv[1:3] == ["agent", "report"]
        append(os.environ["FIXTURE_REPORTS"], sys.argv[1:])


class Model(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, work):
        super().__init__(("127.0.0.1", 0), ModelRequest)
        self.work = work
        self.requests = []
        self.failures = []
        self.aborting = threading.Event()
        self.stop = threading.Event()
        self.worker = threading.Thread(target=self.serve_forever, daemon=True)
        self.worker.start()

    @property
    def url(self):
        return f"http://127.0.0.1:{self.server_port}/v1"

    def close(self):
        self.stop.set()
        self.shutdown()
        self.server_close()
        self.worker.join(timeout=2)


class ModelRequest(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        try:
            self.connection.settimeout(5)
            assert self.path == "/v1/chat/completions", self.path
            assert self.headers.get("Authorization") == "Bearer fixture-key"
            size = int(self.headers["Content-Length"])
            assert 0 < size <= LIMIT
            request = json.loads(self.rfile.read(size))
            self.server.requests.append(request)
            assert len(self.server.requests) <= 12, "unexpected repeated model requests"
            assert request["model"] == "fixture-model"
            messages = request["messages"]
            user = next(message for message in reversed(messages) if message["role"] == "user")
            marker = json.dumps(user["content"])
            tools = {tool["function"]["name"] for tool in request.get("tools", [])}
            if tools:
                assert BRIEF in json.dumps(messages), "native plugin did not deliver the brief"
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            if tools and "COCKPIT_ABORT_FIXTURE" in marker:
                self.chunk({"role": "assistant"})
                self.server.aborting.set()
                assert self.server.stop.wait(30), "held fixture turn was not aborted"
                return
            if not tools or messages[-1]["role"] == "tool":
                self.chunk({"role": "assistant", "content": "Fixture complete"})
                self.chunk({}, "stop")
            else:
                if "COCKPIT_TOOL_FIXTURE" in marker:
                    name = "read"
                    arguments = {"filePath": str(self.server.work / "fixture.txt")}
                else:
                    assert "COCKPIT_PERMISSION_FIXTURE" in marker, marker
                    name = "bash"
                    arguments = {
                        "command": "/usr/bin/printf cockpit-permission-fixture",
                        "description": "Harmless conformance request; owner will reject it",
                    }
                assert name in tools, tools
                self.chunk(
                    {
                        "role": "assistant",
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_fixture",
                                "type": "function",
                                "function": {"name": name, "arguments": json.dumps(arguments)},
                            }
                        ],
                    }
                )
                self.chunk({}, "tool_calls")
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except Exception as error:
            self.server.failures.append(repr(error))
            self.close_connection = True

    def chunk(self, delta, finish=None):
        chunk = {
            "id": "chatcmpl_fixture",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "fixture-model",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        }
        self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
        self.wfile.flush()


def wait_for(description, condition, model, deadline=20):
    until = time.monotonic() + deadline
    while time.monotonic() < until:
        assert not model.failures, model.failures
        if result := condition():
            return result
        time.sleep(0.05)
    raise AssertionError(f"timed out waiting for {description}")


def capture(destination, events, reports, receipt, root):
    """Redact volatile fixture data, preserving native fields and relationships."""
    identifiers = {}
    start = events[0]["properties"]["info"]["time"]["created"]

    def identifier(match):
        original = match[0]
        if original not in identifiers:
            identifiers[original] = original[:4] + f"{len(identifiers) + 1:0{len(original) - 4}x}"
        return identifiers[original]

    def redact(value, key=""):
        if isinstance(value, dict):
            return {k: redact(v, k) for k, v in value.items()}
        if isinstance(value, list):
            return [redact(v) for v in value]
        if isinstance(value, str):
            if key == "slug":
                return "fixture-session"
            value = value.replace(str(root), "/fixture").replace(str(root).lstrip("/"), "fixture")
            return re.sub(r"\b(?:evt|ses|msg|prt|per)_[A-Za-z0-9]+", identifier, value)
        if isinstance(value, int) and not isinstance(value, bool) and value >= start:
            return value - start + 1
        return value

    destination.mkdir(parents=True, exist_ok=False)
    for name, values in [("events", events), ("reports", reports)]:
        raw = "".join(json.dumps(v, separators=(",", ":")) + "\n" for v in values)
        receipt[f"raw_{name}_sha256"] = hashlib.sha256(raw.encode()).hexdigest()
        (destination / f"{name}.jsonl").write_text(
            "".join(json.dumps(redact(v), separators=(",", ":")) + "\n" for v in values)
        )
    receipt["runtime"]["serverUrl"] = "http://127.0.0.1:<launch-port>/"
    receipt["redactions"] = [
        "fixture root -> /fixture; relative root -> fixture",
        "IDs -> stable equal-length IDs preserving references; slug -> fixture-session",
        "timestamps -> milliseconds from first session.created + 1; launch port -> <launch-port>",
    ]
    (destination / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")


def run(config):
    launch = json.loads(config.read_text())
    root = config.parent
    work = root / "work"
    home = root / "home"
    home.mkdir()
    # 1.19.5 still reads macOS MDM preferences independently of HOME and managed-dir.
    if sys.platform == "darwin":
        managed = Path("/Library/Managed Preferences")
        user = pwd.getpwuid(os.getuid()).pw_name
        assert not any(
            path.exists()
            for path in [
                managed / "ai.opencode.managed.plist",
                managed / user / "ai.opencode.managed.plist",
            ]
        ), "native fixture refuses host-managed OpenCode preferences"
    config_dir = home / "config" / "opencode"
    config_dir.mkdir(parents=True)
    (config_dir / ".gitignore").write_text("node_modules\n")
    (work / "fixture.txt").write_text("Only isolated fixture data.\n")
    events = root / "events.jsonl"
    reports = root / "reports.jsonl"
    runtime = root / "runtime.json"
    observer = root / "runtime-observer.mjs"
    observer.write_text(
        'import {writeFileSync} from "node:fs";\n'
        "export default async ({serverUrl}) => {writeFileSync(process.env.FIXTURE_RUNTIME, "
        "JSON.stringify({bun:Bun.version,serverUrl:String(serverUrl)}));return {}};\n"
    )
    model = Model(work)
    environment = {
        "HOME": str(home),
        "PATH": f"{Path(sys.executable).parent}:/usr/bin:/bin",
        **{
            f"XDG_{name}_HOME": str(home / name.lower())
            for name in ["CONFIG", "DATA", "CACHE", "STATE"]
        },
        "OPENCODE_DISABLE_PROJECT_CONFIG": "1",
        "OPENCODE_TEST_MANAGED_CONFIG_DIR": str(home / "managed"),
        "OPENCODE_DISABLE_MODELS_FETCH": "1",
        "OPENCODE_DISABLE_AUTOUPDATE": "1",
        "OPENCODE_EXPERIMENTAL_DISABLE_FILEWATCHER": "true",
        "OPENCODE_DISABLE_FFF": "1",
        "OPENCODE_DISABLE_AUTOCOMPACT": "1",
        "FIXTURE_EVENTS": str(events),
        "FIXTURE_REPORTS": str(reports),
        "FIXTURE_BRIDGE": launch["bridge"],
        "FIXTURE_RUNTIME": str(runtime),
        **launch["environment"],
    }
    inline = json.loads(environment["OPENCODE_CONFIG_CONTENT"])
    assert len(inline["plugin"]) == 1
    inline["plugin"].append(str(observer))
    inline.update(
        {
            "model": "fixture/fixture-model",
            "small_model": "fixture/fixture-model",
            "enabled_providers": ["fixture"],
            "share": "disabled",
            "snapshot": False,
            "lsp": False,
            "formatter": False,
            "provider": {
                "fixture": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "Isolated fixture",
                    "options": {"baseURL": model.url, "apiKey": "fixture-key"},
                    "models": {
                        "fixture-model": {
                            "name": "Fixture",
                            "tool_call": True,
                            "limit": {"context": 65536, "output": 4096},
                        }
                    },
                }
            },
            "permission": {
                "*": "deny",
                # 1.19.5 asks with a worktree-relative path; a non-Git fixture uses /.
                "read": {os.path.relpath(work / "fixture.txt", "/"): "allow"},
                "bash": "ask",
            },
        }
    )
    environment["OPENCODE_CONFIG_CONTENT"] = json.dumps(inline)
    port = environment["TORLEK_OPENCODE_PORT"]
    credentials = "opencode:" + environment["OPENCODE_SERVER_PASSWORD"]
    headers = {
        "Authorization": "Basic " + base64.b64encode(credentials.encode()).decode(),
        "Content-Type": "application/json",
    }

    def api(method, path, data=None):
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}{path}",
            data=None if data is None else json.dumps(data).encode(),
            headers=headers,
            method=method,
        )
        with urllib.request.urlopen(request, timeout=5) as response:
            body = response.read(LIMIT + 1)
        assert len(body) <= LIMIT
        return json.loads(body) if body else None

    log = collections.deque(maxlen=16)
    process = None
    try:
        config_dir.chmod(0o555)
        assert not os.access(config_dir, os.W_OK), (
            "cannot prevent background dependency installation"
        )
        # Rust checked this in a separate empty home with bounded output and descendants.
        version = launch["observed_version"]
        assert version == "1.19.5", version
        process = subprocess.Popen(
            [launch["program"], "serve", *launch["arguments"]],
            env=environment,
            cwd=work,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )

        def collect_log():
            while chunk := process.stdout.read1(4096):
                log.append(chunk)

        reader = threading.Thread(target=collect_log, daemon=True)
        reader.start()

        def ready():
            try:
                return api("GET", "/global/health")
            except (urllib.error.URLError, TimeoutError):
                return None

        assert wait_for("native server", ready, model)["version"] == version
        session = api("POST", "/session", {"title": "Cockpit conformance fixture"})["id"]
        wait_for(
            "native plugin session event",
            lambda: any(
                e["type"] == "session.created" and e["properties"]["info"]["id"] == session
                for e in records(events)
            ),
            model,
        )
        measured_runtime = json.loads(runtime.read_text())
        assert measured_runtime["bun"] and f":{port}" in measured_runtime["serverUrl"]

        def prompt(text):
            api(
                "POST",
                f"/session/{session}/prompt_async",
                {"parts": [{"type": "text", "text": text}]},
            )

        def event_since(offset, event_type, check):
            return next(
                (
                    e
                    for e in records(events)[offset:]
                    if e["type"] == event_type and check(e["properties"])
                ),
                None,
            )

        prompt("COCKPIT_TOOL_FIXTURE")
        wait_for(
            "native completed read",
            lambda: event_since(
                0,
                "message.part.updated",
                lambda p: (
                    p["part"]["tool"] == "read" and p["part"]["state"]["status"] == "completed"
                ),
            ),
            model,
        )
        wait_for(
            "completed turn",
            lambda: event_since(0, "message.updated", lambda p: p["info"].get("finish") == "stop"),
            model,
        )
        wait_for(
            "completed turn idle",
            lambda: event_since(0, "session.status", lambda p: p["status"]["type"] == "idle"),
            model,
        )

        offset = len(records(events))
        prompt("COCKPIT_PERMISSION_FIXTURE")
        permission = wait_for(
            "native permission request",
            lambda: event_since(offset, "permission.asked", lambda p: p["permission"] == "bash"),
            model,
        )
        assert permission["properties"]["sessionID"] == session
        request_id = permission["properties"]["id"]
        # An unauthenticated peer cannot answer even this harmless permission request.
        unauthorized = urllib.request.Request(
            f"http://127.0.0.1:{port}/permission/{request_id}/reply",
            data=b'{"reply":"reject"}',
            headers={"Content-Type": "application/json"},
        )
        try:
            urllib.request.urlopen(unauthorized, timeout=5).close()
        except urllib.error.HTTPError as error:
            assert error.code == 401, error.code
        else:
            raise AssertionError("permission reply accepted without the launch password")
        assert api("POST", f"/permission/{request_id}/reply", {"reply": "reject"}) is True
        wait_for(
            "refused command event",
            lambda: event_since(
                offset,
                "message.part.updated",
                lambda p: p["part"]["tool"] == "bash" and p["part"]["state"]["status"] == "error",
            ),
            model,
        )
        wait_for(
            "refused turn idle",
            lambda: event_since(offset, "session.status", lambda p: p["status"]["type"] == "idle"),
            model,
        )

        offset = len(records(events))
        prompt("COCKPIT_ABORT_FIXTURE")
        wait_for("held local model turn", model.aborting.is_set, model)
        assert api("POST", f"/session/{session}/abort") is True
        aborted = wait_for(
            "native aborted message",
            lambda: event_since(
                offset,
                "message.updated",
                lambda p: p["info"].get("error", {}).get("name") == "MessageAbortedError",
            ),
            model,
        )
        after_abort = records(events).index(aborted) + 1
        wait_for(
            "idle after aborted message",
            lambda: event_since(
                after_abort, "session.status", lambda p: p["status"]["type"] == "idle"
            ),
            model,
        )
        wait_for(
            "all normalized reports", lambda: len(records(reports)) == len(records(events)), model
        )
        captured_events, captured_reports = records(events), records(reports)
        assert len(captured_events) == len(captured_reports)
        assert all(report[-2:] == ["--conversation", session] for report in captured_reports)
        assert captured_reports[after_abort - 1] == [
            "agent",
            "report",
            "--state",
            "waiting",
            "--conversation",
            session,
        ], "aborting must not report a completed or failed outcome"
        permission_index = captured_events.index(permission)
        assert captured_reports[permission_index][2:6] == [
            "--state",
            "waiting",
            "--dialog",
            "OpenCode asks permission to bash /usr/bin/printf cockpit-permission-fixture",
        ]
        assert not any("--outcome" in report for report in captured_reports[permission_index:])
        assert not list(home.rglob("node_modules")), "unexpected dependency installation"
        assert not list(home.rglob("package.json")), "unexpected dependency manifest"
        assert not model.failures, model.failures
        receipt = {
            "captured_at": datetime.datetime.now(datetime.UTC).isoformat(),
            "version": version,
            "runtime": measured_runtime,
            "events": len(captured_events),
            "model_requests": len(model.requests),
            "permission_decision": "reject",
            "unauthenticated_permission_reply": 401,
            "account_usage": "none; local fixture model",
            "plugin_sha256": hashlib.sha256(Path(inline["plugin"][0]).read_bytes()).hexdigest(),
            "fixture_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        }
        with Path(launch["program"]).open("rb") as binary:
            receipt["binary_sha256"] = hashlib.file_digest(binary, "sha256").hexdigest()
        if destination := os.environ.get("COCKPIT_OPENCODE_CAPTURE_DIR"):
            capture(Path(destination), captured_events, captured_reports, receipt, root)
        print(json.dumps(receipt))
    except Exception:
        print(b"".join(log).decode(errors="replace"), file=sys.stderr)
        print(f"Recorded fixture events: {records(events)}", file=sys.stderr)
        raise
    finally:
        if process is not None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)
        # Rust's bounded runner stops the whole process group, including plugin children.
        model.close()
        config_dir.chmod(0o755)


if __name__ == "__main__":
    if Path(sys.argv[0]).name == "record-bridge":
        bridge()
    elif Path(sys.argv[0]).name == "cockpit-fixture":
        cockpit()
    else:
        assert sys.argv[1] == "run"
        run(Path(sys.argv[2]))
