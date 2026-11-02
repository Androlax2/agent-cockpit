#!/usr/bin/env python3
"""Host-approved proposal App and Git-only deploy-key setup for Androlax2/agent-cockpit.

Each command needs its own reviewed host approval. Registration requires browser
confirmation; environment protection must already exist before credential storage.
"""

import argparse
import json
import re
import subprocess
import sys
import tempfile
import time
import urllib.parse
from pathlib import Path

from setup_release_app import (
    SetupError,
    Store,
    api,
    gh,
    registration_server,
    sign_jwt,
)

REPOSITORY = "Androlax2/agent-cockpit"
OWNER = "Androlax2"
ENVIRONMENT = "release-proposal"
ENV_PATH = f"repos/{REPOSITORY}/environments/{ENVIRONMENT}"
VARIABLE = "PROPOSAL_APP_CLIENT_ID"
APP_SECRET = "PROPOSAL_APP_PRIVATE_KEY"
SSH_SECRET = "PROPOSAL_GIT_SSH_KEY"
KEY_TITLE = "release-proposal Git-only push"
PERMISSIONS = {"contents": "read", "pull_requests": "write"}


def manifest(callback):
    return {
        "name": "Agent Cockpit Proposal",
        "url": f"https://github.com/{REPOSITORY}",
        "redirect_url": callback,
        "hook_attributes": {"url": "https://example.invalid/unused", "active": False},
        "public": False,
        "default_permissions": dict(PERMISSIONS),
        "default_events": [],
        "request_oauth_on_install": False,
    }


def check_app(app):
    permissions = dict(app["permissions"])
    if permissions.get("metadata") == "read":
        del permissions["metadata"]
    if (
        app["owner"]["login"] != OWNER
        or permissions != PERMISSIONS
        or app["events"]
        or type(app["id"]) is not int
        or app["id"] <= 0
        or app["id"] == 5573576
        or not re.fullmatch(r"[A-Za-z0-9_.-]+", app["client_id"])
        or not re.fullmatch(r"[a-z0-9-]+", app["slug"])
    ):
        raise SetupError("Proposal App identity, permissions or events differ from approved scope.")


def load_record(store):
    record = store.load()
    if record and record.get("purpose") != ENVIRONMENT:
        raise SetupError("Not proposal recovery state; refusing to change it.")
    if record.get("phase") in ("exchanging", "registering-key", "uploading"):
        raise SetupError("Previous write is uncertain; reconcile privately before continuing.")
    return record


def metadata(path, field=None):
    pages = gh(["api", "--paginate", "--slurp", path + "?per_page=100"])
    return [item for page in pages for item in (page[field] if field else page)]


def configuration():
    return tuple(
        {item["name"]: item for item in metadata(f"{ENV_PATH}/{kind}", kind)}
        for kind in ("variables", "secrets")
    )


def check_environment():
    environment = gh(["api", ENV_PATH])
    refs = metadata(f"{ENV_PATH}/deployment-branch-policies", "branch_policies")
    if (
        environment["name"] != ENVIRONMENT
        or any(rule["type"] != "branch_policy" for rule in environment["protection_rules"])
        or environment["deployment_branch_policy"]
        != {"protected_branches": False, "custom_branch_policies": True}
        or [(ref["name"], ref["type"]) for ref in refs] != [("main", "branch")]
    ):
        raise SetupError("Proposal environment must allow only the main branch, without reviewers.")


def check_repository_copies():
    for kind in ("variables", "secrets"):
        entries = metadata(f"repos/{REPOSITORY}/actions/{kind}", kind)
        if {entry["name"] for entry in entries} & {VARIABLE, APP_SECRET, SSH_SECRET}:
            raise SetupError("Repository-level proposal credentials exist; reconcile before setup.")


def convert(store, code):
    store.save({"purpose": ENVIRONMENT, "phase": "exchanging"})
    response = api("POST", f"/app-manifests/{urllib.parse.quote(code, safe='')}/conversions")
    app = {
        key: response[key] for key in ("id", "client_id", "slug", "owner", "permissions", "events")
    }
    store.save({"purpose": ENVIRONMENT, "phase": "registered", "app": app, "pem": response["pem"]})
    check_app(app)


def installation_url(record):
    check_app(record["app"])
    return f"https://github.com/apps/{record['app']['slug']}/installations/new"


