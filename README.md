# Torlek

[![Documentation (latest)](https://img.shields.io/badge/docs-latest-lightgrey)](https://androlax2.github.io/agent-cockpit/latest/)
[![OpenSSF Scorecard](https://api.scorecard.dev/projects/github.com/Androlax2/agent-cockpit/badge)](https://scorecard.dev/viewer/?uri=github.com/Androlax2/agent-cockpit)

Torlek runs your coding agents (Claude Code, Codex, OpenCode) side by side, each in its own terminal session, and lets an agent ask you to run commands on your machine.
One screen shows every agent, its questions and a todo list per project.

**Alpha 1, Linux and Windows through WSL2.** Torlek may lose its data between versions: keep your work in Git.
The sandbox is off by default, so agents run with your rights (see [Known gaps](https://github.com/Androlax2/agent-cockpit/blob/main/docs/KNOWN-GAPS.md)).

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
4. Run "torlek daemon install", then "torlek init".
5. Follow what "torlek init" says, one step at a time. Ask me before each step
   that changes my settings. Two things are mine to do, so tell me what to do
   and wait: writing sandbox = "off" under [agents] in my Torlek config file
   (that is my consent to run agents without a sandbox), and signing in to
   Claude, Codex or OpenCode.
6. When "torlek init" has nothing left that you can do, tell me in plain words
   what you did and what is left for me.
```

## Install from the command line

You need Linux or WSL2 (x86_64 or aarch64), `git`, `curl`, `tmux` 3.6 or newer, and a systemd user session.
On Windows, open your WSL2 Linux from Windows Terminal and run everything there.
Ubuntu 24.04 ships tmux 3.4: install a newer tmux first.

1. Install the newest alpha with the install script, as your own user (not root):

   ```sh
   curl -fsSL https://github.com/Androlax2/agent-cockpit/releases/download/v0.1.0-alpha.5/install.sh | sh
   ```

   The script ships from v0.1.0-alpha.5 on: until that release is published, the URL does not exist yet, so use the commands by hand below.
   It downloads your machine's release archive and its `SHA256SUMS`, checks the archive before unpacking it into `~/.local/lib/torlek`, and links `~/.local/bin/torlek` to it.
   It never uses sudo and leaves your Torlek settings, data and daemon alone. Run it again to upgrade; `| sh -s -- --version v0.1.0-alpha.5` installs one release.
   [Install script](docs/packaging/install-script.md) says what it checks and what it does not.

   To read the script before it runs, download it with the release's checksums, check it, read it, then run it:

   ```sh
   cd "$(mktemp -d)"
   curl -fLO https://github.com/Androlax2/agent-cockpit/releases/download/v0.1.0-alpha.5/install.sh
   curl -fLO https://github.com/Androlax2/agent-cockpit/releases/download/v0.1.0-alpha.5/SHA256SUMS
   sha256sum --check --ignore-missing SHA256SUMS
   less install.sh
   sh install.sh
   ```

   Or by hand: download the release, check it and unpack it into `~/.local/lib/torlek`:

   ```sh
   version=$(curl -fsSL "https://api.github.com/repos/Androlax2/agent-cockpit/releases?per_page=1" | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p')
   archive="torlek-v${version:?GitHub named no release}-linux-$(uname -m)"
   cd "$(mktemp -d)"
   curl -fLO "https://github.com/Androlax2/agent-cockpit/releases/download/v$version/$archive.tar.gz"
   curl -fLO "https://github.com/Androlax2/agent-cockpit/releases/download/v$version/SHA256SUMS"
   sha256sum --check --ignore-missing SHA256SUMS &&
     mkdir -p ~/.local/lib/torlek ~/.local/bin &&
     tar -xzf "$archive.tar.gz" -C ~/.local/lib/torlek &&
     ln -sfn ~/.local/lib/torlek/"$archive"/bin/torlek ~/.local/bin/torlek
   ```

   `sha256sum` must print `OK`; if it does not, nothing is installed. If `torlek` is not found afterwards, add `~/.local/bin` to your `PATH`.

2. Set it up:

   ```sh
   torlek daemon install
   torlek init
   ```

   `torlek init` changes nothing. It lists what is done and what is left, and ends with the one thing to do next.
   Do that, then run `torlek init` again.

   This release has no sandbox: agents run with your rights, so Torlek's grants do not bind them.
   One step asks you to write `sandbox = "off"` under `[agents]` in your Torlek config file.
   Write it yourself: it records that you accept running agents unsandboxed.
   Add `desktop_session = "withheld"` on the next line to keep agents away from your screen and desktop session.
   Without it, agents can use them.

Packages for Arch Linux (AUR), Debian and Ubuntu (APT) and Windows (Scoop) are prepared but not published yet.
Until they are, use the archive above on every system.

## Next

Read the [quickstart](https://github.com/Androlax2/agent-cockpit/blob/main/docs/QUICKSTART.md) to start your first agent. It takes five minutes.

The [user guide](https://github.com/Androlax2/agent-cockpit/blob/main/docs/USER-GUIDE.md) has the details: providers, settings and where Torlek keeps its files.
Users of the older desktop setup will find it in the [legacy guide](docs/legacy/README.md).
To contribute, read [Contributing](CONTRIBUTING.md). Report security problems through [Security](SECURITY.md).
