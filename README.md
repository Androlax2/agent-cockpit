# Agent Cockpit

Agent Cockpit runs your coding agents in terminal sessions and keeps their work, questions and approvals together.
One installation belongs to one person, with a CLI and terminal UI backed by a per-user daemon and tmux.
Agents work inside a sandbox; you decide which requests may run outside it.

The next release is **Alpha 1**, being built under `crates/` beside the running legacy product.
Alpha state may reset between releases; keep your work in Git and do not rely on alpha state migrating to beta.
Existing desktop users can find their commands and demo screenshots in the [legacy operator guide](docs/legacy/README.md).

## Install status

Public installation is still being prepared in [#97](https://github.com/Androlax2/agent-cockpit/issues/97).
The pinned Greywall/greyproxy engine installer is tracked in [#470](https://github.com/Androlax2/agent-cockpit/issues/470).
Until those land, there is no complete public clean-install recipe here.
The root `make install` installs the legacy desktop setup; it is not the Alpha installer.

Developers can build the current greenfield binaries with the pinned Rust toolchain:

```sh
git clone https://github.com/Androlax2/agent-cockpit.git
cd agent-cockpit
cargo build --workspace --locked -j 2
export PATH="$PWD/target/debug:$PATH"
cockpit doctor
```

See [Development](docs/DEVELOPMENT.md) for build prerequisites and checks.
`doctor` names missing prerequisites and their next action; building alone does not install the sandbox engine or sign in to a provider.
The quickstart below starts once those prerequisites are ready.

## Platforms and prerequisites

| Platform | Target | Current evidence and limits |
| --- | --- | --- |
| Linux | Native | Linux CI runs the greenfield gate; public archive onboarding is an Alpha release checkpoint |
| macOS | Native | Platform checks run in CI for selected crates; the service is a launchd agent; public archive onboarding remains a release checkpoint |
| Windows | Inside WSL2, from Windows Terminal | Linux backend inside WSL2; platform checks run in CI for selected crates; no native Windows daemon |

You need Git, tmux, a supported provider CLI, and the Cockpit-pinned Greywall/greyproxy engines.
Linux service installation uses a systemd user manager; macOS uses launchd.
Run `cockpit doctor --project /absolute/path/to/project` to check the actual host and project requirements.
The platform checks are described in [Development](docs/DEVELOPMENT.md#verification-policy); they do not establish a signed-in provider conversation on every platform.

## First project and session

Run these commands in your own terminal. For a source build, the default profile is `dev`; release bundles default to `cockpit`.
`cockpit config show` and `cockpit doctor` show which profile and paths you are using.
Use the same profile throughout. To keep a preview beside an existing installation, use `cockpit --profile next VERB` for every command, including `daemon install`.

1. Start the daemon as your user service, then read the first-run checklist:

   ```sh
   cockpit daemon install
   cockpit doctor
   cockpit init
   ```

   `daemon install` writes and starts a user service. `init` writes nothing: it shows `done`, `todo`, `waits` or `unknown` and ends with the next action.
   Repeat it after completing that action. An `unknown` sign-in needs the manual check it names.

2. Open the owner configuration path printed by `cockpit config show` and add the following settings, preserving any existing settings:

   ```toml
   [agents]
   sandbox = "required"
   desktop_session = "withheld"
   ```

   This records your sandbox choice and keeps agents off your desktop session.
   If the sandbox cannot enforce its rules, launching fails with the reason.
   The default desktop setting is `shared`, which exposes your display and filtered session bus to agents; choose it only if you want desktop access.
   Run `cockpit config reload` after changing session settings on a running daemon.

3. Choose a provider and follow `cockpit init`'s sign-in instruction in your terminal:

   ```sh
   cockpit provider list
   cockpit init
   ```

   A local sign-in check does not prove your account has model access or remaining quota.
   Provider accounts and usage are managed by the provider.

4. Register a Git repository. This small example creates a new, empty project:

   ```sh
   mkdir -p "$HOME/cockpit-example"
   git -C "$HOME/cockpit-example" init
   cockpit project add "$HOME/cockpit-example" --name example
   cockpit project list
   ```

   For existing work, pass the repository's top-level directory instead.

5. Start a session, choosing an installed provider name from the list:

   ```sh
   cockpit session new "$HOME/cockpit-example" --agent claude --prompt 'Describe this repository without changing files.'
   cockpit session list
   cockpit tui
   ```

   Replace `claude` with `codex` or `opencode` if that is your provider.
   In the TUI, use its help for available keys. From the CLI, `cockpit session show ID` opens a session; replace `ID` with the identifier from `session list`.
   `cockpit session end ID` stops it. `cockpit init` shows when the first-session step is complete.

## Approvals

Inside a launched session, an agent can request an outside-sandbox command with
`cockpit ask run --reason 'why this needs the owner' 'command'`, or human steps with `cockpit ask guide`.
A request does not authorize itself. Repository instructions and agent messages cannot grant host access.

Review requests in the TUI approval pane or in your terminal:

```sh
cockpit approval list
cockpit approval show ID
```

Read the exact command, directory, reason and content it runs. To approve, use
`cockpit approval approve ID --revision REVISION --digest DIGEST` with the revision and digest shown by `approval show`.
An edit requires another review; stale approval details are refused.
Use `cockpit approval decline ID` to decline. Approval runs that command once with the daemon's host rights.
Provider-native permission dialogs are separate and remain in the provider terminal.

## Providers and capability evidence

`cockpit provider list` shows installed provider names and whether each offers resume and events.
These capabilities depend on the release's hook/plugin bridges being present.
The following table describes the adapters in this checkout, not a promise that every installed provider version or account works.

| Provider name | Adapter capabilities with its bridge | Capability pin and provenance |
| --- | --- | --- |
| `claude` (Claude Code) | Resume, events/outcomes, submit/cancel, effort, compact, remote control, screen fallback | [2.1.316 adapter and fixture evidence](crates/cockpit-claude/README.md) |
| `codex` (Codex) | Resume, events/outcomes, submit/cancel through a private app-server | [0.161.0 capability pin](crates/cockpit-codex/README.md); local sign-in has a separate 0.166.0 conformance pin |
| `opencode` (OpenCode) | Resume, events/outcomes, submit/cancel, native commands | [1.19.5 adapter and fixture evidence](crates/cockpit-opencode/README.md); sign-in remains discovered/unverified |

Adapters label a capability `tested` only at their exact capability pin; other or unknown versions are `discovered` (unverified).
Fixture tests and help/flag checks do not establish a live signed-in end-to-end conversation.
In particular, Codex delivered-turn approvals and in-session resume/fork still have unmeasured behavior; see its adapter's known gaps.
Receiving a prompt is not evidence that an agent completed its task.

## Configuration and state

`cockpit config show` works without the daemon and prints effective values with their sources.
`cockpit config show --project /absolute/path/to/project` includes project settings.
Owner configuration grants policy inputs; a repository's `.cockpit/config.toml` describes workflow, checks and requirements and cannot grant owner permissions.
Unknown keys fail with the file and line. See the [current configuration guide](crates/cockpit-config/README.md) for supported settings and precedence.

| Data | Default profile location |
| --- | --- |
| Owner settings | `~/.config/cockpit/config.toml` |
| Database, session records and instruction bundle | `~/.local/state/cockpit/` |
| Cockpit operational logs | `~/.local/state/cockpit/logs/` |
| Cache | `~/.cache/cockpit/` |
| Sockets and instance lock | `$XDG_RUNTIME_DIR/cockpit/`, or the state directory's `run/` when unset |

`XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME` override their respective base directories.
Named profiles replace the `cockpit` directory component with `cockpit-NAME` (for example `cockpit-dev`).
Use the paths printed by your binary; keep private state and provider credentials out of Git and support messages.

## Help and support

Start with `cockpit doctor`, `cockpit version --all` and `cockpit init`.
The [CLI guide](crates/cockpit-cli/README.md) lists current verbs.
`cockpit docs` lists the installed agent instruction topics; `cockpit docs TOPIC` reads one and `cockpit --skill` prints the full agent manual through the daemon.
Generated CLI/config references and the protocol schema site are tracked in [#102](https://github.com/Androlax2/agent-cockpit/issues/102) and [#295](https://github.com/Androlax2/agent-cockpit/issues/295).

For a Cockpit problem, create and inspect a local bug-report draft:

```sh
cockpit bug-report new 'Describe what happened' --steps 'Describe how to reproduce it'
cockpit bug-report list
cockpit bug-report show ID
```

Each draft includes a support bundle. Inspect its listed contents before sharing it.
Cockpit's diagnostic logs exclude hosted prompts, agent output, shell commands, credentials and environment values; your own description may still contain private details.
`cockpit bug-report send ID` opens a prefilled GitHub issue in your browser; you review and submit it, then attach the bundle yourself.
Nothing is filed automatically. There is no standalone `cockpit support-bundle` CLI verb yet.
If the daemon is unavailable, use [GitHub issues](https://github.com/Androlax2/agent-cockpit/issues) with your version, platform and reproduction steps, excluding secrets.
Report security problems through [Security](SECURITY.md).

To contribute, read [Contributing](CONTRIBUTING.md). Architecture proposals and decisions live in the [greenfield design reports](docs/design/greenfield/README.md) and [decision records](docs/decisions/README.md).
