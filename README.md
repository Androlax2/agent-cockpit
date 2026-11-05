# Torlek

[![Documentation (latest)](https://img.shields.io/badge/docs-latest-lightgrey)](https://androlax2.github.io/agent-cockpit/latest/)

Torlek runs your coding agents (Claude Code, Codex, OpenCode) side by side, each in its own terminal session.
Every agent works inside a sandbox and must ask you before it runs anything outside it.
One screen shows all your agents, their questions and a todo list per project.

This is an early test version (Alpha 1) for Linux and for Windows through WSL2.
It may lose its own data between versions, so keep your work in Git.

## Install with your AI agent

Open Claude Code, Codex or OpenCode in a terminal and paste this:

```text
Install Torlek on this machine and set it up for me.
Use only the version and commands in this README, under "Install from the command line":
https://github.com/Androlax2/agent-cockpit/blob/main/README.md
Do not guess other commands, packages or download links.

1. Check that this is Linux or WSL2 on x86_64 or aarch64, and that git, curl,
   tmux 3.6 or newer and a systemd user session are present. Tell me what is missing.
2. Before any command that uses sudo or the system package manager, show me the
   exact command, say why it is needed, and wait for my yes.
3. Download the release archive, check its checksum, unpack it and link torlek
   into ~/.local/bin, exactly as the README shows.
4. Run "torlek sandbox install", then "torlek daemon install", then "torlek init".
5. Follow what "torlek init" says, one step at a time. Ask me before each step
   that changes my settings. Two things are mine to do, so tell me what to do
   and wait: writing the sandbox setting in my Torlek config file (that is my
   consent), and signing in to Claude, Codex or OpenCode.
6. When "torlek init" has nothing left that you can do, tell me in plain words
   what you did and what is left for me.
```

## Install from the command line

You need Linux or WSL2 (x86_64 or aarch64), `git`, `curl`, `tmux` 3.6 or newer, and a systemd user session.
On Windows, open your WSL2 Linux from Windows Terminal and run everything there.
Ubuntu 24.04 ships tmux 3.4: install a newer tmux first.

1. Download the release, check it and unpack it into `~/.local/lib/torlek`:

   ```sh
   version=$(curl -fsSL "https://api.github.com/repos/Androlax2/agent-cockpit/releases?per_page=1" | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p')
   archive="torlek-v$version-linux-$(uname -m)"
   cd "$(mktemp -d)"
   curl -fLO "https://github.com/Androlax2/agent-cockpit/releases/download/v$version/$archive.tar.gz"
   curl -fLO "https://github.com/Androlax2/agent-cockpit/releases/download/v$version/SHA256SUMS"
   sha256sum --check --ignore-missing SHA256SUMS &&
     mkdir -p ~/.local/lib/torlek ~/.local/bin &&
     tar -xzf "$archive.tar.gz" -C ~/.local/lib/torlek &&
     ln -sfn ~/.local/lib/torlek/"$archive"/bin/torlek ~/.local/bin/torlek
   ```

   The first line asks GitHub for the newest release; in a release archive's own README it names that release.
   `sha256sum` must print `OK`; if it does not, nothing is installed. If `torlek` is not found afterwards, add `~/.local/bin` to your `PATH`.

2. Set it up:

   ```sh
   torlek sandbox install
   torlek daemon install
   torlek init
   ```

   `torlek init` changes nothing. It lists what is done and what is left, and ends with the one thing to do next.
   Do that, then run `torlek init` again.

   One step asks you to write `sandbox = "required"` under `[agents]` in your Torlek config file.
   Write it yourself: it records that you chose the sandbox.
   Add `desktop_session = "withheld"` on the next line to keep agents away from your screen and desktop session.
   Without it, agents can use them.

Packages for Arch Linux (AUR), Debian and Ubuntu (APT) and Windows (Scoop) are prepared but not published yet.
Until they are, use the archive above on every system.

## Next

Read the [quickstart](https://github.com/Androlax2/agent-cockpit/blob/main/docs/QUICKSTART.md) to start your first agent. It takes five minutes.

The [user guide](https://github.com/Androlax2/agent-cockpit/blob/main/docs/USER-GUIDE.md) has the details: providers, settings and where Torlek keeps its files.
Users of the older desktop setup will find it in the [legacy guide](docs/legacy/README.md).
To contribute, read [Contributing](CONTRIBUTING.md). Report security problems through [Security](SECURITY.md).
