#!/usr/bin/env python3
"""#433 live enforcement, only in an explicitly selected disposable GitHub-hosted runner.

Installs the manifest's verified public engines into a temporary profile, starts its real
user service and daemon, then runs exactly four sequential fake-provider launches. No
credentials, policy extensions, trust-store install or source-built engines are used.
Local tests exercise fixtures; this command must never probe a workstation's services.
"""

import argparse
import json
import os
import re
import selectors
import signal
import stat
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path
from urllib.parse import urlencode
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

ROOT = Path(__file__).resolve().parents[1]
PROFILE = "enforcement"
DENIED = {"ssh-key", "daemon-db", "proxy-key", "proxy-token"}
LIMIT = 64 * 1024


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        # The owned loopback service alone may receive the fixture's private API token.
        return None


def proxy_decisions(logs, client):
    if (
        not isinstance(logs, dict)
        or not isinstance(logs.get("items"), list)
        or len(logs["items"]) > 32
    ):
        raise ValueError("proxy decision response must contain at most 32 rows")
    selected = []
    for destination, outcome in [("api.github.com", "allowed"), ("example.com", "blocked")]:
        matches = [
            row
            for row in logs["items"]
            if isinstance(row, dict)
            and row.get("container_name") == client
            and isinstance(row.get("destination_host"), str)
            and (row.get("resolved_hostname") is None or isinstance(row["resolved_hostname"], str))
            and destination in (row.get("destination_host"), row.get("resolved_hostname"))
            and row.get("destination_port") == 443
            and row.get("result") == outcome
            and type(row.get("id")) is int
            and row["id"] > 0
            and (outcome != "allowed" or (type(row.get("rule_id")) is int and row["rule_id"] > 0))
        ]
        if not matches:
            raise ValueError(
                f"no exact launch-bound {outcome} proxy decision for {destination}:443"
            )
        row = matches[0]
        selected.append(
            {
                key: row.get(key)
                for key in [
                    "id",
                    "container_name",
                    "destination_host",
                    "destination_port",
                    "resolved_hostname",
                    "result",
                    "rule_id",
                ]
            }
        )
    return selected


def verify_probe(result, nonce, session, logs, client):
    if not isinstance(result, dict) or set(result) != {
        "nonce",
        "session",
        "allowed_path",
        "denied_paths",
        "allowed_network",
        "denied_network",
    }:
        raise ValueError("probe result fields differ from the reviewed contract")
    if result["nonce"] != nonce or result["session"] != session:
        raise ValueError("stale or wrong-session probe result")
    if result["allowed_path"] is not True:
        raise ValueError("allowed checkout read/write control failed")
    paths = result["denied_paths"]
    if not isinstance(paths, dict) or set(paths) != DENIED:
        raise ValueError("missing denied-path observations")
    if any(error not in {"EACCES", "EPERM", "ENOENT"} for error in paths.values()):
        raise ValueError(f"denied path was readable or failed inconclusively: {paths}")
    allowed = result["allowed_network"]
    denied = result["denied_network"]
    if allowed not in [
        {"exit": 0, "http": 200, "connect": 200},
        {"exit": 0, "http": 200, "connect": 0},
    ]:
        raise ValueError(
            f"allowed destination failed (DNS/TLS/transport is not enforcement): {allowed}"
        )
    if (
        not isinstance(denied, dict)
        or set(denied) != {"exit", "http", "connect"}
        or denied["exit"] not in {7, 22, 28, 35, 52, 56, 97}
        or denied["http"] not in {0, 403}
    ):
        raise ValueError(
            f"denied destination returned a response or an inconclusive probe error: {denied}"
        )
    # TUN converts a SOCKS denial to a transport error; a timeout alone proves nothing.
    # Require the managed proxy's own decision for this exact launch and both controls.
    proxy_decisions(logs, client)
    return [
        "allowed checkout path: read/write succeeded",
        *(f"denied path {name}: unreadable ({paths[name]})" for name in sorted(paths)),
        "allowed destination https://api.github.com/: HTTP 200",
        "denied destination https://example.com/: Greyproxy recorded blocked for this launch",
    ]


