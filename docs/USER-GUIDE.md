# Torlek user guide

The [README](../README.md) installs Torlek and the [quickstart](QUICKSTART.md) walks through a first session.
This guide is the detailed reference behind them.

Torlek runs your coding agents in terminal sessions and keeps their work, questions and approvals together.
One installation belongs to one person, with a CLI and terminal UI backed by a per-user daemon and tmux.
This release has no sandbox: agents run with your rights until Torlek's own sandbox ([#546](https://github.com/Androlax2/agent-cockpit/issues/546)) is live. You still decide which requests an agent asks you to run.

This is **Alpha 1 for Linux and WSL2**, built under `crates/` beside the running legacy product.
Alpha state may reset between releases; keep your work in Git and do not rely on alpha state migrating to beta.
Existing desktop users can find their commands and demo screenshots in the [legacy operator guide](legacy/README.md).

## Install status

Install a release archive as the [README](../README.md#install-from-the-command-line) shows.
Alpha 1 uses archive installation; other packaging (#563/#565) is deferred to Alpha 2. The Arch AUR recipe (#562) is prepared in Alpha 1 beside the archive, not as its prerequisite ([decision](decisions/2026-11-06-562-committed-aur-recipe.md), [recipe](packaging/aur.md)). macOS delivery and Homebrew are deferred beyond Alpha 1 under [#568](https://github.com/Androlax2/agent-cockpit/issues/568), with no restoration milestone assigned.
The root `make install` installs the legacy desktop setup; it is not the Alpha installer.

Developers can build the current greenfield binaries with the pinned Rust toolchain:

```sh
git clone https://github.com/Androlax2/agent-cockpit.git
cd agent-cockpit
cargo build --workspace --locked -j 2
export PATH="$PWD/target/debug:$PATH"
torlek doctor
```

See [Development](DEVELOPMENT.md) for build prerequisites and checks.
`doctor` names missing prerequisites and their next action; building alone does not sign in to a provider.
The steps below start once those prerequisites are ready.

## Platforms and prerequisites

| Platform | Target | Current evidence and limits |
| --- | --- | --- |
| Linux | Native | Linux CI runs the greenfield gate; public archive onboarding is an Alpha release checkpoint |
| macOS | Deferred beyond Alpha 1 | No Alpha 1 download, Homebrew package or macOS acceptance is promised. [#568](https://github.com/Androlax2/agent-cockpit/issues/568) tracks restoration, including unresolved [public CA isolation (#527)](https://github.com/Androlax2/agent-cockpit/issues/527); existing tests do not establish supported installation |
| Windows | Inside WSL2, from Windows Terminal | Linux backend inside WSL2; platform checks run in CI for selected crates; no native Windows daemon |

You need Git, tmux **3.6 or newer**, curl and a supported provider CLI.
Ubuntu 24.04 (including WSL2) ships tmux 3.4: upgrade tmux before using Torlek.
An existing private tmux server also needs the supported version; upgrading its binary alone does not replace it.
See [tmux prerequisites](DEVELOPMENT.md#tmux-prerequisite) for checks and upgrade guidance.
Linux/WSL2 service installation requires a systemd user manager.
Run `torlek doctor --project /absolute/path/to/project` to check the actual host and project requirements.
The platform checks are described in [Development](DEVELOPMENT.md#verification-policy); they do not establish a signed-in provider conversation on every platform.

## First project and session

The [quickstart](QUICKSTART.md) walks through your first project and session. This section holds the settings behind it.
Run `torlek daemon install` and `torlek init` first, as the [README](../README.md#install-from-the-command-line) shows, make the settings below, then follow the quickstart.

A source build uses the profile `dev` and release bundles use `torlek`; `torlek config show` and `torlek doctor` show which profile and paths you are using.
Use the same profile for every command. To keep a preview's state apart from an existing installation, put `--profile next` before every command, including `daemon install`.

1. Accept running agents without a sandbox. Add this to the owner configuration file (`torlek config show` prints its path), keeping any existing settings:

   ```toml
   [agents]
   sandbox = "off"
   desktop_session = "withheld"
   ```

   `sandbox = "off"` records that you accept this: an unsandboxed agent runs with your rights, can reach the owner socket and act as you, so Torlek's grants do not bind it. The daemon logs that risk at its start and at every launch.
   `desktop_session = "withheld"` keeps agents off your screen and desktop session; the default, `shared`, exposes your display and filtered session bus.
   To refuse every unsandboxed launch instead, see Sandbox mode in [Configuration and state](USER-GUIDE.md#configuration-and-state).
   If the daemon is already running, run `torlek config reload`. Until then it keeps the `agents.sandbox` it last read, and `torlek doctor` blocks on its `sandbox` line, naming both values.

2. Sign in to a provider. Run `torlek provider list` to see the installed names, then `torlek init` and follow its sign-in instruction in your terminal. A sign-in marked `unknown` needs the manual check `init` names.
   A local sign-in check does not prove your account has model access or remaining quota.
   Provider accounts and usage are managed by the provider.

3. Choose a model and effort for a Claude session. Pass `--model MODEL` (an alias such as `opus` or a model id) and `--effort LEVEL` (`low`, `medium`, `high`, `xhigh` or `max`) to `torlek session new`.
   Every restart, wake and resume of the session keeps them. Without them, Claude Code's own settings apply. A provider that cannot set them refuses the start.
   `--compaction-threshold VALUE` sets the harness's own compaction threshold, off by default, in the harness's words: Claude Code `auto` or a context window such as `200k` (`--autocompact`), Codex a whole percent such as `75` (`model_post_turn_compact_threshold_percent`), OpenCode a token count such as `20000` (`compaction.reserved`). The session card shows it. An installed harness version without the option refuses the start with the missing flag or setting.

## Approvals

Inside a launched session, an agent can request a command it needs you to run with
`torlek ask run --reason 'why this needs the owner' 'command'`, or human steps with `torlek ask guide`.
A request does not authorize itself. Repository instructions and agent messages cannot grant host access.

Run owner commands (`approval`, `project add`, `session end`, `session grant`…) from your own terminal, not from an agent's pane: inside a session (Claude Code's `!` included) `torlek` sends every command on the agent socket, where they are refused.

Review requests in the TUI approval pane or in your terminal:

```sh
torlek approval list
torlek approval show ID
```

Read the exact command, directory, reason and content it runs. To approve, use
`torlek approval approve ID --revision REVISION --digest DIGEST` with the revision and digest shown by `approval show`.
An edit requires another review; stale approval details are refused.
Use `torlek approval decline ID` to decline. Approval runs that command once with the daemon's host rights.
Provider-native permission dialogs are separate and remain in the provider terminal.

A running command that waits at a prompt shows in the queue. Type into it with `torlek approval input ID`, which reads one line from standard input, or `i` in the TUI pane.
The line is typed only while the command's terminal hides what is typed, as at a `sudo`, `ssh` or `gpg` password prompt (echo off, the line read whole); otherwise it is refused, because an echoed line lands in the output the agent reads and Torlek stores.
To answer a visible prompt (`y`, `[Y/n]`, `read`) on purpose, use `torlek approval input ID --shown`, or Tab in the TUI to show the line before Enter; that answer appears in the output.

To remove a finished run's stored output, run `torlek approval purge-output ID`; database retention does the same after `[storage] database_retention_days`.
The daily database backups still hold it until they age out (`[storage] database_backups_kept`, 7 by default, in the state directory's `backups` folder); delete the copies taken after the run to remove it sooner.
The cleared text can stay in the database file's unused pages until SQLite reuses them, and what the agent already received stays in its own conversation.

### Network requests

Under `agents.sandbox = "required"`, a host no entry allows is refused at once (`403`) and becomes a request for you (#674).
It shows in the TUI approval pane and in `torlek approval list`; `torlek network list` lists only these:

```sh
torlek network list
torlek network allow ID            # for the rest of that session
torlek network allow ID --always   # for every session of the project, until revoked
torlek network deny ID             # refused at once from now on, without asking again
torlek network grants [PROJECT]
torlek network revoke GRANT        # closes the tunnels it carried
```

In the pane, `y` allows once, `a` allows always and `x` denies.
"Allow once" lasts the rest of the asking session, not one connection; a new session asks again.
"Allow always" covers only that host and port. Only names are asked about: IP addresses and names that resolve to private or loopback addresses stay refused.

## Phone status page

`torlek status serve` shows your sessions on your phone, read-only: each session's project, state, label, what it is doing and its open question (flagged), the approvals waiting for you (reason and age, never the command), the daemon's readiness with the low-disk guard, and when the page was generated, so a stalled daemon shows.
It refreshes itself every 15 s; `/status.json` returns the same data.
Nothing on it approves, answers or changes anything: decide from the TUI or your terminal.
What agents wrote (label, doing, question, reason) shows as written, so whatever they put in it shows too.
It is an owner verb: a session cannot start it, but it has no login: any process on this machine and anyone you share it with on your tailnet can read it.

It listens on `127.0.0.1` only (port 8850 unless you pass `--port N`). Reach it from your phone through [Tailscale Serve](https://tailscale.com/kb/1242/tailscale-serve), which keeps it inside your tailnet over HTTPS:

1. Run `tailscale serve status` to read your machine's tailnet name, such as `box.tailnet.ts.net`.
2. Start the page with that name: `torlek status serve --host box.tailnet.ts.net`.
3. In another terminal, run `sudo tailscale serve --bg --https=443 http://127.0.0.1:8850`.
4. On your phone, with Tailscale connected, open `https://box.tailnet.ts.net/`.

A default Tailscale install on Linux refuses `serve` for a regular user, hence `sudo`; skip it only if you are already Tailscale's operator. This guide does not recommend `sudo tailscale set --operator=$USER`: it lets any process running as you change Tailscale's config, including `tailscale funnel`, which publishes to the internet, and that matters most while agents run unsandboxed (`agents.sandbox = "off"`).

The page refuses any request whose `Host` is not `127.0.0.1:PORT`, `localhost:PORT` or the `--host` name (bare or with `:443`), which blocks DNS rebinding. Tailscale Serve forwards your phone's `Host` unchanged, hence `--host`; it takes one exact name, no wildcard.
Never expose the page with `tailscale funnel`, which publishes it to the internet. Stop sharing it with `sudo tailscale serve --https=443 http://127.0.0.1:8850 off`.

## Providers and capability evidence

`torlek provider list` shows installed provider names and whether each offers resume and events.
These capabilities depend on the release's hook/plugin bridges being present.
The following table describes the adapters in this checkout, not a promise that every installed provider version or account works.

| Provider name | Adapter capabilities with its bridge | Capability pin and provenance |
| --- | --- | --- |
| `claude` (Claude Code) | Resume, events/outcomes, submit/cancel, effort, compact, remote control, screen fallback | [2.1.316 adapter and fixture evidence](../crates/cockpit-claude/README.md) |
| `codex` (Codex) | Resume, events/outcomes, submit/cancel through a private app-server | [0.161.0 capability pin](../crates/cockpit-codex/README.md); local sign-in has a separate 0.166.0 conformance pin |
| `opencode` (OpenCode) | Resume, events/outcomes, submit/cancel, native commands | [1.19.5 adapter and fixture evidence](../crates/cockpit-opencode/README.md); sign-in remains discovered/unverified |

Adapters label a capability `tested` only at their exact capability pin; other or unknown versions are `discovered` (unverified).
Fixture tests and help/flag checks do not establish a live signed-in end-to-end conversation.
Codex delivered-turn approval routing has local fake-model evidence on 0.166.0; signed-in acceptance, the older 0.161.0 target and in-session resume/fork remain unmeasured. See its adapter's known gaps.
Receiving a prompt is not evidence that an agent completed its task.

## Configuration and state

`torlek config show` works without the daemon and prints effective values with their sources.
`torlek config show --project /absolute/path/to/project` includes project settings.
Owner configuration grants policy inputs; a repository's `.torlek/config.toml` describes workflow, checks and requirements and cannot grant owner permissions.
Unknown keys fail with the file and line. See the [current configuration guide](../crates/cockpit-config/README.md) for supported settings and precedence.

| Data | Default profile location |
| --- | --- |
| Owner settings | `~/.config/torlek/config.toml` |
| A project's GitHub token, its unsandboxed sessions' `GH_TOKEN` | `~/.config/torlek/github-tokens/<project>` |
| A project's GitHub API token, which the session proxy adds for sandboxed sessions | `~/.config/torlek/github-api-tokens/<project>` |
| A provider's credential, which the session proxy adds for sandboxed sessions | `~/.config/torlek/provider-credentials/<provider>/<VARIABLE>`; another account's in `<provider>/<account>/<VARIABLE>` |
| A login account's sign-in, which only `torlek account sign-in` writes | `~/.local/state/torlek/account-logins/<provider>/<account>/login/` |
| Database, session records and instruction bundle | `~/.local/state/torlek/` |
| Torlek operational logs | `~/.local/state/torlek/logs/` |
| Each session's temporary files (its `TMPDIR`), removed when the session ends | `~/.local/state/torlek/tmp/<session>/` |
| Cache | `~/.cache/torlek/` |
| Update choices and the last release check (`update.json`) | `~/.local/state/torlek/` |
| Sockets and instance lock | `$XDG_RUNTIME_DIR/torlek/`, or the state directory's `run/` when unset |

`torlek storage` lists each session's temporary folder and its size, then one row per area
(build folders, session temp folders, check outputs, database, backups, logs) with its apparent
bytes, what its prune would retire now and the command that frees it, and a total. It deletes
nothing; an area it could not read in full says why and makes it exit 1.

`torlek usage quota` shows each provider account's plan windows as the daemon last read
them: the share used, the state (`available`, `low` from your lowest alert threshold,
`exhausted`, `blocked`, or `stale` when the reading is old, a refresh failed after it or
its reset has passed), when each resets,
and the last refresh that failed. The daemon reads them every `usage.quota_refresh_minutes`;
Codex is read from its own sign-in in `~/.local/share/codex-quota`. When a window passes one of
`usage.quota_alert_percents` (80 and 95 by default), `torlek notify` tells you once (the
`usage` event).

The queue paces itself to these windows (`usage.pacing = "even"` by default): it starts
queued work on a session only while each window of the session's account is used below an
even line to its reset, the share of the window elapsed times 90 % plus 15 %, and never
past 90 %. The last 10 % (`usage.pacing_reserve_percent`) is kept for your own sessions;
the 15 % (`usage.pacing_burst_percent`) lets work start as a window opens. Low-priority
items get no burst, so they wait first; high-priority items ignore the line and stop only at
the reserve. Work already running is never paused. A held item shows `usage-paced` in
`torlek queue explain` with the window and about when it starts. `torlek usage quota` shows
each window's line for normal-priority items now (`queue pace 51%`; low-priority items wait
from the burst below it) and, while the window holds them, how long they wait (`queue waits
12 min (pace 60%)`); the TUI shows the same, one row per account above its status line. A
stale reading, a window scoped to one model and a session on an account Torlek reads no
windows for pace nothing.
`usage.pacing = "off"` starts queued work at full speed.

To keep one project from taking a whole window, give it a share in `[usage.project_shares]`
(`app = 30`, from 1 to 100). The queue then starts none of that project's work on an account
once the project has used about its share of one of the account's windows, whatever its
priority and even with `pacing = "off"`, until the window resets; `torlek queue explain`
names the project's share. The rest of the window stays free for every project, and a
project without a share has no cap. Providers report only the account's total, so a
project's part is estimated: the window's share used times what the project's replies since
the window opened cost at API prices (see `torlek usage cost`), over the account's. Replies
of a model with no price count for neither, so while one is in the window the projects with
priced replies read a larger part than they used. A share named for a project Torlek does not
know caps nothing. `torlek usage quota` and the TUI show the account's pace, not the
projects' shares. Your own interactive sessions are never capped: only queue starts wait.

`torlek usage cost [--days N]` shows what your replies cost at the provider's published API
prices (a plan is flat, so this is what the same tokens would bill on API credits): one line
per UTC day, provider, account and project over the last N days (7 by default, up to 366),
then the exact total. Torlek reads the replies from each harness's own records, in your home
and in each session's own home, about once a minute and counts each reply once. A reply of a
model Torlek has no price for is counted and named, never priced by guess. Only Claude Code is
read so far. A session that runs without the sandbox writes to your home, so its replies show
under `default`.

`torlek usage digest [--days N]` shows how your days went, one block per UTC day over the last N
days (7 by default, up to 366): the time your agents worked (sessions working at once count
once, and a gap under five minutes between turns counts as time), the sessions, turns and
approvals you granted, the cost above, one indented line per project, and the provider incidents
under way. A day with nothing says so. Torlek records a turn when it ends, so a turn still
running is not in the figure yet.

To let a project's agents push and open pull requests without your own `gh` login, create a
fine-grained token limited to that repository (contents and pull requests: read and write)
and save it alone in `~/.config/torlek/github-tokens/<project>` (`mkdir -p -m 700` the
folder, then `chmod 600` the file).
Its next sessions get it as `GH_TOKEN`; a sandbox would hide your `~/.config/gh` login from them, but this release runs agents unsandboxed, so they can read it.
A project without one gets no `GH_TOKEN`. `torlek project show PROJECT` says which, never the
token. A file whose mode is not `0600` or `0400`, or one that holds anything but the token, stops the
project's sessions from launching until you fix it.

Under `agents.sandbox = "required"` no session gets that token: it can push, and pushes go
through `torlek push`. Save a second token for the project in
`~/.config/torlek/github-api-tokens/<project>`, same rules, with pull requests and issues
read and write and contents read only. The session proxy adds it on `api.github.com`, and on
`github.com` for fetches; the agent holds a placeholder `GH_TOKEN` and trusts a per-session
CA through `SSL_CERT_FILE` and its siblings. It is added only on the GitHub hosts the session
reaches at launch, including one you allowed for the project (`torlek network allow ID
--always`) or for the session; a host first allowed while the session runs gets it at the
session's next launch.
`torlek session sandbox SESSION` names any host it is not added on.

`XDG_CONFIG_HOME`, `XDG_STATE_HOME` and `XDG_CACHE_HOME` override their respective base directories.
Named profiles replace the `torlek` directory component with `torlek-NAME` (for example `torlek-dev`).
Use the paths printed by your binary; keep private state and provider credentials out of Git and support messages.

### Sandbox mode

Where no sandbox backend exists, `sandbox = "required"` under `[agents]` refuses every agent launch and names the missing backend. It never runs an agent unsandboxed.

A sandboxed session never reads your own provider sign-in, such as `~/.claude/.credentials.json`.
Give each provider one account for its sandboxed sessions instead: save one credential alone in
`~/.config/torlek/provider-credentials/<provider>/<VARIABLE>`, the file named after the variable
the provider reads it from (`mkdir -p -m 700` the folder, then `chmod 600` the file). For Claude
that is an API key in `ANTHROPIC_API_KEY` or a `claude setup-token` token in
`CLAUDE_CODE_OAUTH_TOKEN`. The session proxy adds it on the provider's hosts; the agent holds a
placeholder. Two files for one provider, a mode other than `0600` or `0400`, or anything but the
credential in the file stop that provider's sandboxed sessions from launching. Without one,
`torlek session new` prints a `sign-in:` line and `torlek doctor` a `sandboxed sign-in` row
naming the files.

To give a provider several accounts, list them in the order sandboxed launches try them:

```toml
[accounts]
claude = ["work", "default"]
```

Each account other than `default` keeps its credential in a folder of its own,
`~/.config/torlek/provider-credentials/claude/work/<VARIABLE>`, by the same rules; `default`
is the file directly in the provider's folder, so a single credential keeps working without a
list. A launch runs on the first listed account that holds a credential and passes over one
that holds none; an unusable credential stops the launch, naming its account. The daemon
records the account each launch runs on (`torlek session show` prints `account:`); an
unsandboxed session signs in from your own home and runs on none. A restart, wake, resume
or restore keeps the session's account, since its conversation belongs to it; one whose account
is no longer listed or holds no credential is refused. `torlek doctor` has a
`sandboxed sign-in` row per account. The list is read when the daemon starts.

A provider sign-in that refreshes itself, a Claude `/login` or Codex's ChatGPT plan, is kept
as a login account; OpenCode 2 keeps its sign-ins in its database, so it has none yet. Sign a
listed account in once with
`torlek account sign-in codex work`: Torlek runs the provider's own sign-in in your terminal,
with a fresh folder of its own as its home and none of your provider variables, then moves only
the login file into `~/.local/state/torlek/account-logins/codex/work/login/`, unread. No agent
reads that folder, and your own sign-in is never copied, so it keeps its own tokens. Signing in
again replaces the login. A project's sessions may use a login account only through your grant:
`torlek account grant cockpit codex work` records it with when and through which client,
`torlek account grants [PROJECT]` lists the active ones and `torlek account revoke GRANT` takes
one back. A grant can come before the sign-in, and applies from the project's next launch.
Removing the project revokes its grants.

A sandboxed launch uses a signed-in login account the project holds a grant for like an
account with a credential, in the listed order. The session sees the login read-write where the
harness keeps it, so a sign-in the harness refreshes is the account's for its next session.
A login account the project holds no grant for is passed over for the next listed account,
and `session new` and `session show` print a `passed over:` line naming it and the
`torlek account grant` that lets it. A restart kept on a login whose grant you revoked is
refused, naming that grant. An account that holds both a credential file and a login refuses
the launch until you remove one. `torlek doctor` shows a signed-in login account's row as
available, naming the login file (never its contents) and the projects granted it; with no
grant it names the grant to give. It blocks on an account holding both, or on a login that is
no plain file, and an account holding neither offers the sign-in beside the credential file.

One session at a time uses a login account, so two sessions never refresh the same sign-in
at once and sign each other out. A launch passes over a login another starting, running or
stopping session uses, naming that session on its `passed over:` line; a session asleep,
failed or ended holds none, nor does a session on the account's credential file. Waking or
restarting a session kept on a login another session took meanwhile is refused, naming that
session. Of two sessions launching on one login at the same moment, the second is refused:
launch it again and it takes the next listed account.

When a session reports that its plan's usage limit stopped its turn (Codex does), Torlek
relaunches it at once on the next listed account that holds a credential and has not
quota-stopped in the last five hours, resuming the same conversation; `torlek session show`
then prints `account: spare (switched from work at a quota stop)`. Once its agent waits at
its prompt there, Torlek sends it one message: "Torlek: the plan usage limit that stopped your
last turn has reset. Continue where you stopped." New launches pass over an account for five
hours after it quota-stopped, unless no other account holds a credential. With no other
account to move to, the session stays where it is.

A session that stays quota-stopped gets the same one message when its plan's limit resets.
Torlek tells the reset from its own plan-window readings of your sign-in: a reading after the
stop must show the limit spent, then a later one room. A session asleep by then is woken
first; sessions holding a coordinator grant go before the others. `torlek session show`
prints the stop and where its resume stands, for example `quota stop: 2026-11-09 14:00:00
+00:00 on the owner's sign-in; resumes after the plan's reset at 2026-11-09 19:00:00 +00:00`. When the agent works again first, stops
again or its session ends, the stop is dropped and no message goes out. Torlek reads no plan
windows for a named account, nor for a provider without plan windows, so a stop there it
cannot move asks you to continue the session yourself, as does a stop whose windows show room,
a wake that fails, or a stop still waiting five hours on (an hour past the reset a reading said,
if later).

A limit the harness does not name as a spent plan can still be one. When a Claude Code
session's turns fail with `rate_limit` three times within 30 minutes, with no turn ending
otherwise in between, Torlek records the third as a quota stop and handles it as above.
`torlek session show` says where it came from, for example ``quota stop: … on the owner's
sign-in, inferred from 3 `rate_limit` failed turns within 30 minutes; …``, and the daemon
logs it. Codex names a spent plan itself. OpenCode reports every provider error as `APIError`, so
Torlek infers no quota stop from it.

Each sandboxed session has its own home, which starts empty and goes when the session ends.
For a project in `projects.trusted`, and its parallel clones, Torlek records the folder trust in
that home's Claude configuration before the agent starts, so Claude does not ask again; a project
not in it asks at each session. In every sandboxed session's home Torlek also marks Claude's
first-run onboarding done and, when an API key is bound, says yes to the API key question for
the placeholder the agent holds, so the session reaches its first turn with nobody at its
terminal.

### Optional skill pack

Torlek ships six skills from [mattpocock/skills](https://github.com/mattpocock/skills) (MIT),
pinned to one commit and off until you turn them on. Starters `diagnosing-bugs`, `tdd` and
`writing-for-agents` are on with the pack; `pr`, `codebase-design` and `wait-what` only when
you name them:

```toml
[skill_pack]
enabled = true
skills = { tdd = false, pr = true }
```

The daemon delivers them at its next start to Claude Code and OpenCode sessions, not Codex. A
skill of yours, or of a project in `projects.trusted`, with the same name wins for each
harness that reads its folder: Claude Code reads only `.claude/skills`. `torlek skills`
lists the pack's skills and their state beside the skills you have, collisions marked; the
pack never edits your skill folders. `torlek brief --measure` counts its bytes apart.

### Terminal screen

The TUI rings the terminal bell when a session finishes, asks a question or fails. Add `[tui]` with `attention_bell = false` to turn that off, then close the manager session and run `torlek tui` again.

On a terminal narrower than 60 columns, such as a phone over SSH, the TUI switches to a
one-column layout: requests and questions first, each with its whole command, then one line
per session. Enter opens a session full screen and read-only; `i` types a line into it, Esc
goes back. `[tui] phone_below_columns` in the owner configuration changes the width, and `0`
turns it off.

### Desktop notifications and sounds

`torlek notify` shows a desktop notification and plays a sound when a session fails, is held
on your permission, asks a question, stops at its usage limit or finishes. It also tells you
once when free disk space falls below the floor of `[storage] low_disk_floor_mib`, and when a
plan window passes one of your quota alert thresholds.

`torlek daemon install` also installs it as the user service `torlek-notify.service`
(`torlek-NAME-notify.service` for a named profile), tied to `graphical-session.target`: it
starts with your desktop session and stops with it. A host with no graphical session (WSL2, a
server, SSH only) never starts it and shows no error. `torlek doctor` says whether it is running,
installed but not running, or not installed, and the command that fixes it. A hand-made unit of
that name that runs `torlek notify` is replaced by the install, which says so. To not have the
service, set `autostart = false` in the owner configuration and run `torlek daemon install`
again, which removes it; `torlek notify` still runs by hand:

```toml
[notify]
autostart = false
```

Run by hand, it runs until you stop it. It needs `notify-send` for notifications and
`canberra-gtk-play`, `pw-play` or `paplay` for sounds; without them it keeps running and logs
which outlet is off. Turn single events off in the owner configuration, then start it again:

```toml
[notify.desktop]
finished = false    # no notification when a session finishes
[notify.sound]
question = false    # no sound for a question
```

Each event has a sound of Torlek's own (a kalimba for `finished`, marimbas for `question` and
`failure`, glass for `hold`, a soft falling sine for `quota`); a low disk plays your sound theme's `dialog-warning`, a
quota alert its `dialog-information` and a completed turn its `message`. To use your own sound for an event, name the file by its absolute path.
`.oga` and `.wav` are safe with every player; others depend on the one installed. A relative
path, an unknown event or a file that is missing or unreadable is refused when the configuration
loads, naming the key and the path:

```toml
[notify.sound.files]
question = "/home/me/sounds/question.wav"
```

`torlek notify test EVENT` plays that event's sound once, with your files and settings, and
exits with an error when it cannot. An agent's own terminal bell (Claude Code `terminal_bell`,
Codex `bel`) is a second sound the notifier does not control.

The events are `failure`, `hold`, `question`, `quota`, `disk`, `usage`, `finished` and `turn`. `disk`
is told when free space falls below the floor, once per fall: not again while it stays low, but
again when it rises above the floor and falls below it again. `usage` is told once per threshold
and window: when the five-hour window passes 80 %, again at 95 %, and again after it resets. `quota`
is a session stopped at its limit; `usage` warns before that, so you can turn it off apart. `finished` means the
session's task item is marked done; a session that exits by itself is a `failure`. A completed turn
alone is the `turn` event, which is off by default for the desktop and the sound; set `turn = true`
under `[notify.desktop]` or `[notify.sound]` to be told of every one.

The notifier writes one line to its standard error for every notice it tells or skips, with the
event, the session, what the desktop and the sound did and why one did not (a setting, a hold, a
missing program). Read them with `journalctl --user -u torlek-notify` when it runs as a service, to
find which event notifies you.

To keep it quiet for a while, `torlek notify away on`; `torlek notify away off` ends it, and so
does logging out where your desktop sets `XDG_RUNTIME_DIR`. Your own programs can hold it quiet as well: a program that exits 0
while you are in a meeting or playing, named in the owner configuration with an absolute path,
which the notifier runs every five seconds:

```toml
[notify.hold]
meeting = "/home/me/bin/in-a-meeting"
```

While a hold stands nothing shows and nothing plays; when the last one ends, one quiet
notification lists what still needs you.

While it runs, `torlek notify` also keeps a status file for your bar: the sessions, what each
needs you for, how many need you now and which holds stand. It sits in the profile's runtime directory
(`paths.runtime_dir` in `torlek config show`), usually `$XDG_RUNTIME_DIR/torlek/status.json`
(`torlek-NAME` for a named profile). Its schema is in the
[cockpit-notify README](../crates/cockpit-notify/README.md#status-file). The notifier rewrites
it every five seconds, so an older `written_at` means it stopped. For example, a waybar
`custom` module can show the count, and nothing once the notifier is gone, with:

```sh
jq -c --argjson now "$(date +%s)" '
  if $now - .written_at > 15 then {text: "", class: "gone"}
  else {text: "\(.needs_you)", class: (if .needs_you > 0 then "needs-you" else "idle" end)} end' \
  "$XDG_RUNTIME_DIR/torlek/status.json"
```

## Updates

Torlek tells you when a newer release is available. It never downloads, installs, restarts the daemon or stops a session for you: it prints the commands, and you run them.

- **When it checks.** At most once a day, when you run `torlek doctor` or `torlek session list` in a terminal, with a 3 second limit and the answer cached. Offline, rate-limited or failed checks print nothing and are not retried until the next day. Piped output, scripts and agent sessions never check or print a notice. A notice appears at most once a day.
- **What it offers.** The newest release of your channel or the next one (an alpha installation is offered the next beta; a beta installation is never offered an alpha), never an older one, a draft, or a release without an archive for your platform. A build that is not from a release archive has nothing to compare.
- **Your choices**, kept in `update.json` in the state directory:

  | Command | Effect |
  | --- | --- |
  | `torlek update` | Shows the cached release, its notes and the commands to update. Changes nothing. |
  | `torlek update later` | No reminders for 3 days. |
  | `torlek update skip` | No reminders about the offered version; a newer release is announced again. |
  | `torlek update checks off` / `on` | Turns automatic checks off or on. |
  | `torlek update check` | Asks the release list now, even with checks off. |

- **The update route.** For an installation unpacked as the README shows, `torlek update` prints the README's download, checksum and unpack commands for the new version, then `torlek daemon install`, which replaces the running daemon (stop, then start) while agent sessions keep running. Alpha releases may reset your data: read the release notes first.
- **After an update.** `torlek version --all` names the release the running daemon runs and each live session's bundle, so "installed, but this daemon and these sessions still run the previous release" reads as a normal, explained state. A daemon of another release than the installed one is told to stop and start; a session keeps its bundle until it is restarted.
- **Old sandbox services.** A profile used with Torlek's earlier Greywall and Greyproxy sandbox may still hold its `torlek-greyproxy-<profile>` service and state. `torlek doctor` lists them. Once no session that an earlier release launched in Greywall still runs, `torlek doctor --remove-old-sandbox` stops the service and deletes them.
- **Not supported yet.** An installation that is not a release archive unpacked in `~/.local/lib/torlek` as the README shows (a distribution package once #562, #563 and #565 publish them, an archive unpacked elsewhere, or a build from source) is told so, with no commands: update it the way you installed it. There is no macOS build (#568). The check needs `curl`, which the install route needs as well.

## Help and support

Start with `torlek doctor`, `torlek version --all` and `torlek init`.
The [CLI guide](../crates/cockpit-cli/README.md) lists current verbs.
`torlek docs` lists the installed agent instruction topics; `torlek docs TOPIC` reads one and `torlek --skill` prints the full agent manual through the daemon.
The [latest published user guide](https://androlax2.github.io/agent-cockpit/latest/) includes generated CLI help and effective configuration defaults, with source and binary provenance.
The generated protocol schema is still planned in [#295](https://github.com/Androlax2/agent-cockpit/issues/295).

For a Torlek problem, create and inspect a local bug-report draft:

```sh
torlek bug-report new 'Describe what happened' --steps 'Describe how to reproduce it'
torlek bug-report list
torlek bug-report show ID
```

Each draft includes a support bundle. Inspect its listed contents before sharing it.
Torlek's diagnostic logs exclude hosted prompts, agent output, shell commands, credentials and environment values; your own description may still contain private details.
`torlek bug-report send ID` opens a prefilled GitHub issue in your browser; you review and submit it, then attach the bundle yourself.
Torlek cannot see that you submitted it, so the draft is listed as opened until you record its issue: in the TUI press `B`, select the draft and press Enter, or run `torlek bug-report send ID --filed ISSUE_URL`. The issue's number works in place of its URL.
Nothing is filed automatically. There is no standalone `torlek support-bundle` CLI verb yet.
If the daemon is unavailable, use [GitHub issues](https://github.com/Androlax2/agent-cockpit/issues) with your version, platform and reproduction steps, excluding secrets.
Report security problems through [Security](../SECURITY.md).

To contribute, read [Contributing](../CONTRIBUTING.md). Architecture proposals and decisions live in the [greenfield design reports](design/greenfield/README.md) and [decision records](decisions/README.md).
