"""Isolated native instruction-delivery fixture; no account access or real model calls."""

import datetime
import hashlib
import json
import os
import signal
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

LIMIT = 2 * 1024 * 1024
SKILL_DESCRIPTION = "How to work inside a Cockpit session"
# Provider discovery kills its wrapper at ten seconds. Leave time for group cleanup.
VERSION_TIMEOUT = 3


def write_json(path, value):
    partial = path.with_suffix(".partial")
    partial.write_text(json.dumps(value))
    partial.replace(path)


def refuse_existing(paths):
    for path in paths:
        try:
            Path(path).lstat()
        except FileNotFoundError:
            continue
        raise AssertionError(f"external configuration exists: {path}; no native launch")


def preflight(root, provider):
    assert sys.platform == "linux", "native fixture currently validated on Linux only"
    assert root.is_absolute() and root.resolve() == root
    system = {
        "claude": ["/etc/claude-code", "/home/claude/.claude/remote"],
        "codex": ["/etc/codex"],
        "opencode": ["/etc/opencode"],
    }
    refuse_existing(system[provider])
    for parent in root.parents:
        refuse_existing(
            parent / name
            for name in (
                ".codex",
                ".claude",
                ".agents",
                ".opencode",
                "AGENTS.md",
                "AGENTS.override.md",
                "CLAUDE.md",
                ".mcp.json",
                "opencode.json",
                "opencode.jsonc",
            )
        )


def prepare(config, url):
    root = Path(config["root"])
    preflight(root, config["provider"])
    for name in ["tmp", "data", "cache", "claude", ".codex", "managed", "probe-processes"]:
        (root / name).mkdir(exist_ok=True)
    if config["provider"] == "opencode":
        folder = root / "config/opencode"
        folder.mkdir(parents=True)
        (folder / ".gitignore").write_text("node_modules\n")
        # Exact 1.19.5 npm.ts checks writability before its background plugin install.
        folder.chmod(0o555)
        assert not os.access(folder, os.W_OK), "cannot prevent dependency installation"
    if config["provider"] == "codex":
        (root / ".codex/config.toml").write_text(f'''
model = "mock-model"
model_provider = "fixture"
approval_policy = "on-request"
approvals_reviewer = "user"
sandbox_mode = "read-only"
cli_auth_credentials_store = "file"
mcp_oauth_credentials_store = "file"
check_for_update_on_startup = false
notify = []
[analytics]
enabled = false
[feedback]
enabled = false
[otel]
exporter = "none"
trace_exporter = "none"
metrics_exporter = "none"
[features]
plugins = false
apps = false
memories = false
shell_snapshot = false
enable_request_compression = false
[model_providers.fixture]
name = "Isolated instruction fixture"
base_url = "{url}/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
request_max_retries = 0
stream_max_retries = 0
[projects.{json.dumps(str(root / "projects/native-instructions"))}]
trust_level = "trusted"
''')
    initialize_project(root)


def initialize_project(root):
    """Real Git, without the caller's hooks, signers, templates or configuration."""
    project = root / "projects/native-instructions"
    project.mkdir(parents=True)
    empty = root / "empty-git-config"
    empty.mkdir()
    env = {
        "HOME": str(root),
        "XDG_CONFIG_HOME": str(root / "config"),
        "PATH": "/usr/bin:/bin",
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0",
    }
    git = [
        "git",
        "-c",
        f"core.hooksPath={empty}",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
    ]
    (project / "README").write_text("fixture\n")
    with (root / "git.log").open("w+") as output:
        for args in [
            ["init", "--quiet", "--initial-branch=main", f"--template={empty}"],
            ["add", "README"],
            ["commit", "--quiet", "-m", "initial"],
        ]:
            assert run_bounded(git + args, env, 3, output, cwd=project) == 0, "fixture Git failed"