def register(store):
    record = load_record(store)
    if record.get("app"):
        print("Install only agent-cockpit: " + installation_url(record))
        return
    if record:
        raise SetupError("Registration interrupted; reconcile before registering another App.")
    if gh(["api", "user"])["login"] != OWNER:
        raise SetupError("Host GitHub CLI must be authenticated as Androlax2.")
    check_repository_copies()
    environments = metadata(f"repos/{REPOSITORY}/environments", "environments")
    if any(environment["name"] == ENVIRONMENT for environment in environments):
        variables, configured_secrets = configuration()
        if VARIABLE in variables or {APP_SECRET, SSH_SECRET} & configured_secrets.keys():
            raise SetupError(
                "Proposal credentials already exist; reconcile instead of registering."
            )
    with registration_server(
        store,
        make_manifest=manifest,
        exchange=convert,
        description=(
            "Signed in as Androlax2: register a private proposal App with contents read "
            "and pull-request write access. Install only agent-cockpit afterward."
        ),
    ) as server:
        store.save({"purpose": ENVIRONMENT, "phase": "waiting"})
        print(
            f"Open on this host: http://127.0.0.1:{server.server_port}/start/{server.state}",
            flush=True,
        )
        deadline = time.monotonic() + 3500
        while not server.consumed and time.monotonic() < deadline:
            server.handle_request()
        if not server.consumed or server.failed:
            raise SetupError("Registration incomplete; preserve private state and reconcile.")
    print("Install only agent-cockpit: " + installation_url(store.load()))


def verify_installation(record, directory):
    check_app(record["app"])
    jwt = sign_jwt(record, directory)
    current = api("GET", "/app", jwt)
    check_app(current)
    if any(current[key] != record["app"][key] for key in ("id", "client_id", "slug")):
        raise SetupError("Saved key authenticates a different App.")
    installations = api("GET", "/app/installations?per_page=2", jwt)
    if len(installations) != 1:
        raise SetupError("Exactly one proposal App installation is required.")
    installation = installations[0]
    if (
        installation["app_id"] != current["id"]
        or installation["account"]["login"] != OWNER
        or installation["repository_selection"] != "selected"
        or installation["suspended_at"]
        or installation["permissions"] != current["permissions"]
    ):
        raise SetupError("Proposal installation owner, permissions or selection is not approved.")
    token = api(
        "POST",
        f"/app/installations/{int(installation['id'])}/access_tokens",
        jwt,
        {"permissions": {"metadata": "read"}},
    )["token"]
    try:
        repositories = api("GET", "/installation/repositories?per_page=2", token)
        if repositories["total_count"] != 1 or [
            repo["full_name"] for repo in repositories["repositories"]
        ] != [REPOSITORY]:
            raise SetupError("Proposal installation must select only Androlax2/agent-cockpit.")
    finally:
        api("DELETE", "/installation/token", token)
    bot = current["slug"] + "[bot]"
    user = gh(["api", "users/" + urllib.parse.quote(bot, safe="")])
    if (
        user["login"] != bot
        or user["type"] != "Bot"
        or type(user["id"]) is not int
        or user["id"] <= 0
    ):
        raise SetupError("Proposal bot identity is not verified.")
    return {
        "app_id": current["id"],
        "client_id": current["client_id"],
        "slug": current["slug"],
        "bot": bot,
        "bot_id": user["id"],
        "installation_id": installation["id"],
    }


def verify(store):
    record = load_record(store)
    print(json.dumps(verify_installation(record, store.directory), sort_keys=True))


def check_deploy_key(record):
    keys = metadata(f"repos/{REPOSITORY}/keys")
    matches = [key for key in keys if key["id"] == record["deploy_key_id"]]
    if len(matches) != 1 or any(
        matches[0][field] != expected
        for field, expected in (
            ("key", record["ssh_public"]),
            ("title", KEY_TITLE),
            ("read_only", False),
        )
    ):
        raise SetupError("Recorded deploy key no longer matches; reconcile without replacement.")


