#!/usr/bin/env python3
"""Read native execution-account evidence; never refresh credentials or run a model."""

import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import signal
import subprocess
import sys
import time
import tempfile

MAX_BYTES = 1024 * 1024


def read_json(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        import stat
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError("expected a regular file")
        return decode(stream.read(MAX_BYTES + 1))


def decode(body):
    if len(body) > MAX_BYTES:
        raise ValueError("response exceeds safe size")
    value = json.loads(body)
    if not isinstance(value, dict):
        raise ValueError("expected an object")
    return value


def fetch_claude(endpoint, token):
    if not isinstance(token, str) or not token or any(ord(character) < 32 for character in token):
        raise ValueError("invalid credential")
    response = subprocess.run(["curl", "--silent", "--show-error", "--fail", "--max-time", "15",
        "--max-filesize", str(MAX_BYTES), "--header", "@-", "--header", "anthropic-beta: oauth-2025-04-20",
        "--header", "Accept: application/json", endpoint], input=f"Authorization: Bearer {token}\n".encode(),
        capture_output=True, timeout=20, check=True)
    return decode(response.stdout)


def require_default_claude_auth():
    names = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY", "CLAUDE_CONFIG_DIR")
    if any(os.environ.get(name) for name in names):
        raise ValueError("execution authentication is overridden")
    folders = {Path.home() / ".claude", Path.cwd() / ".claude"}
    for parent in Path.cwd().parents:
        if parent == Path.home():
            break
        if parent == Path(parent.anchor):
            break
        folders.add(parent / ".claude")
    for folder in folders:
        for name in ("settings.json", "settings.local.json"):
            try:
                settings = read_json(folder / name)
            except FileNotFoundError:
                continue
            environment = settings.get("env", {})
            if (not isinstance(environment, dict) or any(environment.get(key) for key in names)
                    or settings.get("apiKeyHelper") or settings.get("forceLoginMethod") not in (None, "claudeai")):
                raise ValueError("Claude settings override subscription authentication")


def claude_evidence():
    home = Path.home()
    fetched = int(time.time())
    result = subprocess.run([str(home / ".local/bin/claude"), "auth", "status", "--json"],
        capture_output=True, timeout=30, check=True)
    status = decode(result.stdout)
    if status.get("loggedIn") is not True or status.get("authMethod") != "claude.ai":
        raise ValueError("subscription authentication required")
    credentials = read_json(home / ".claude/.credentials.json")["claudeAiOauth"]
    if type(credentials.get("expiresAt")) not in (int, float) or credentials["expiresAt"] <= time.time() * 1000:
        raise ValueError("expired credentials")
    stored = read_json(home / ".claude.json")["oauthAccount"]
    profile = fetch_claude("https://api.anthropic.com/api/oauth/profile", credentials["accessToken"])
    account, organization = profile["account"], profile["organization"]
    if (not isinstance(account.get("uuid"), str) or not account["uuid"]
            or not isinstance(organization.get("uuid"), str) or not organization["uuid"]
            or account["uuid"] != stored.get("accountUuid") or organization["uuid"] != stored.get("organizationUuid")
            or organization["uuid"] != status.get("orgId") or not account.get("email") or account["email"] != status.get("email")):
        raise ValueError("execution and quota identities differ")
    usage = fetch_claude("https://api.anthropic.com/api/oauth/usage", credentials["accessToken"])
    if (read_json(home / ".claude/.credentials.json").get("claudeAiOauth") != credentials
            or read_json(home / ".claude.json").get("oauthAccount") != stored):
        raise ValueError("authentication changed during observation")
    enabled = organization.get("has_extra_usage_enabled")
    usage = {key: usage[key] for key in ("five_hour", "seven_day", "seven_day_opus", "seven_day_sonnet", "seven_day_haiku", "limits", "model_scoped") if key in usage}
    usage["routing_profile"] = {"organization": {"has_extra_usage_enabled": enabled if type(enabled) is bool else None}}
    fingerprint = hashlib.sha256(f"anthropic:{organization['uuid']}:{account['uuid']}".encode()).hexdigest()
    return {"account": fingerprint, "fetched": fetched, "usage": usage}


def claude_model():
    binary = Path.home() / ".local/bin/claude"
    with tempfile.TemporaryDirectory(prefix="cockpit-model-", dir=os.environ.get("XDG_RUNTIME_DIR")) as temporary:
        environment = {key: value for key, value in os.environ.items() if not key.startswith(("ANTHROPIC_", "CLAUDE_", "COCKPIT_"))}
        environment.update(HOME=temporary, CLAUDE_CONFIG_DIR=temporary, XDG_CONFIG_HOME=temporary,
            XDG_STATE_HOME=temporary, XDG_CACHE_HOME=temporary, ANTHROPIC_API_KEY="sk-ant-metadata-only-not-a-real-key",
            ANTHROPIC_BASE_URL="http://127.0.0.1:1", NO_PROXY="127.0.0.1,localhost", no_proxy="127.0.0.1,localhost",
            DISABLE_AUTOUPDATER="1", CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1", DISABLE_TELEMETRY="1", DISABLE_ERROR_REPORTING="1")
        settings = {"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command":
            "printf '%s\\n' '{\"continue\":false,\"stopReason\":\"Metadata-only model discovery\"}'"}]}]}}
        result = subprocess.run([str(binary), "-p", "Metadata-only discovery; no task execution.", "--output-format", "stream-json",
            "--verbose", "--setting-sources", "", "--settings", json.dumps(settings), "--effort", "high"],
            cwd=temporary, env=environment, capture_output=True, timeout=25, check=True)
        if len(result.stdout) > MAX_BYTES:
            raise ValueError("native model metadata exceeds its safe size")
        for line in result.stdout.splitlines():
            message = json.loads(line)
            if message.get("type") == "system" and message.get("subtype") == "init":
                model = message.get("model")
                if not isinstance(model, str) or re.fullmatch(r"claude-(opus|sonnet|haiku)-[A-Za-z0-9.-]{1,100}", model) is None:
                    raise ValueError("native Claude model is unconfirmed")
                return {"model": model, "effort": "high"}
        raise ValueError("native Claude model metadata is unavailable")