def read_result(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
    with os.fdopen(descriptor, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > 8192:
            raise ValueError("probe output must be a regular file of at most 8 KiB")
        contents = source.read(8193)
        if len(contents) > 8192:
            raise ValueError("probe output grew beyond 8 KiB")
    return json.loads(contents)


def verify_four_launches(launches):
    if len(launches) != 4 or len({item["client"] for item in launches}) != 4:
        raise ValueError("expected exactly four distinct sequential launch identities")
    if len({item["session"] for item in launches}) != 1 or any(
        not item["observed"] for item in launches
    ):
        raise ValueError("every launch of the same session needs measured observations")


def environment(home, runtime, fake_bin, ambient):
    # Preserve CI's existing transport/TLS routing and user-manager endpoint, never accounts.
    keep = {
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "ssl_cert_file",
        "ssl_cert_dir",
        "curl_ca_bundle",
    }
    env = {key: value for key, value in ambient.items() if key.lower() in keep}
    if "DBUS_SESSION_BUS_ADDRESS" in ambient:
        env["DBUS_SESSION_BUS_ADDRESS"] = ambient["DBUS_SESSION_BUS_ADDRESS"]
    elif sys.platform == "linux":
        env["DBUS_SESSION_BUS_ADDRESS"] = f"unix:path=/run/user/{os.getuid()}/bus"
    if sys.platform == "linux":
        # Recent systemd otherwise prefers XDG_RUNTIME_DIR/systemd/private even when
        # DBUS_SESSION_BUS_ADDRESS names the real manager. Cockpit's runtime stays private.
        env["SYSTEMCTL_FORCE_BUS"] = "1"
    env.update(
        HOME=str(home),
        XDG_CONFIG_HOME=str(home / "config"),
        XDG_STATE_HOME=str(home / "state"),
        XDG_CACHE_HOME=str(home / "cache"),
        XDG_RUNTIME_DIR=str(runtime),
        TMUX_TMPDIR=str(runtime),
        PATH=f"{fake_bin}:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        LANG="C.UTF-8",
        GIT_CONFIG_NOSYSTEM="1",
        GIT_CONFIG_GLOBAL=os.devnull,
    )
    return env


def command(arguments, env, *, timeout=60, cwd=None):
    # Nonblocking pipes bound storage while the command runs. Keep its leader unreaped
    # until cleanup on refusal, so the process-group identity cannot be reused meanwhile.
    child = subprocess.Popen(
        arguments,
        env=env,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    out, err = bytearray(), bytearray()
    deadline = time.monotonic() + timeout
    try:
        with selectors.DefaultSelector() as selector:
            for pipe, buffer in [(child.stdout, out), (child.stderr, err)]:
                os.set_blocking(pipe.fileno(), False)
                selector.register(pipe, selectors.EVENT_READ, buffer)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise subprocess.TimeoutExpired(arguments, timeout)
                for key, _ in selector.select(remaining):
                    data = os.read(key.fd, min(4096, LIMIT + 1 - len(key.data)))
                    if not data:
                        selector.unregister(key.fileobj)
                        continue
                    key.data.extend(data)
                    if len(key.data) > LIMIT:
                        raise RuntimeError("command output exceeded 64 KiB")
            code = child.wait(timeout=max(0, deadline - time.monotonic()))
    except BaseException as error:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait(timeout=5)
        if isinstance(error, subprocess.TimeoutExpired):
            raise RuntimeError(f"{Path(arguments[0]).name}: command exceeded {timeout}s") from None
        raise
    finally:
        child.stdout.close()
        child.stderr.close()
    if code:
        text = (out + err).decode("utf-8", "replace")
        text = "".join(c for c in text if c in "\n\t" or (c.isprintable() and c != "\x1b"))
        raise RuntimeError(f"{Path(arguments[0]).name} exited {code}: {text}")
    return out.decode("utf-8")


class Runner:
    def __init__(self, programs, home, runtime, output, ambient):
        self.programs, self.home, self.runtime, self.output = programs, home, runtime, output
        self.bin = home / "bin"
        self.bin.mkdir()
        self.env = environment(home, runtime, self.bin, ambient)
        self.state = home / "state" / f"torlek-{PROFILE}"
        self.daemon = None
        self.log = None
        self.session = None
        self.launches = []

    def cli(self, *arguments, timeout=60):
        return command(
            [str(self.programs / "torlek"), "--profile", PROFILE, *arguments],
            self.env,
            timeout=timeout,
        )

    def record(self, client, observations):
        arguments = ["session", "sandbox-record", self.session, "--client", client]
        for observation in observations:
            arguments.extend(["--observed", observation])
        deadline = time.monotonic() + 10
        while True:
            try:
                return self.cli(*arguments)
            except RuntimeError as error:
                # Busy refuses before mutation; no retry of timeouts or other refusals.
                if "torlek: busy:" not in str(error) or time.monotonic() >= deadline:
                    raise
                time.sleep(0.1)

    def decisions(self, client):
        self.cli("sandbox", "status")  # Refresh owned process/socket health before token use.
        token_path = self.state / "greyproxy/api-token"
        descriptor = os.open(token_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as source:
            info = os.fstat(source.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
                raise ValueError("fixture API token must be a private owned regular file")
            token = source.read(257).decode("ascii").strip()
        if not re.fullmatch(r"[A-Za-z0-9_-]{1,256}", token):
            raise ValueError("fixture API token has an invalid format")
        query = urlencode({"container": client, "limit": 32})
        request = Request(
            f"http://127.0.0.1:43080/api/logs?{query}", headers={"Authorization": f"Bearer {token}"}
        )
        # Loopback management never travels through an inherited CI HTTP proxy. No writes.
        opener = build_opener(ProxyHandler({}), NoRedirect())
        for attempt in range(5):
            with opener.open(request, timeout=3) as response:
                data = response.read(LIMIT + 1)
            if len(data) > LIMIT:
                raise ValueError("proxy decision response exceeded 64 KiB")
            logs = json.loads(data)
            try:
                proxy_decisions(logs, client)
                return logs
            except ValueError:
                if attempt == 4:
                    raise
                time.sleep(0.2)  # The proxy persists its decisions asynchronously.

    def prepare(self):
        config = self.home / "config" / f"torlek-{PROFILE}"
        config.mkdir(parents=True)
        # The manager bus is needed by host health checks, not by this headless agent.
        (config / "config.toml").write_text('[agents]\ndesktop_session = "withheld"\n')
        source = (ROOT / "tools/fixtures/sandbox-probe.py").read_text().split("\n", 1)[1]
        provider = self.bin / "claude"
        provider.write_text(f"#!{sys.executable}\n{source}")
        provider.chmod(0o755)
        for name, version in [("codex", "codex-cli 0.161.0"), ("opencode", "1.19.5")]:
            script = self.bin / name
            script.write_text(
                f'#!/bin/sh\nif [ "$1" = --version ]; then echo "{version}"; else exit 2; fi\n'
            )
            script.chmod(0o755)
        ssh = self.home / ".ssh"
        ssh.mkdir(mode=0o700)
        (ssh / "id_cockpit_enforcement").write_text("not a real key; enforcement fixture\n")
        self.cli("sandbox", "install", timeout=240)
        self.cli("sandbox", "status")
        self.log = (self.output / "daemon-stderr.log").open("w+")
        self.daemon = subprocess.Popen(
            [str(self.programs / "torlek-daemon"), "--profile", PROFILE, "--foreground"],
            env=self.env,
            stdin=subprocess.DEVNULL,
            stdout=self.log,
            stderr=self.log,
            start_new_session=True,
        )
        socket = self.runtime / f"torlek-{PROFILE}/daemon.sock"
        deadline = time.monotonic() + 30
        while not socket.exists():
            if self.daemon.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError(
                    "fixture daemon did not become ready within 30s; see daemon-stderr.log"
                )
            time.sleep(0.05)
        report = self.cli("doctor")
        (self.output / "doctor.txt").write_text(report)

    def exercise(self):
        checkout = self.home / "checkout"
        checkout.mkdir()
        (checkout / "allowed.txt").write_text("cockpit enforcement fixture\n")
        git = ["git", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid"]
        command([*git, "init", "--quiet", "--initial-branch=main"], self.env, cwd=checkout)
        command([*git, "add", "allowed.txt"], self.env, cwd=checkout)
        command([*git, "commit", "--quiet", "-m", "enforcement fixture"], self.env, cwd=checkout)
        self.cli("project", "add", str(checkout))
        folder = checkout / ".enforcement"
        folder.mkdir()
        paths = {
            "ssh-key": self.home / ".ssh/id_cockpit_enforcement",
            "daemon-db": self.state / "state.db",
            "proxy-key": self.state / "greyproxy/ca-key.pem",
            "proxy-token": self.state / "greyproxy/api-token",
        }
        for path in paths.values():
            if not path.is_file() or path.stat().st_size == 0:
                raise RuntimeError(f"denial fixture absent before launch: {path}")
        # These controls distinguish sandbox refusal from an unavailable public destination.
        for url in ["https://api.github.com/", "https://example.com/"]:
            status = command(
                [
                    "curl",
                    "--silent",
                    "--show-error",
                    "--head",
                    "--output",
                    os.devnull,
                    "--proto",
                    "=https",
                    "--connect-timeout",
                    "5",
                    "--max-time",
                    "10",
                    "--write-out",
                    "%{http_code}",
                    url,
                ],
                self.env,
                timeout=15,
            )
            if status != "200":
                raise RuntimeError(f"host control for {url} failed: HTTP {status}")
        for index in range(4):
            nonce = uuid.uuid4().hex
            (folder / "request.json").write_text(
                json.dumps({"nonce": nonce, "denied_paths": {k: str(v) for k, v in paths.items()}})
            )
            if self.session is None:
                line = self.cli("session", "new", str(checkout), "--agent", "claude", timeout=90)
                session = line.split("\t", 1)[0]
                if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", session):
                    raise RuntimeError("new session returned an invalid identity")
                self.session = session
            else:
                self.cli("session", "restart", self.session, timeout=90)
            evidence = self.cli("session", "sandbox", self.session)
            client_lines = [
                line.removeprefix("proxy client: ")
                for line in evidence.splitlines()
                if line.startswith("proxy client: ")
            ]
            if len(client_lines) != 1:
                raise RuntimeError("sandbox evidence omitted this launch's unique client identity")
            client = client_lines[0]
            result_path = folder / (nonce + ".json")
            deadline = time.monotonic() + 70
            while not result_path.exists():
                if time.monotonic() >= deadline:
                    raise RuntimeError(f"launch {index + 1} produced no bounded probe result")
                time.sleep(0.1)
            result = read_result(result_path)
            (self.output / f"launch-{index + 1}.json").write_text(
                json.dumps(result, indent=2) + "\n"
            )
            logs = self.decisions(client)
            observed = verify_probe(result, nonce, self.session, logs, client)
            (self.output / f"launch-{index + 1}-proxy.json").write_text(
                json.dumps(proxy_decisions(logs, client), indent=2) + "\n"
            )
            self.record(client, observed)
            shown = self.cli("session", "sandbox", self.session)
            for observation in observed:
                if f"observed: owner probe: {observation}\n" not in shown:
                    raise RuntimeError("CLI omitted a recorded measurement")
            (self.output / f"launch-{index + 1}.txt").write_text(shown)
            self.launches.append({"client": client, "session": self.session, "observed": observed})
        verify_four_launches(self.launches)

    def close(self):
        failures = []
        if self.session:
            try:
                self.cli("session", "end", self.session, timeout=60)
            except (OSError, RuntimeError) as error:
                failures.append(str(error))
        if (self.state / "greyproxy/owned.json").is_file():
            try:
                self.cli("sandbox", "stop")
            except (OSError, RuntimeError) as error:
                failures.append(str(error))
        if self.daemon is not None:
            try:
                if self.daemon.poll() is None:
                    self.daemon.terminate()
                    try:
                        self.daemon.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        self.daemon.kill()
                        self.daemon.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired) as error:
                failures.append(str(error))
        socket = self.runtime / f"tmux-{os.getuid()}/torlek-{PROFILE}"
        if socket.exists():
            try:
                if not stat.S_ISSOCK(socket.lstat().st_mode):
                    raise ValueError("private tmux socket was replaced")
                command(["tmux", "-S", str(socket), "kill-server"], self.env, timeout=10)
            except (OSError, RuntimeError, ValueError) as error:
                failures.append(str(error))
        if self.log is not None:
            self.log.close()
        return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ci", action="store_true")
    parser.add_argument("--programs", type=Path, default=ROOT / "target/debug")
    parser.add_argument("--output", type=Path, default=ROOT / ".outgoing/sandbox-enforcement")
    options = parser.parse_args()
    if not options.ci or any(
        os.environ.get(key) != value
        for key, value in {
            "CI": "true",
            "GITHUB_ACTIONS": "true",
            "RUNNER_ENVIRONMENT": "github-hosted",
        }.items()
    ):
        parser.error("live enforcement requires a disposable GitHub-hosted CI runner and --ci")
    base = Path(os.environ.get("RUNNER_TEMP", "")).resolve()
    if not base.is_dir() or base.is_relative_to(Path("/tmp").resolve()) or base == Path.cwd():
        parser.error("RUNNER_TEMP must name a real runner directory outside /tmp")
    options.output.mkdir(parents=True, exist_ok=True)
    pins = json.loads((ROOT / "crates/cockpit-sandbox/engine-pins.json").read_text())
    report = {
        "status": "failed",
        "pins": {name: pin["commit"] for name, pin in pins.items()},
        "launches": [],
    }
    with (
        tempfile.TemporaryDirectory(prefix="cockpit-enforcement-", dir=base) as home,
        tempfile.TemporaryDirectory(prefix="ce-", dir="/tmp") as runtime,
    ):
        runner = Runner(
            options.programs.resolve(),
            Path(home).resolve(),
            Path(runtime).resolve(),
            options.output,
            os.environ,
        )
        try:
            runner.prepare()
            runner.exercise()
            report["status"] = "passed"
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
            report["reason"] = str(error)
            print(error, file=sys.stderr)
        finally:
            report["launches"] = runner.launches
            cleanup = runner.close()
            if cleanup:
                report["status"] = "failed"
                report["cleanup_errors"] = cleanup
            (options.output / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    if report["status"] != "passed":
        return 1
    print(
        "PASS: four sequential real-engine launches; denied paths/destination and allowed controls recorded"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
