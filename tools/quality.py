"""Run read-only language checks on tracked source, including extensionless scripts."""

import argparse
import json
import re
import shlex
import subprocess
import sys
from pathlib import Path

EXTENSIONS = {
    ".py": "python",
    ".sh": "shell",
    ".fish": "fish",
    ".js": "js",
    ".mjs": "js",
    ".cjs": "js",
    ".ts": "js",
    ".tsx": "js",
    ".jsx": "js",
    ".md": "markdown",
    ".lua": "lua",
}


def interpreter(path):
    with path.open("rb") as source:
        first = source.readline(4096).decode("utf-8", errors="replace")
    if not first.startswith("#!/"):
        return None
    words = shlex.split(first[2:])
    command = Path(words[0]).name
    if command == "env":
        command = next(
            (word for word in words[1:] if not word.startswith("-") and "=" not in word),
            "",
        )
    return command


def discover(language):
    tracked = subprocess.check_output(["git", "ls-files", "-z"]).decode().split("\0")
    selected = []
    for name in sorted(set(tracked) - {""}):
        path = Path(name)
        if path.is_symlink() or not path.is_file():
            continue
        if language == "typos":
            selected.append(f"./{name}")
            continue
        command = interpreter(path)
        if command and re.fullmatch(r"python[\d.]*", command):
            kind = "python"
        elif command in ("sh", "bash", "dash", "ksh"):
            kind = "shell"
        elif command == "fish":
            kind = "fish"
        else:
            kind = EXTENSIONS.get(path.suffix)
        if kind == language:
            selected.append(f"./{name}")
    return selected


def check(language, paths):
    if not paths:
        print(f"{language}: no tracked source files")
        return
    if language == "python":
        commands = [["ruff", "check", *paths], ["ruff", "format", "--check", *paths]]
    elif language == "shell":
        commands = []
        for path in paths:
            source = Path(path)
            directive = re.search(r"^#\s*shellcheck shell=(\w+)", source.read_text(), re.MULTILINE)
            dialect = interpreter(source) or (directive[1] if directive else "sh")
            shell = "sh" if dialect == "dash" else dialect
            commands.append(["shellcheck", f"--shell={shell}", path])
            formatting = "posix" if shell == "sh" else "mksh" if shell == "ksh" else "bash"
            commands.append(["shfmt", "-d", "-i", "4", "-ln", formatting, path])
    elif language == "fish":
        commands = [["fish", "--no-execute", path] for path in paths]
        commands.append(["fish_indent", "--check", *paths])
    elif language == "js":
        commands = [["biome", "check", *paths]]
    elif language == "markdown":
        commands = [["markdownlint-cli2", *paths]]
    elif language == "links":
        commands = [
            ["lychee", "--offline", "--include-fragments=anchor-only", "--no-progress", *paths]
        ]
    elif language == "typos":
        commands = [["typos", "--force-exclude", "--config", ".config/typos.toml", *paths]]
    else:
        commands = [["luac", "-p", path] for path in paths]
    for command in commands:
        print(shlex.join(command), flush=True)
        subprocess.run(command, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("language", choices=sorted(set(EXTENSIONS.values()) | {"links", "typos"}))
    parser.add_argument("--list", action="store_true", help="print source paths as JSON")
    arguments = parser.parse_args()
    try:
        paths = discover("markdown" if arguments.language == "links" else arguments.language)
        if arguments.list:
            print(json.dumps(paths))
        else:
            check(arguments.language, paths)
    except FileNotFoundError as error:
        print(f"quality: missing tool: {error.filename}", file=sys.stderr)
        return 127
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