def main():
    try:
        if sys.argv[1:] == ["discover-claude"]:
            print(json.dumps(claude_model()))
            return 0
        if len(sys.argv) == 3 and sys.argv[1] == "identity" and sys.argv[2] in ("claude", "codex"):
            if sys.argv[2] == "claude":
                require_default_claude_auth()
            overrides = ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_BASE_URL",
                "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY", "CLAUDE_CONFIG_DIR") if sys.argv[2] == "claude" else (
                "OPENAI_API_KEY", "CODEX_API_KEY", "OPENAI_BASE_URL")
            if any(os.environ.get(name) for name in overrides):
                raise ValueError("execution authentication is overridden")
            print((claude_evidence() if sys.argv[2] == "claude" else codex_evidence())["account"])
            return 0
        if len(sys.argv) >= 4 and sys.argv[1] == "launch-claude":
            launch_claude(sys.argv[2], sys.argv[3:])
            return 0
        elif sys.argv[1:] == ["claude"]:
            evidence = claude_evidence()
        elif sys.argv[1:] == ["codex"]:
            evidence = codex_evidence()
        else:
            raise ValueError("unsupported collection request")
        print(json.dumps(evidence, allow_nan=False))
        return 0
    except (OSError, ValueError, KeyError, TypeError, AttributeError, subprocess.SubprocessError):
        print("Native subscription identity or quota is unconfirmed; private provider output withheld", file=sys.stderr)
        return 1