def environment(config, url):
    root = Path(config["root"])
    # Never inherit provider credentials, proxies, helpers or an owner's config.
    preserve = {
        "TERM",
        "COLORTERM",
        "TMUX",
        "TMUX_PANE",
        "TMUX_TMPDIR",
        "CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS",
        "OPENCODE_CONFIG_CONTENT",
        "OPENCODE_SERVER_PASSWORD",
        "OPENCODE_SERVER_USERNAME",
    }
    env = {k: v for k, v in os.environ.items() if k.startswith("COCKPIT_") or k in preserve}
    env.update(
        {
            "HOME": str(root),
            "PATH": f"{root}/bin:/usr/bin:/bin",
            "SHELL": "/bin/sh",
            "LANG": "C.UTF-8",
            "TERM": env.get("TERM", "xterm-256color"),
            "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_DATA_HOME": str(root / "data"),
            "XDG_CACHE_HOME": str(root / "cache"),
            "XDG_STATE_HOME": str(root / "state"),
            "XDG_RUNTIME_DIR": str(root / "run"),
            "TMPDIR": str(root / "tmp"),
            "CODEX_HOME": str(root / ".codex"),
            "CLAUDE_CONFIG_DIR": str(root / "claude"),
            "CLAUDE_SECURESTORAGE_CONFIG_DIR": str(root / "claude"),
            "ANTHROPIC_BASE_URL": url,
            "ANTHROPIC_API_KEY": "sk-cockpit-instruction-validation-placeholder",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
            "DISABLE_TELEMETRY": "1",
            "DISABLE_ERROR_REPORTING": "1",
            "DISABLE_GROWTHBOOK": "1",
            "DISABLE_AUTOUPDATER": "1",
            "DISABLE_UPDATES": "1",
            "DISABLE_INSTALLATION_CHECKS": "1",
            "CLAUDE_CODE_DISABLE_OFFICIAL_MARKETPLACE_AUTOINSTALL": "1",
            "CLAUDE_CODE_DISABLE_AGENT_VIEW": "1",
            "OPENCODE_DISABLE_MODELS_FETCH": "1",
            "OPENCODE_DISABLE_PROJECT_CONFIG": "1",
            "OPENCODE_DISABLE_AUTOUPDATE": "1",
            "OPENCODE_DISABLE_FFF": "1",
            "OPENCODE_DISABLE_AUTOCOMPACT": "1",
            "OPENCODE_EXPERIMENTAL_DISABLE_FILEWATCHER": "true",
            "OPENCODE_TEST_MANAGED_CONFIG_DIR": str(root / "managed"),
        }
    )
    if config["provider"] == "opencode":
        inline = json.loads(env.get("OPENCODE_CONFIG_CONTENT", "{}"))
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
                        "options": {"baseURL": url + "/v1", "apiKey": "fixture-key"},
                        "models": {
                            "fixture-model": {
                                "name": "Fixture",
                                "tool_call": True,
                                "limit": {"context": 65536, "output": 4096},
                            }
                        },
                    }
                },
                "permission": {"*": "deny", "skill": "allow", "cockpit_*": "allow"},
            }
        )
        env["OPENCODE_CONFIG_CONTENT"] = json.dumps(inline)
    return env


def run_bounded(argv, env, timeout, output=None, cwd=None):
    """Retain the waitable leader until its descendants are stopped, even on a signal."""
    child = subprocess.Popen(argv, env=env, start_new_session=True, stdout=output, cwd=cwd)
    marker = Path(env["HOME"]) / "probe-processes" / str(os.getpid())
    try:
        marker.touch()
        until = time.monotonic() + timeout
        while os.waitid(os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is None:
            if (Path(env["HOME"]) / "stop").exists():
                break
            assert time.monotonic() < until, "native fixture process timed out"
            if output is not None:
                assert output.tell() <= 65536, "excessive helper output"
            time.sleep(0.025)
    finally:
        # The real Codex launcher may also signal its app-server wrapper while the
        # fixture is stopping. A second signal must not interrupt group cleanup.
        blocked = signal.pthread_sigmask(
            signal.SIG_BLOCK, {signal.SIGTERM, signal.SIGHUP, signal.SIGINT}
        )
        try:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
            marker.unlink(missing_ok=True)
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, blocked)
    return child.returncode


