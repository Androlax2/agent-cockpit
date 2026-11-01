# cockpit-config

Owns the effective configuration and where each value came from: XDG layout, owner settings,
precedence, provenance and start-file change detection. Design:
[report 09](../../docs/design/greenfield/09-project-setup-readiness.md).

**Depends on:** no product crate (only `serde` and `toml`).
**Used by:** `cockpit-daemon` for paths and settings, `cockpit-sessions` for start-file
snapshots (one per launch, #146), `cockpit-readiness` for the project's requirements (#50), and `cockpit-cli` for `cockpit config show` and an offline `cockpit doctor`. It reads files; it never
grants: owner settings are inputs to `cockpit-authorization`, which decides.

## Files and precedence

| Layer (lowest first) | Location | May set |
| --- | --- | --- |
| Product defaults | this crate | everything |
| Owner config | `$XDG_CONFIG_HOME/cockpit/config.toml` (default `~/.config/cockpit/config.toml`) | everything |
| Project config | `<project>/.cockpit/config.toml` | describing settings only (`workflow.preset`, `[[checks]]`, `[execution]`, `[needs]`, `[results]`) |
| Overrides | `--set key=value` (TOML value) | `projects.trusted`, `workflow.preset` |

A project file that sets an owner setting (`projects`, `start_files`, `tasks`, `sessions`) fails with its file and
line: repository configuration describes requirements, owner settings grant. An owner file
that declares `[[checks]]` fails the same way: a check describes a project (#313); each needs
a `name` and a `command`, and a name declared twice fails at its second line. The project's
requirements fail the same way in the owner file (#50): `[execution] environment` (`native`,
`sandbox` or `docker`), `[needs] cpu_millis` and `memory_mib` (at least one), and
`[results] artifacts` (paths relative to the project, never `..`). `cockpit doctor` reports
whether this host meets them; the configuration never grants them. Unknown keys,
relative paths and unknown providers fail the same way. State and cache go under
`$XDG_STATE_HOME/cockpit` and `$XDG_CACHE_HOME/cockpit`; the runtime directory is
`$XDG_RUNTIME_DIR/cockpit` and shown as unset when that variable is.

```sh
cockpit config show [--project DIR] [--set KEY=VALUE]...
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
