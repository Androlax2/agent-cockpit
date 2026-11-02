#!/usr/bin/env python3
"""Collect repository protection metadata with GET requests; never apply settings."""

import argparse
import json
import re
import subprocess
from datetime import UTC, datetime
from time import monotonic

REPOSITORY = "Androlax2/agent-cockpit"
PREFIX = f"repos/{REPOSITORY}"

QUERIES = (
    ("repository", ""),
    ("rulesets", "/rulesets?per_page=100"),
    ("main_rules", "/rules/branches/main?per_page=100"),
    ("classic_main_protection", "/branches/main/protection"),
    ("branches", "/branches?per_page=100"),
    ("actions", "/actions/permissions"),
    ("workflow_permissions", "/actions/permissions/workflow"),
    ("fork_approval", "/actions/permissions/fork-pr-contributor-approval"),
    ("environments", "/environments?per_page=100"),
    ("release_environment", "/environments/release"),
    ("release_ref_policy", "/environments/release/deployment-branch-policies?per_page=100"),
    ("private_reporting", "/private-vulnerability-reporting"),
    ("dependabot_alerts", "/vulnerability-alerts"),
    ("dependabot_updates", "/automated-security-fixes"),
    ("codeql_setup", "/code-scanning/default-setup"),
    ("scanning_analyses", "/code-scanning/analyses?per_page=100"),
    ("workflows", "/actions/workflows?per_page=100"),
    ("repository_secrets", "/actions/secrets?per_page=100"),
    ("release_secrets", "/environments/release/secrets?per_page=100"),
    ("release_app_client_id", "/actions/variables/RELEASE_APP_CLIENT_ID"),
)


def project_metadata(endpoint, pages):
    if endpoint == PREFIX:
        fields = (
            "full_name",
            "visibility",
            "default_branch",
            "allow_squash_merge",
            "allow_merge_commit",
            "allow_rebase_merge",
            "security_and_analysis",
        )
        return [{key: page[key] for key in fields if key in page} for page in pages]
    if "/secrets?" in endpoint:
        return [
            {
                "total_count": page["total_count"],
                "secrets": [
                    {
                        key: secret[key]
                        for key in ("name", "created_at", "updated_at")
                        if key in secret
                    }
                    for secret in page["secrets"]
                ],
            }
            for page in pages
        ]
    if "/variables/" in endpoint:
        return [
            {key: page[key] for key in ("name", "value", "created_at", "updated_at") if key in page}
            for page in pages
        ]
    return pages


def collect(endpoint):
    observation = {"endpoint": endpoint, "state": "unknown", "pages": []}
    deadline = monotonic() + 60
    page_number = 1
    while True:
        remaining = deadline - monotonic()
        if remaining <= 0:
            return {**observation, "reason": "gh timed out after 60 seconds"}
        separator = "&" if "?" in endpoint else "?"
        page_endpoint = endpoint if page_number == 1 else f"{endpoint}{separator}page={page_number}"
        try:
            # Fetch pages separately so a failed response cannot erase or contaminate prior pages.
            result = subprocess.run(
                [
                    "gh",
                    "api",
                    page_endpoint,
                    "--hostname",
                    "github.com",
                    "--method",
                    "GET",
                    "--include",
                ],
                capture_output=True,
                text=True,
                check=False,
                timeout=remaining,
            )
        except FileNotFoundError:
            return {**observation, "reason": "gh unavailable"}
        except subprocess.TimeoutExpired:
            return {**observation, "reason": "gh timed out after 60 seconds"}
        except OSError:
            return {**observation, "reason": "gh could not be executed"}
        if result.returncode:
            # Raw CLI errors can contain private transport details; retain only HTTP status.
            status = re.search(r"\(HTTP (\d{3})\)", result.stderr)
            reason = f"HTTP {status[1]}" if status else f"gh exited {result.returncode}"
            return {**observation, "reason": reason}
        headers, boundary, body = result.stdout.replace("\r\n", "\n").partition("\n\n")
        status = re.match(r"HTTP/\S+ (\d{3})\b", headers)
        if not boundary or not status or status[1] not in ("200", "204"):
            return {**observation, "reason": "unexpected HTTP response from gh"}
        if status[1] == "204":
            return {**observation, "state": "observed"}
        try:
            page = json.loads(body)
        except json.JSONDecodeError:
            return {**observation, "reason": "invalid JSON from gh"}
        if not isinstance(page, (dict, list)):
            return {**observation, "reason": "unexpected page shape from gh"}
        try:
            observation["pages"].extend(project_metadata(endpoint, [page]))
        except (KeyError, TypeError):
            return {**observation, "reason": "unexpected metadata shape from gh"}
        links = re.search(r"^link:\s*(.+)$", headers, re.IGNORECASE | re.MULTILINE)
        if not links or not re.search(r';\s*rel="next"', links[1]):
            return {**observation, "state": "observed"}
        # Only advance this fixed endpoint; never follow a response-supplied host or path.
        page_number += 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.parse_args()
    observations = {name: collect(PREFIX + suffix) for name, suffix in QUERIES}
    rulesets = observations["rulesets"]
    for page in rulesets["pages"]:
        if not isinstance(page, list) or any(
            not isinstance(rule, dict) or type(rule.get("id")) is not int or rule["id"] <= 0
            for rule in page
        ):
            rulesets["state"] = "unknown"
            rulesets["reason"] = "invalid ruleset listing"
            continue
        for rule in page:
            identifier = rule["id"]
            observations[f"ruleset:{identifier}"] = collect(f"{PREFIX}/rulesets/{identifier}")
    complete = all(observation["state"] == "observed" for observation in observations.values())
    print(
        json.dumps(
            {
                "repository": REPOSITORY,
                "collected_at": datetime.now(UTC).isoformat(),
                "collection_complete": complete,
                "meaning": "Metadata only; observed does not mean enabled, secure or enforced.",
                "observations": observations,
            },
            indent=2,
        )
    )
    return 0 if complete else 1


if __name__ == "__main__":
    raise SystemExit(main())
