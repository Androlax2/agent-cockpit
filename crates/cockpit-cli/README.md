# cockpit-cli

Owns the `cockpit` command line: it parses verbs and renders their results.

The installed command is `torlek`. The internal package description above is retained until #570.

**Depends on:** `cockpit-client` and `cockpit-protocol` for daemon verbs, and
the capability crates whose verbs must work without the daemon, listed in
[report 02](../../docs/design/greenfield/02-crate-architecture.md) (#77). The
`cli -> client/protocol/offline capabilities only` rule of
`cockpit-workspace-rules/tests/crate_graph.rs` fails when the CLI reaches any other product crate.
**Used by:** nothing; it is a frontend.

## Verbs

| Verb | Works without the daemon | Owner |
| --- | --- | --- |
| `torlek config show [--project DIR] [--set KEY=VALUE]...` | yes | `cockpit-config` |
| `torlek config reload` (a running daemon applies the owner's changed session settings, #354) | no | `cockpit-sessions` |
| `torlek project add DIR [--name NAME]`, `torlek project list` | no | `cockpit-projects` |
| `torlek checkout list [PROJECT]` | no | `cockpit-projects` |
| `torlek checkout clone PROJECT BRANCH`, `torlek checkout review PROJECT PULL_REQUEST` | no | `cockpit-projects` |
| `torlek checkout claim CHECKOUT HOLDER`, `torlek checkout release CHECKOUT HOLDER` | no | `cockpit-projects` |
| `torlek checkout sweep [--dry-run]`, `torlek checkout push CHECKOUT` | no | `cockpit-projects` |
| `torlek storage`: registered clone `target/` apparent bytes; read-only, owner-only; exits 1 for incomplete, unavailable or truncated results | no | `cockpit-projects` |
| `torlek storage prune`: preview/prepare, explicitly confirm an immutable plan, inspect its status, or abandon inactive cleanup while retaining all files; Linux mount proof and owner policy required; no whole-clone cleanup | no | `cockpit-projects` |
| `torlek ask run [--cwd DIR] [--no-wait] --reason TEXT COMMAND`, `--wait ID`, `--check` (agent socket) | no | `cockpit-approvals` |
| `torlek ask guide [--cwd DIR]` (steps on standard input), `--wait ID` (agent socket) | no | `cockpit-approvals` |
| `torlek approval list`, `show ID`, `approve ID --revision R --digest D`, `decline ID`, `edit ID --revision R COMMAND`, `input ID` (the line typed comes from standard input) | no | `cockpit-approvals` |
| `torlek guide show ID`, `answer ID STEP [ANSWER]`, `stuck ID STEP NOTE`, `close ID` | no | `cockpit-approvals` |
| `torlek init`, and `torlek` alone until the setup is done: the first-run checklist (host readiness, sandbox and approvals, provider, sign-in, project, first session), each step `done`, `todo`, `waits` or `unknown` with its next action, and the one action to take now on the last line; writes nothing; `torlek` alone prints the usage once every step is done or `unknown` (#100) | partly: without a daemon it reads the configuration and names the daemon as the first action | `cockpit-cli`, from the daemon's readiness report, projects and sessions |
| `torlek doctor [--project DIR]`: the active profile, its directories, daemon, tmux server and unit, and every other installation seen (#196, #134); then one line per requirement with its status (report 09) and a `next:` line when it is not available; exits 1 only when one is `missing-prerequisite` or `unsupported` (#50) | partly: without a daemon it reads the configuration and names the daemon as missing | `cockpit-cli` for the installations, `cockpit-readiness`, served by the daemon, for the requirements |
| `torlek version [--all]`: version and source revision; `--all` adds the profile, the build and the release bundle's manifest (#96, #196) | yes | `cockpit-cli` |
| `torlek daemon install`: the active profile's daemon as a systemd user unit or launchd agent, started now; refuses the default profile beside a legacy Cockpit (#134) | yes | `cockpit-cli` |
| `torlek session new DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH]`, `list [--ended]`, `restart ID\|--all`, `sleep\|wake\|resume\|end ID`, `send ID MESSAGE`, `cancel ID`, `end --finished` (every session the task list says finished, #157), `link ID URL` | no | `cockpit-sessions` |
| `torlek session grant ID observe-sessions\|message-sessions\|start-sessions [--per-hour N] [--without-confirmation]`, `revoke ID PERMISSION`, `grants ID` (the grants and the audit, newest first), #410 | no | `cockpit-authorization`, through the daemon |
| `torlek agent sessions [--watch]`, `message ID TEXT`, `start DIR [--prompt TEXT] [--clone BRANCH]` (agent socket, each under its coordinator grant; a confirmed start waits like `ask run`), #410 | no | the daemon's coordinator kit |
| `torlek session show ID` (in the manager's viewer, waking it when asleep) | no | the daemon's manager session |
| `torlek provider list` (the names `--agent` takes, whether each resumes and reports `events`) | no | the daemon's wiring |
| `torlek tui` (opens the manager session with the `torlek-tui` installed beside `torlek`, then attaches to it) | no | the daemon's manager session |
| `torlek todo add TITLE [--project P] [--notes TEXT] [--kind KIND] [--priority 1-6] [--after ID]...`, `list [--project P] [--all]`, `show ID`, `start ID`, `done ID [--summary TEXT]`, `delete ID` (agent socket in a launched session, else the owner's) | no | `cockpit-tasks` |
| `torlek todo propose TITLE --why TEXT [--project P]`, `blocked TITLE --why TEXT` (agent socket) | no | `cockpit-tasks` |
| `torlek workflow presets`, `list [--project P] [--all]`, `show ID`, `packet ID`, `complete ID --changed T --checked T --outcome T [--needs-human T] [--evidence REF]... [--waive REQUIREMENT]...`, `release ID [handoff flags]`, `block ID ACTION --detail TEXT`, `unblock ID` (agent socket in a launched session, else the owner's) | no | `cockpit-workflows` |
| `torlek workflow begin ID`, `checkpoint ID NOTE` (agent socket); `start ITEM [--preset NAME]`, `cancel ID` (the owner's) | no | `cockpit-workflows` |
| `torlek check run CHECK --project P [--checkout PATH]` (the owner's: prints the run's output and receipt, exits 0 only on a pass), `gate [--project P] [--checkout PATH]` (one line per check: status, receipt, next action; exits 0 only when the gate passed), `claim CHECK STATEMENT` (agent socket; recorded unverified) | no | `cockpit-verification` |
| `torlek queue explain [ITEM]` (why each open item waits), `dispatch` (a scheduling pass now) | no | `cockpit-scheduling` |
| `torlek --skill` (the whole agent manual in one read), `torlek docs [TOPIC]` (the topics, or one), `torlek brief [--measure]` (a launched session's launch brief; `--measure` adds the context budget, agent socket) | no | `cockpit-agent-context`, served by the daemon |
| `torlek mcp` (an MCP server on standard input and output: the topics as resources, the `torlek_docs` and `torlek_brief` tools) | no | `cockpit-agent-context`, served by the daemon |
| `torlek todo edit ID [--revision R] [--title T] [--notes TEXT] [--kind KIND]`, `priority ID LEVEL`, `queue`/`unqueue`/`approve`/`decline`/`reopen ID`, `after`/`unwait ID ON...`, `image add ID FILE`, `image get ID NUMBER FILE`, `draft save (ID \| --new PROJECT)`, `drafts`, `draft discard` | no | `cockpit-tasks` |

`torlek todo` (and `docs`, `brief`, `mcp`, `--skill`) picks its socket from `TORLEK_LAUNCH_TOKEN`: a launched session's verbs go to
the agent socket, where the daemon identifies it; without the token they are the owner's.
`show` ends with `next:`, what has to happen for the item to move.

Every verb serves one profile (#196): `--profile NAME` before the verb, else the command's name
(`torlek-next` is profile `next`), else `$TORLEK_PROFILE`, else the build's own: `torlek` for a
release bundle, `dev` for any other build. A development build reaches `torlek` only through
`--profile torlek`; an inherited `TORLEK_PROFILE=torlek` is ignored, and `config show` and
`doctor` say so. The profile names every directory (`torlek-<name>`), the socket and the tmux
server.

Usage errors exit 2; configuration errors exit 1 with the file and line at fault. A daemon
verb's refusal exits 1 as `torlek: <code>: <message>`.

`torlek ask run` and `torlek ask guide` keep `ask-to-run`'s exit codes: the command's own
once it ran, 125 declined or the guide closed, 124 still pending after
`TORLEK_ASK_WAIT_SECONDS` (default 540; `--wait ID` waits on), 126 the daemon unreachable
(with the fix), 3 stuck on a guide step, 1 any other refusal. `--no-wait` files the request
and exits 124 at once, so the agent goes on working and collects the answer with `--wait ID`.
`approval show` lists the kinds of hidden characters escaped, the changed build files the
command runs and the `timeout` hint; `approval list` folds a long command behind an explicit
"more". The owner's verbs print commands,
directories and reasons under the daemon's display rules, and count guide steps from 1.