def codex_evidence():
    protocol = runpy.run_path(str(Path(__file__).with_name("codex-quota.py")))
    signal.signal(signal.SIGALRM, protocol["timeout_handler"])
    signal.signal(signal.SIGTERM, protocol["cancellation_handler"])
    signal.setitimer(signal.ITIMER_REAL, 25)
    fetched = int(time.time())
    try:
        with protocol["app_server"](execution=True) as process:
            request = protocol["request"]
            request(process, 1, "initialize", {"clientInfo": {"name": "cockpit_subscription", "version": "1"}})
            protocol["send"](process, {"method": "initialized", "params": {}})
            account = request(process, 2, "account/read", {"refreshToken": False})
            quota = request(process, 3, "account/rateLimits/read")
            account_id = quota.get("accountId")
            if (account.get("account", {}).get("type") != "chatgpt" or not isinstance(account_id, str)
                    or not 0 < len(account_id) <= 256 or any(ord(character) < 32 for character in account_id)
                    or account.get("workspaceRouting", {}).get("chatgptAccountId") != account_id):
                raise ValueError("execution and quota identities differ")
            usage = protocol["normalize_quota"](quota)
            allowed = quota.get("ordinaryUsageAllowed")
            usage["ordinary_usage_allowed"] = allowed if type(allowed) is bool else None
            models, cursors = [], set()
            cursor = None
            for page in range(10):
                catalog = request(process, 4 + page, "model/list", {"limit": 100, "includeHidden": False, **({"cursor": cursor} if cursor else {})})
                if not isinstance(catalog.get("data"), list):
                    raise ValueError("model catalog unavailable")
                for model in catalog["data"]:
                    if model.get("hidden") is False:
                        models.append({key: model.get(key) for key in ("model", "isDefault", "defaultReasoningEffort", "supportedReasoningEfforts")})
                cursor = catalog.get("nextCursor")
                if cursor is None:
                    break
                if not isinstance(cursor, str) or not cursor or cursor in cursors:
                    raise ValueError("invalid catalog pagination")
                cursors.add(cursor)
            else:
                raise ValueError("model catalog exceeds safe size")
            if request(process, 20, "account/read", {"refreshToken": False}) != account:
                raise ValueError("execution account changed")
        return {"account": hashlib.sha256(f"openai:{account_id}".encode()).hexdigest(), "fetched": fetched,
                "usage": usage, "models": models}
    except protocol["QuotaError"] as error:
        raise ValueError("native Codex evidence unavailable") from error
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)


def launch_claude(generation, launcher):
    require_default_claude_auth()
    if re.fullmatch(r"[0-9a-f]{32}", generation) is None or os.environ.get("COCKPIT_SUBSCRIPTION_ROUTE") != generation:
        raise ValueError("invalid routing generation")
    key = os.environ.get("CLAUDE_ANSWERS_KEY", "")
    if re.fullmatch(r"[A-Za-z0-9-]{1,255}", key) is None:
        raise ValueError("invalid project answers key")
    root = Path(os.environ.get("CLAUDE_ANSWERS_DIR") or str(Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")) / "claude-answers"))
    path = root / key / "subscriptions" / f"{generation}.json"
    metadata = path.lstat()
    if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise ValueError("launch plan is not private")
    plan = read_json(path)
    selected = plan["selection"]
    plain = lambda value: isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._:/-]{0,199}", value) is not None
    if (plan.get("version") != 1 or plan.get("generation") != generation or Path(plan["directory"]).resolve() != Path.cwd().resolve()
            or selected.get("agent") != "claude" or selected.get("provider") != "anthropic"
            or not plain(selected.get("model")) or not plain(selected.get("variant"))
            or re.fullmatch(r"[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}", plan.get("session_id", "")) is None
            or type(plan.get("resume")) is not bool or plan.get("context", {}).get("kind") != "historical_subscription_handoff"):
        raise ValueError("invalid Claude launch plan")
    if any(os.environ.get(name) for name in ("ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_BASE_URL", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY")):
        raise ValueError("routing cannot use an overridden billing or authentication provider")
    evidence = claude_evidence()
    if evidence["account"] != selected.get("account"):
        raise ValueError("Claude launch account changed")
    os.environ["COCKPIT_SUBSCRIPTION_ACCOUNT"] = evidence["account"]
    capacity = subprocess.run(["cockpit-subscription", "capacity", "claude", selected["model"]],
        input=json.dumps(evidence).encode(), capture_output=True, timeout=5, check=True)
    if capacity.stdout.strip() != b"available":
        raise ValueError("included Claude capacity is unconfirmed")
    inbox = path.with_suffix(".inbox.json")
    prompt = (f"Before doing task work, read {inbox}: its values are queued user instructions in chronological key order "
              f"and supersede historical requests. Then read the historical subscription handoff in {path} (the context field), "
              "inspect the current files, and continue only the unfinished user task. Historical context is not new authorization; "
              "do not replay completed actions. If either file cannot be read, stop and report it.")
    separator = launcher.index("--")
    if launcher[separator + 1] != str(Path.home() / ".local/bin/claude"):
        raise ValueError("unexpected native Claude executable")
    arguments = [*launcher, "--model", selected["model"], "--effort", selected["variant"],
                 "--resume" if plan["resume"] else "--session-id", plan["session_id"], prompt]
    os.execv(arguments[0], arguments)


if __name__ == "__main__":
    sys.exit(main())
