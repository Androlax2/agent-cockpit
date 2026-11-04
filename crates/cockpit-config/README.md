# cockpit-config

Owns the effective configuration and where each value came from: the profile, XDG layout, owner
settings, precedence, provenance and start-file change detection. Design:
[report 09](../../docs/design/greenfield/09-project-setup-readiness.md).

**Depends on:** no product crate (only `serde` and `toml`).
**Used by:** `cockpit-daemon` for paths and settings, `cockpit-sessions` for start-file
snapshots (one per launch, #146), `cockpit-readiness` for the project's requirements (#50), and `cockpit-cli` for `torlek config show` and an offline `torlek doctor`. It reads files; it never
grants: owner settings are inputs to `cockpit-authorization`, which decides.

## Files and precedence

| Layer (lowest first) | Location | May set |
| --- | --- | --- |
| Product defaults | this crate | everything |
| Owner config | `$XDG_CONFIG_HOME/torlek/config.toml` (default `~/.config/torlek/config.toml`) | everything |
| Project config | `<project>/.torlek/config.toml` | describing settings only (`workflow.preset`, `[[checks]]`, `[execution]`, `[needs]`, `[results]`) |
| Overrides | `--set key=value` (TOML value) | `projects.trusted`, `workflow.preset` |

A project file that sets an owner setting (`projects`, `start_files`, `tasks`, `sessions`, `storage`, `memory`) fails with its file and
line: repository configuration describes requirements, owner settings grant. An owner file
that declares `[[checks]]` fails the same way: a check describes a project (#313); each needs
a `name` and a `command`, and a name declared twice fails at its second line. The project's
requirements fail the same way in the owner file (#50): `[execution] environment` (`native`,
`sandbox` or `docker`), `[needs] cpu_millis` and `memory_mib` (at least one), and
`[results] artifacts` (paths relative to the project, never `..`). A check's
`working_directory` follows the same rule, and its `environment` is `native`, `sandbox` or
`docker`. `torlek doctor` reports
whether this host meets them; the configuration never grants them. Unknown keys,
relative paths and unknown providers fail the same way. State and cache go under
`$XDG_STATE_HOME/torlek` and `$XDG_CACHE_HOME/torlek`; the runtime directory is
`$XDG_RUNTIME_DIR/torlek` and shown as unset when that variable is.

```sh
torlek config show [--project DIR] [--set KEY=VALUE]...
```

prints every value as `key = value  # source`. It reads files only, so it works without the
daemon, tmux or a provider.

## Tasks

Owner settings for what sessions file (`cockpit-tasks`, #38):

```toml
[tasks]
blocker_project = "dotfiles"   # where blocker reports go; unset refuses them
cross_project_open_max = 3     # open items one project may file in another's list
```

## Sessions

How long an agent may wait at its prompt before its session sleeps, and how long a session may
sleep before it ends. The daemon reads both at start; a value below the minimum stops it.

```toml
[sessions]
idle_sleep_after_minutes = 60  # at least 30
asleep_expiry_days = 7         # at least 1
```

## Memory

What the daemon and its private tmux server may hold in memory (#406). Owner settings, read at
the daemon's start; a value outside its range stops it.

```toml
[memory]
tmux_history_lines = 10000      # 200 to 1000000: history lines each pane keeps
feed_buffer_kib = 1024          # 64 to 1048576: change batches waiting for one slow subscriber
agentless_session_minutes = 10  # at least 1: a running session with no agent is reported after
```

The daemon applies `tmux_history_lines` to its tmux server at each start. Sessions opened
afterwards are bound by it; tmux 3.8 also trims the history of the sessions already open, at
once. Measured with tmux 3.8, a pane full of coloured 200-column output holds about 6 MiB at
2,000 lines, 25 MiB at 10,000 and 116 MiB at 50,000.

`feed_buffer_kib` is what one viewer that reads slowly, or stopped reading, makes the daemon
keep for it; the changes it missed reach it later in fewer, merged batches.
`agentless_session_minutes` is how long a session may stay running after its agent exited,
kept alive by processes the agent started, before it shows as waiting on the owner with the
command that ends it. Nothing is ended automatically.

## Start files

Files an agent reads only at launch. The product declares the provider-owned ones
(`CLAUDE.md`, Claude `settings` and `plugins`, `opencode config`, `codex hooks`); the owner adds
installation-specific ones, and an entry whose label matches a default replaces it:

```toml
[[start_files]]
label = "launcher"
path = "~/.local/bin/greywall-agents/claude"
providers = ["claude"]
```

`StartFileSnapshot::capture(files, provider)` records each file's path, modification time,
ctime, inode and size when a session launches; `snapshot.changed(files)` names those that
differ now, including a file replaced by an older copy, deleted, created, repointed, or
declared after launch.