def launch(config, arguments):
    root = Path(config["root"])
    preflight(root, config["provider"])
    env = environment(config, (root / "ready").read_text())
    if arguments[:1] == ["auth"]:
        # Discovery is deliberately signed out; this never invokes real account commands.
        return 2
    with (root / "launch.log").open("a") as log:
        log.write(json.dumps(arguments) + "\n")
    argv = [config["program"], *arguments]
    if arguments == ["--version"]:
        with (root / "version.txt").open("w+") as output:
            status = run_bounded(argv, env, VERSION_TIMEOUT, output)
            output.seek(0)
            observed = output.read(65537).strip()
        expected = {
            "claude": f"{config['version']} (Claude Code)",
            "codex": f"codex-cli {config['version']}",
            "opencode": config["version"],
        }[config["provider"]]
        assert status == 0 and observed == expected, observed
        with Path(config["program"]).open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        write_json(
            root / "provenance.json",
            {
                "version_output": observed,
                "program_sha256": digest,
                "program_name": Path(config["program"]).name,
                "fixture_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                "captured_at": datetime.datetime.now(datetime.UTC).isoformat(),
            },
        )
        print(observed)
        return 0
    if config["provider"] == "claude":
        argv[1:1] = [
            "--print",
            "--setting-sources",
            "project",
            "--strict-mcp-config",
            "--no-chrome",
        ]
    return run_bounded(argv, env, 50)


def text_blocks(value):
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        return "\n".join(text_blocks(item) for item in value)
    if isinstance(value, dict):
        return value.get("text", "")
    return ""


def blocks(value):
    return value if isinstance(value, list) else [value]


def tool_names(tools, prefix=""):
    for tool in tools:
        if tool.get("type") == "namespace":
            # Captured Codex 0.166.0 groups cockpit_brief/docs under mcp__cockpit.
            yield from tool_names(tool["tools"], prefix + tool["name"] + "__")
        elif name := tool.get("function", tool).get("name"):
            yield prefix + name


def evidence(config, request, number):
    provider = config["provider"]
    messages = request.get("input" if provider == "codex" else "messages", [])
    user = [
        text_blocks(b)
        for m in messages
        if m.get("role") == "user"
        for b in blocks(m.get("content"))
    ]
    prompt = config["prompt"]
    main = prompt in user and bool(request.get("tools"))
    if provider == "codex":
        context = "\n".join(
            text_blocks(m.get("content")) for m in messages if m.get("role") == "developer"
        )
    elif provider == "opencode":
        context = "\n".join(
            text_blocks(m.get("content")) for m in messages if m.get("role") == "system"
        )
    else:
        # 2.1.316's native request carries this context in a system message,
        # before any model response. It is not the first user prompt or tool text.
        context = "\n".join(
            text_blocks(b)
            for m in messages
            if m.get("role") == "system"
            for b in blocks(m.get("content"))
            if text_blocks(b).startswith("SessionStart hook additional context:")
        )
    tools = set(tool_names(request.get("tools", [])))
    prefix = "cockpit_" if provider == "opencode" else "mcp__cockpit__"
    mcp_inline = all(prefix + name in tools for name in ["cockpit_brief", "cockpit_docs"])
    mcp_deferred = (
        provider == "claude"
        and "ToolSearch" in tools
        and all(
            name in context
            for name in ["mcp__cockpit__cockpit_brief", "mcp__cockpit__cockpit_docs"]
        )
    )
    skill = SKILL_DESCRIPTION in context and (
        (provider == "claude" and "- cockpit: " in context and "Skill" in tools)
        or (provider == "opencode" and "<name>cockpit</name>" in context and "skill" in tools)
    )
    return {
        "number": number,
        "main": main,
        "context": context,
        "prompt": prompt if main else None,
        "skill": skill,
        "mcp": mcp_inline or mcp_deferred,
        "mcp_advertisement": "deferred" if mcp_deferred else "inline",
        "request_sha256": hashlib.sha256(json.dumps(request, sort_keys=True).encode()).hexdigest(),
        "request": request,
    }


