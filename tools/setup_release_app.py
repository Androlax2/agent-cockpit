#!/usr/bin/env python3
"""Host-approved, interactive Release App bootstrap for Androlax2/agent-cockpit.

Registration requires GitHub's browser confirmation. This does not publish, tag,
change workflows, rotate credentials or implement release-plz PR updates.
"""

import argparse
import base64
import fcntl
import hmac
import html
import json
import os
import re
import secrets
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

REPOSITORY = "Androlax2/agent-cockpit"
OWNER = "Androlax2"
VARIABLE = "RELEASE_APP_CLIENT_ID"
SECRET = "RELEASE_APP_PRIVATE_KEY"
PERMISSIONS = {"contents": "write", "pull_requests": "write"}
API_VERSION = "2022-11-28"


class SetupError(Exception):
    """Only deliberately public messages may cross this boundary."""


class Store:
    """Single operator, owner-only recovery state, outside every Git checkout."""

    def __init__(self, directory):
        self.directory = Path(directory).absolute()
        self.fd = None

    def __enter__(self):
        for parent in (self.directory, *self.directory.parents):
            if parent.is_symlink() or (parent / ".git").exists():
                raise SetupError(
                    "State must be outside Git checkouts and have no symlink ancestors."
                )
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.fd = os.open(self.directory, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        info = os.fstat(self.fd)
        if info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
            os.close(self.fd)
            self.fd = None
            raise SetupError("Recovery directory must be owned by you with mode 0700.")
        try:
            fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            os.close(self.fd)
            self.fd = None
            raise SetupError("Another setup process holds the recovery directory.") from None
        return self

    def __exit__(self, *_):
        os.close(self.fd)

    def load(self):
        try:
            fd = os.open("state.json", os.O_RDONLY | os.O_NOFOLLOW, dir_fd=self.fd)
        except FileNotFoundError:
            return {}
        with os.fdopen(fd) as stream:
            info = os.fstat(stream.fileno())
            if (
                not stat.S_ISREG(info.st_mode)
                or info.st_uid != os.getuid()
                or stat.S_IMODE(info.st_mode) != 0o600
                or info.st_nlink != 1
            ):
                raise SetupError("Recovery file must be a private, owner-only regular file.")
            result = json.load(stream)
        if not isinstance(result, dict):
            raise SetupError("Invalid recovery record; reconcile manually without displaying it.")
        return result

    def save(self, value):
        self.load()  # Refuse an unexpected symlink or unsafe existing file before replacement.
        fd, name = tempfile.mkstemp(prefix=".state-", dir=self.directory)
        try:
            with os.fdopen(fd, "w") as stream:
                json.dump(value, stream)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(name, self.directory / "state.json")
            os.fsync(self.fd)
        finally:
            Path(name).unlink(missing_ok=True)


def manifest(callback):
    return {
        "name": "Agent Cockpit Release",
        "url": f"https://github.com/{REPOSITORY}",
        "redirect_url": callback,
        # Required URL remains inert: this App consumes no webhook events.
        "hook_attributes": {"url": "https://example.invalid/unused", "active": False},
        "public": False,
        "default_permissions": dict(PERMISSIONS),
        "default_events": [],
        "request_oauth_on_install": False,
    }


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


def api(method, path, token=None, data=None):
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": API_VERSION,
        "User-Agent": "agent-cockpit-release-app-setup",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    body = None if data is None else json.dumps(data).encode()
    if body is not None:
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request("https://api.github.com" + path, body, headers, method=method)
    try:
        with urllib.request.build_opener(NoRedirect).open(request, timeout=30) as response:
            body = response.read(2_000_001)
        if len(body) > 2_000_000:
            raise SetupError("GitHub response exceeded the setup limit.")
        return json.loads(body) if body else None
    except urllib.error.HTTPError as error:
        # The conversion URL contains a one-time secret; never print exceptions or bodies.
        status = error.code
        error.close()
        raise SetupError(
            f"GitHub request failed (HTTP {status}); reconcile before retrying."
        ) from None
    except (OSError, ValueError):
        raise SetupError(
            "GitHub response unavailable or invalid; reconcile before retrying."
        ) from None


def gh(arguments, data=None):
    environment = dict(os.environ)
    environment.pop("GH_DEBUG", None)
    environment.update(GH_HOST="github.com", GH_PROMPT_DISABLED="1", GH_PAGER="cat")
    result = subprocess.run(
        ["gh", *arguments],
        input=data or b"",
        capture_output=True,
        env=environment,
        timeout=60,
        check=False,
    )
    if result.returncode:
        raise SetupError(f"GitHub CLI failed (exit {result.returncode}); no raw output displayed.")
    if arguments[0] == "secret" or "--method" in arguments:
        return None
    try:
        return json.loads(result.stdout)
    except ValueError:
        raise SetupError("GitHub CLI returned invalid metadata; no raw output displayed.") from None


def remote_configuration():
    result = []
    for kind in ("variables", "secrets"):
        pages = gh(
            ["api", "--paginate", "--slurp", f"repos/{REPOSITORY}/actions/{kind}?per_page=100"]
        )
        result.append({item["name"]: item for page in pages for item in page[kind]})
    return result


def check_app(app):
    permissions = dict(app["permissions"])
    if permissions.get("metadata") == "read":
        del permissions["metadata"]
    if (
        app["owner"]["login"] != OWNER
        or permissions != PERMISSIONS
        or app["events"]
        or not isinstance(app["id"], int)
        or not re.fullmatch(r"[A-Za-z0-9_.-]+", app["client_id"])
        or not re.fullmatch(r"[a-z0-9-]+", app["slug"])
    ):
        raise SetupError("App identity, permissions or events differ from the approved setup.")


def install_url(app):
    check_app(app)
    return f"https://github.com/apps/{app['slug']}/installations/new"


def convert(store, code):
    store.save({"phase": "exchanging"})
    response = api("POST", f"/app-manifests/{urllib.parse.quote(code, safe='')}/conversions")
    # Persist the one-time PEM before further validation, so a scope error cannot lose it.
    app = {
        key: response[key] for key in ("id", "client_id", "slug", "owner", "permissions", "events")
    }
    store.save({"phase": "registered", "app": app, "pem": response["pem"]})
    check_app(app)


def registration_server(store):
    class Handler(BaseHTTPRequestHandler):
        def setup(self):
            self.request.settimeout(5)
            super().setup()

        def log_message(self, *_):
            pass  # Request paths contain the one-time manifest conversion code.

        def reply(self, status, body):
            self.send_response(status)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header(
                "Content-Security-Policy",
                "default-src 'none'; form-action https://github.com; frame-ancestors 'none'",
            )
            self.end_headers()
            self.wfile.write(body.encode())

        def do_GET(self):
            parsed = urllib.parse.urlsplit(self.path)
            if self.headers.get("Host") != f"127.0.0.1:{self.server.server_port}":
                self.reply(403, "Unexpected host.")
                return
            if parsed.path == "/start/" + self.server.state and not self.server.consumed:
                payload = html.escape(json.dumps(manifest(self.server.callback)), quote=True)
                self.reply(
                    200,
                    "<!doctype html><title>Register Cockpit Release App</title>"
                    f'<form method="post" action="https://github.com/settings/apps/new?state={self.server.state}">'
                    f'<input type="hidden" name="manifest" value="{payload}">'
                    "<p>Signed in as Androlax2: register a private App with contents and "
                    "pull-request write access. Installation is a separate confirmation.</p>"
                    "<button>Continue to GitHub</button></form>",
                )
                return
            query = urllib.parse.parse_qs(parsed.query)
            state = query.get("state", [""])
            code = query.get("code", [""])
            if (
                parsed.path != "/callback"
                or self.server.consumed
                or len(state) != 1
                or len(code) != 1
                or not re.fullmatch(r"[A-Za-z0-9_-]{1,512}", code[0])
                or not re.fullmatch(r"[A-Za-z0-9_-]{43}", state[0])
                or not hmac.compare_digest(state[0], self.server.state)
            ):
                self.reply(400, "Invalid or already consumed registration callback.")
                return
            self.server.consumed = True
            try:
                convert(store, code[0])
                self.reply(
                    200,
                    "Registration saved privately. Return to the setup command for installation.",
                )
            except (SetupError, OSError, ValueError, KeyError, TypeError):
                self.server.failed = True
                self.reply(
                    500,
                    "Registration incomplete. Preserve the private recovery record and reconcile manually.",
                )

    server = HTTPServer(("127.0.0.1", 0), Handler)
    server.state = secrets.token_urlsafe(32)
    server.callback = f"http://127.0.0.1:{server.server_port}/callback"
    server.consumed = False
    server.failed = False
    server.timeout = 1
    return server


def register(store):
    record = store.load()
    if record.get("app"):
        print("App already recorded. Install only agent-cockpit: " + install_url(record["app"]))
        return
    if record:
        raise SetupError(
            "Registration was interrupted. Inspect GitHub App settings and reconcile the private record; do not register another App."
        )
    if gh(["api", "user"])["login"] != OWNER:
        raise SetupError("Host GitHub CLI must be authenticated as Androlax2.")
    variables, configured_secrets = remote_configuration()
    if VARIABLE in variables or SECRET in configured_secrets:
        raise SetupError("Release credentials already exist; registration will not replace them.")
    with registration_server(store) as server:
        store.save({"phase": "waiting"})
        print(
            f"Open on this host: http://127.0.0.1:{server.server_port}/start/{server.state}",
            flush=True,
        )
        deadline = time.monotonic() + 3500
        while not server.consumed and time.monotonic() < deadline:
            server.handle_request()
        if not server.consumed or server.failed:
            raise SetupError(
                "Registration incomplete. Preserve the private record and reconcile before retrying."
            )
    print("Install only agent-cockpit: " + install_url(store.load()["app"]))


def sign_jwt(record, directory):
    def encode(value):
        return base64.urlsafe_b64encode(value).rstrip(b"=")

    now = int(time.time())
    payload = (
        encode(b'{"alg":"RS256","typ":"JWT"}')
        + b"."
        + encode(
            json.dumps(
                {"iat": now - 60, "exp": now + 540, "iss": record["app"]["client_id"]}
            ).encode()
        )
    )
    fd, name = tempfile.mkstemp(prefix=".signing-key-", dir=directory)
    try:
        with os.fdopen(fd, "w") as stream:
            stream.write(record["pem"])
        result = subprocess.run(
            ["openssl", "dgst", "-sha256", "-sign", name],
            input=payload,
            capture_output=True,
            timeout=15,
            check=False,
        )
        if result.returncode:
            raise SetupError("OpenSSL could not sign the App identity; no raw output displayed.")
        return (payload + b"." + encode(result.stdout)).decode()
    finally:
        Path(name).unlink(missing_ok=True)


def verify_installation(record, directory):
    jwt = sign_jwt(record, directory)
    current = api("GET", "/app", jwt)
    check_app(current)
    if any(current[key] != record["app"][key] for key in ("id", "client_id", "slug")):
        raise SetupError("The saved key authenticates a different App.")
    installations = api("GET", "/app/installations?per_page=2", jwt)
    if len(installations) != 1:
        raise SetupError("Exactly one App installation is required; install only agent-cockpit.")
    installation = installations[0]
    if (
        installation["app_id"] != current["id"]
        or installation["account"]["login"] != OWNER
        or installation["repository_selection"] != "selected"
        or installation["suspended_at"]
        or installation["permissions"] != current["permissions"]
    ):
        raise SetupError("Installation owner, permissions or repository selection is not approved.")
    # Do NOT narrow repositories here: that would hide an overbroad installation.
    token = api(
        "POST",
        f"/app/installations/{int(installation['id'])}/access_tokens",
        jwt,
        {"permissions": {"metadata": "read"}},
    )["token"]
    try:
        repos = api("GET", "/installation/repositories?per_page=2", token)
        if repos["total_count"] != 1 or [repo["full_name"] for repo in repos["repositories"]] != [
            REPOSITORY
        ]:
            raise SetupError("App installation must select only Androlax2/agent-cockpit.")
    finally:
        api("DELETE", "/installation/token", token)


def configure(store):
    record = store.load()
    if not record.get("app") or not record.get("pem"):
        raise SetupError("Register the App first; no private recovery credentials found.")
    if record["phase"] == "uploading":
        raise SetupError(
            "Previous secret write is uncertain. Reconcile manually; it will not be retried."
        )
    verify_installation(record, store.directory)
    variables, configured_secrets = remote_configuration()
    variable = variables.get(VARIABLE)
    secret = configured_secrets.get(SECRET)
    if variable and variable["value"] != record["app"]["client_id"]:
        raise SetupError("Existing Release App client ID differs; refusing replacement.")
    if record["phase"] == "configured":
        if not variable or not secret or secret["updated_at"] != record["secret_updated_at"]:
            raise SetupError(
                "Configuration changed since setup; reconcile manually without replacement."
            )
        print(
            "App scope verified; existing configuration left unchanged (secret values are unreadable)."
        )
        return
    if secret:
        raise SetupError("Release App private-key secret already exists; refusing replacement.")
    if not variable:
        gh(
            ["api", "--method", "POST", f"repos/{REPOSITORY}/actions/variables", "--input", "-"],
            json.dumps({"name": VARIABLE, "value": record["app"]["client_id"]}).encode(),
        )
    # Durable intent before the irreversible API write; a lost response never retries a PUT.
    record["phase"] = "uploading"
    store.save(record)
    gh(["secret", "set", SECRET, "--repo", REPOSITORY], record["pem"].encode())
    _, configured_secrets = remote_configuration()
    record["secret_updated_at"] = configured_secrets[SECRET]["updated_at"]
    record["phase"] = "configured"
    store.save(record)
    print(
        "Verified repository scope; configured RELEASE_APP_CLIENT_ID and RELEASE_APP_PRIVATE_KEY."
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("register", "configure"))
    parser.add_argument(
        "--state-dir", type=Path, default=Path.home() / ".local/state/cockpit-release-app"
    )
    args = parser.parse_args(argv)
    try:
        with Store(args.state_dir) as store:
            {"register": register, "configure": configure}[args.command](store)
    except SetupError as error:
        print(f"Release App setup: {error}", file=sys.stderr)
        return 1
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        subprocess.SubprocessError,
        KeyboardInterrupt,
    ):
        # Never include exception text: JSON, URLs, CLI output and subprocess input can be secret.
        print(
            "Release App setup failed or interrupted; preserve private state and reconcile before retrying.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