def deploy_key(store):
    record = load_record(store)
    verify_installation(record, store.directory)
    if record.get("deploy_key_id"):
        check_deploy_key(record)
        print("Recorded Git-only deploy key verified; no write performed.")
        return
    keys = metadata(f"repos/{REPOSITORY}/keys")
    if any(key["title"] == KEY_TITLE or key["key"] == record.get("ssh_public") for key in keys):
        raise SetupError(
            "Proposal deploy key already exists; reconcile without another registration."
        )
    if not record.get("ssh_private"):
        with tempfile.TemporaryDirectory(prefix=".ssh-", dir=store.directory) as temporary:
            key_path = Path(temporary) / "key"
            result = subprocess.run(
                ["ssh-keygen", "-t", "ed25519", "-N", "", "-C", "", "-f", str(key_path)],
                capture_output=True,
                timeout=30,
                check=False,
            )
            if result.returncode:
                raise SetupError("SSH key generation failed; no raw output displayed.")
            record.update(
                ssh_private=key_path.read_text(),
                ssh_public=key_path.with_suffix(".pub").read_text().strip(),
            )
            store.save(record)
    record["phase"] = "registering-key"
    store.save(record)
    # gh api infers POST from --input; unlike --method, the shared transport returns its JSON.
    response = gh(
        ["api", f"repos/{REPOSITORY}/keys", "--input", "-"],
        json.dumps({"title": KEY_TITLE, "key": record["ssh_public"], "read_only": False}).encode(),
    )
    if type(response["id"]) is not int or response["id"] <= 0:
        raise SetupError("Deploy-key response lacks a valid ID; reconcile privately.")
    record["deploy_key_id"] = response["id"]
    check_deploy_key(record)
    record["phase"] = "registered"
    store.save(record)
    print(
        f"Registered Git-only deploy key ID {record['deploy_key_id']}; private key retained privately."
    )


def configure(store):
    record = load_record(store)
    verify_installation(record, store.directory)
    check_environment()
    check_repository_copies()
    check_deploy_key(record)
    variables, configured_secrets = configuration()
    variable = variables.get(VARIABLE)
    if variable and variable["value"] != record["app"]["client_id"]:
        raise SetupError("Existing proposal client ID differs; refusing replacement.")
    written = record.get("secrets", {})
    for name in (APP_SECRET, SSH_SECRET):
        current = configured_secrets.get(name)
        if name in written:
            if not current or current["updated_at"] != written[name]:
                raise SetupError("Proposal secret metadata changed; reconcile without replacement.")
        elif current:
            raise SetupError("Proposal secret already exists; refusing replacement.")
    if record["phase"] == "configured":
        if not variable or set(written) != {APP_SECRET, SSH_SECRET}:
            raise SetupError("Configured proposal metadata is incomplete; reconcile privately.")
        print("Proposal scope and existing configuration verified; no writes performed.")
        return
    if not variable:
        record.update(phase="uploading", pending=VARIABLE)
        store.save(record)
        gh(
            ["api", "--method", "POST", f"{ENV_PATH}/variables", "--input", "-"],
            json.dumps({"name": VARIABLE, "value": record["app"]["client_id"]}).encode(),
        )
        variables, _ = configuration()
        if variables[VARIABLE]["value"] != record["app"]["client_id"]:
            raise SetupError("Proposal variable readback differs; reconcile privately.")
        record.update(phase="registered", pending=None)
        store.save(record)
    for name, value in ((APP_SECRET, record["pem"]), (SSH_SECRET, record["ssh_private"])):
        if name in written:
            continue
        record.update(phase="uploading", pending=name)
        store.save(record)
        gh(["secret", "set", name, "--repo", REPOSITORY, "--env", ENVIRONMENT], value.encode())
        _, configured_secrets = configuration()
        written[name] = configured_secrets[name]["updated_at"]
        record.update(phase="registered", pending=None, secrets=written)
        store.save(record)
    record["phase"] = "configured"
    store.save(record)
    print("Proposal credentials stored only in release-proposal; release credentials untouched.")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("register", "verify", "deploy-key", "configure"))
    parser.add_argument(
        "--state-dir", type=Path, default=Path.home() / ".local/state/cockpit-proposal-app"
    )
    args = parser.parse_args(argv)
    try:
        with Store(args.state_dir) as store:
            {
                "register": register,
                "verify": verify,
                "deploy-key": deploy_key,
                "configure": configure,
            }[args.command](store)
    except SetupError as error:
        print(f"Proposal setup: {error}", file=sys.stderr)
        return 1
    except (
        OSError,
        ValueError,
        KeyError,
        TypeError,
        subprocess.SubprocessError,
        KeyboardInterrupt,
    ):
        print(
            "Proposal setup failed or interrupted; preserve private state and reconcile before retrying.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
