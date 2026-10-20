#!/usr/bin/env python3
"""Read quota through Codex-managed authentication, independently of agent accounts."""

import argparse
from contextlib import contextmanager
import json
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
from urllib.parse import urlsplit


MAX_MESSAGE_BYTES = 1024 * 1024


class QuotaError(Exception):
    pass


def timeout_handler(signum, frame):
    raise QuotaError("Codex quota request timed out; retry or run codex-usage.sh --login")


def cancellation_handler(signum, frame):
    raise QuotaError("Codex quota request cancelled")


@contextmanager
def app_server():
    home = Path.home()
    binary = home / ".local/npm/bin/codex"
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise QuotaError("Codex CLI missing; install it at ~/.local/npm/bin/codex")
    quota_home = Path(os.environ.get("XDG_DATA_HOME") or home / ".local/share") / "codex-quota"
    quota_home.mkdir(mode=0o700, parents=True, exist_ok=True)
    quota_home.chmod(0o700)
    environment = dict(os.environ, CODEX_HOME=str(quota_home))
    process = subprocess.Popen(
        [str(binary), "app-server", "-c", 'cli_auth_credentials_store="file"'],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        cwd=quota_home,
        env=environment,
    )
    try:
        yield process
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        # A peer may stop reading stdin; terminate before closing buffered pipes.
        process.terminate()
        try:
            process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        process.stdin.close()
        process.stdout.close()


def send(process, message):
    process.stdin.write(json.dumps(message).encode() + b"\n")
    process.stdin.flush()


def receive(process):
    line = process.stdout.readline(MAX_MESSAGE_BYTES + 1)
    if not line or len(line) > MAX_MESSAGE_BYTES or not line.endswith(b"\n"):
        raise QuotaError("Codex quota protocol unavailable; check the Codex CLI version")
    message = json.loads(line)
    if not isinstance(message, dict) or ("method" in message and "id" in message):
        raise QuotaError("Unexpected Codex quota protocol message")
    return message


def request(process, request_id, method, params=None):
    send(process, {"id": request_id, "method": method, "params": params or {}})
    while True:
        message = receive(process)
        if "id" not in message:
            continue
        if message["id"] != request_id:
            raise QuotaError("Unexpected Codex quota response")
        if "error" in message or not isinstance(message.get("result"), dict):
            raise QuotaError("Codex quota request failed; retry or run codex-usage.sh --login")
        return message["result"]


def login(process):
    result = request(process, 2, "account/login/start", {"type": "chatgpt"})
    authorization_url = result.get("authUrl")
    if not isinstance(authorization_url, str):
        raise QuotaError("Codex browser login unavailable; check the Codex CLI version")
    parsed_url = urlsplit(authorization_url)
    if (parsed_url.scheme != "https" or parsed_url.hostname != "auth.openai.com"
            or parsed_url.username or parsed_url.password
            or any(ord(character) < 32 or ord(character) == 127 for character in authorization_url)):
        raise QuotaError("Codex returned an unexpected sign-in URL")
    login_id = result.get("loginId")
    if not isinstance(login_id, str) or not login_id:
        raise QuotaError("Codex browser login unavailable; check the Codex CLI version")
    print("Open this URL to sign in for quota monitoring:", flush=True)
    print(authorization_url, flush=True)
    while True:
        message = receive(process)
        if "id" in message:
            raise QuotaError("Unexpected Codex login response")
        if message.get("method") != "account/login/completed":
            continue
        completion = message.get("params")
        if not isinstance(completion, dict) or completion.get("loginId") != login_id:
            continue
        if completion.get("success") is not True:
            raise QuotaError("Codex sign-in failed or was cancelled; run codex-usage.sh --login again")
        print("Codex quota sign-in complete.", flush=True)
        return


def is_number(value):
    return type(value) in (int, float) and math.isfinite(value)


def normalize_window(window):
    if window is None:
        return None
    if not isinstance(window, dict):
        raise QuotaError("Codex quota response unrecognized")
    used = window.get("usedPercent")
    minutes = window.get("windowDurationMins")
    resets = window.get("resetsAt")
    if (not is_number(used) or not 0 <= used <= 100
            or not is_number(minutes) or minutes <= 0
            or not is_number(resets) or resets < 0):
        raise QuotaError("Codex quota response unrecognized")
    return {"used_percent": used, "limit_window_seconds": minutes * 60, "reset_at": resets}


def normalize_quota(result):
    buckets = result.get("rateLimitsByLimitId")
    if buckets is not None:
        if not isinstance(buckets, dict):
            raise QuotaError("Codex quota response unrecognized")
        bucket = buckets.get("codex")
    else:
        bucket = result.get("rateLimits")
    if not isinstance(bucket, dict) or bucket.get("limitId") not in (None, "codex"):
        raise QuotaError("Codex quota bucket unavailable")
    primary = normalize_window(bucket.get("primary"))
    secondary = normalize_window(bucket.get("secondary"))
    if primary is None and secondary is None:
        raise QuotaError("Codex quota windows unavailable")
    return {"rate_limit": {"primary_window": primary, "secondary_window": secondary}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--login", action="store_true", help="Sign in to the dedicated quota account")
    arguments = parser.parse_args()
    os.umask(0o077)
    signal.signal(signal.SIGALRM, timeout_handler)
    signal.signal(signal.SIGTERM, cancellation_handler)
    signal.setitimer(signal.ITIMER_REAL, 300 if arguments.login else 15)
    try:
        with app_server() as process:
            request(process, 1, "initialize", {"clientInfo": {"name": "dotfiles_codex_quota", "version": "1"}})
            send(process, {"method": "initialized", "params": {}})
            if arguments.login:
                login(process)
                return 0
            account = request(process, 2, "account/read", {"refreshToken": False}).get("account")
            if not isinstance(account, dict) or account.get("type") != "chatgpt":
                raise QuotaError("Codex quota sign-in required; run codex-usage.sh --login")
            quota = normalize_quota(request(process, 3, "account/rateLimits/read"))
        print(json.dumps(quota, allow_nan=False))
        return 0
    except QuotaError as error:
        print(str(error), file=sys.stderr)
    except (OSError, ValueError, TypeError, OverflowError):
        print("Codex quota unavailable; check the Codex CLI and retry", file=sys.stderr)
    except KeyboardInterrupt:
        print("Codex quota request cancelled", file=sys.stderr)
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
    return 1


if __name__ == "__main__":
    sys.exit(main())
