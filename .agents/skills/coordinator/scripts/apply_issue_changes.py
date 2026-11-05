"""Post a decision report's proposed changes as one comment on each linked issue.

The report holds a `## Changes for each linked issue` section: an optional introduction, then one
`### #NUMBER title` subsection per issue (blockquote markers are stripped), then an optional
`### Not changed` subsection. Without --go this prints what it would post and posts nothing.

Usage: apply_issue_changes.py REPORT --decision-issue N --summary TEXT [--repo OWNER/NAME]
                              [--only ISSUE...] [--go]
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

SECTION_HEADING = "## Changes for each linked issue"
SECTION_END = re.compile(r"^(### Not changed|## )", re.MULTILINE)
ISSUE_HEADING = re.compile(r"^### #(\d+)\b[^\n]*\n", re.MULTILINE)
REPORT_ONLY_SENTENCE = "Proposed text; the owner applies it. Nothing here edits an issue. "


def run_text(command, cwd=None):
    return subprocess.run(
        command, capture_output=True, text=True, check=True, cwd=cwd
    ).stdout.strip()


def changes_section(report_text):
    if SECTION_HEADING not in report_text:
        sys.exit(f"apply_issue_changes: the report has no '{SECTION_HEADING}' section")
    section = report_text.split(SECTION_HEADING, 1)[1]
    end = SECTION_END.search(section)
    return section[: end.start()] if end else section


def split_changes(section):
    """Returns (introduction, [(issue number, body)])."""
    parts = ISSUE_HEADING.split(section)
    introduction = parts[0].strip().replace(REPORT_ONLY_SENTENCE, "")
    changes = []
    for number, body in zip(parts[1::2], parts[2::2], strict=True):
        changes.append((number, re.sub(r"^> ?", "", body.strip(), flags=re.MULTILINE)))
    return introduction, changes


def report_link(report, repo):
    directory = report.resolve().parent
    root = Path(run_text(["git", "rev-parse", "--show-toplevel"], cwd=directory))
    return f"https://github.com/{repo}/blob/main/{report.resolve().relative_to(root)}"


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("report", type=Path)
    parser.add_argument("--decision-issue", required=True, help="the decision issue's number")
    parser.add_argument(
        "--summary", required=True, help="who decided what, e.g. 'owner, 2026-11-05: Q1 A, Q2 B'"
    )
    parser.add_argument(
        "--repo", help="OWNER/NAME; default: the repository of the current directory"
    )
    parser.add_argument(
        "--only", nargs="+", help="post only these issue numbers (resume after a failure)"
    )
    parser.add_argument("--go", action="store_true", help="post the comments")
    arguments = parser.parse_args(argv)

    repo = arguments.repo or run_text(
        ["gh", "repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner"]
    )
    introduction, changes = split_changes(changes_section(arguments.report.read_text()))
    if not changes:
        sys.exit("apply_issue_changes: no '### #NUMBER' subsections under the changes section")
    link = report_link(arguments.report, repo)
    for number, body in changes:
        if arguments.only and number not in arguments.only:
            continue
        message = (
            f"Decided in #{arguments.decision_issue} ({arguments.summary}; [report]({link})). "
            f"{introduction}\n\nProposed change to this issue:\n\n{body}\n"
        )
        print(f"#{number}: {len(body)} characters")
        if not arguments.go:
            print(message)
            continue
        subprocess.run(
            ["gh", "issue", "comment", number, "-R", repo, "--body", message], check=True
        )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