class Model(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, config):
        super().__init__(("127.0.0.1", 0), Handler)
        self.config = config
        self.lock = threading.Lock()
        self.count = 0


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        root = Path(self.server.config["root"])
        try:
            self.connection.settimeout(5)
            provider = self.server.config["provider"]
            route = {
                "claude": "/v1/messages",
                "codex": "/v1/responses",
                "opencode": "/v1/chat/completions",
            }[provider]
            assert self.path.split("?")[0] == route, self.path
            assert self.headers.get("Authorization") == (
                "Bearer fixture-key" if provider == "opencode" else None
            )
            assert self.headers.get("x-api-key") == (
                "sk-cockpit-instruction-validation-placeholder" if provider == "claude" else None
            )
            size = int(self.headers["Content-Length"])
            assert 0 < size <= LIMIT
            payload = self.rfile.read(size)
            assert len(payload) == size
            request = json.loads(payload)
            with self.server.lock:
                self.server.count += 1
                assert self.server.count <= 8, "unexpected repeated model requests"
                capture = evidence(self.server.config, request, self.server.count)
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            self.respond(request, capture["main"])
            with self.server.lock:
                path = root / "requests.jsonl"
                encoded = json.dumps(capture) + "\n"
                assert len(encoded) + (path.stat().st_size if path.exists() else 0) <= LIMIT
                with path.open("a") as output:
                    output.write(encoded)
        except Exception as error:
            write_json(root / "error.json", {"error": repr(error)})
            self.close_connection = True

    def event(self, value, name=None):
        data = (f"event: {name}\n" if name else "") + "data: " + json.dumps(value) + "\n\n"
        self.wfile.write(data.encode())
        self.wfile.flush()

    def respond(self, request, main):
        provider = self.server.config["provider"]
        if provider == "claude":
            for event in [
                {
                    "type": "message_start",
                    "message": {
                        "id": "msg_fixture",
                        "type": "message",
                        "role": "assistant",
                        "model": request["model"],
                        "content": [],
                        "stop_reason": None,
                        "stop_sequence": None,
                        "usage": {"input_tokens": 0, "output_tokens": 0},
                    },
                },
                {
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""},
                },
                {
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "OK"},
                },
                {"type": "content_block_stop", "index": 0},
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                    "usage": {"output_tokens": 0},
                },
                {"type": "message_stop"},
            ]:
                self.event(event, event["type"])
        elif provider == "codex":
            for event in [
                {"type": "response.created", "response": {"id": "fixture-response"}},
                {
                    "type": "response.output_item.done",
                    "item": {
                        "type": "message",
                        "role": "assistant",
                        "id": "fixture-message",
                        "content": [
                            {
                                "type": "output_text",
                                "text": "OK" if main else '{"title":"Instruction fixture"}',
                            }
                        ],
                    },
                },
                {
                    "type": "response.completed",
                    "response": {
                        "id": "fixture-response",
                        "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0},
                    },
                },
            ]:
                self.event(event)
        else:
            for delta, finish in [({"role": "assistant", "content": "OK"}, None), ({}, "stop")]:
                self.event(
                    {
                        "id": "chatcmpl_fixture",
                        "object": "chat.completion.chunk",
                        "created": 1,
                        "model": "fixture-model",
                        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
                    }
                )
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()


def interrupted(_signal, _frame):
    raise RuntimeError("fixture stopped")


def main():
    config = json.loads(Path(sys.argv[2]).read_text())
    for kind in [signal.SIGTERM, signal.SIGHUP, signal.SIGINT]:
        signal.signal(kind, interrupted)
    if sys.argv[1] == "launch":
        return launch(config, sys.argv[3:])
    assert sys.argv[1] == "serve"
    with Model(config) as model:
        url = f"http://127.0.0.1:{model.server_port}"
        prepare(config, url)
        ready = Path(config["root"]) / "ready"
        partial = ready.with_suffix(".partial")
        partial.write_text(url)
        partial.replace(ready)
        model.serve_forever()
    return 0


if __name__ == "__main__":
    sys.exit(main())
