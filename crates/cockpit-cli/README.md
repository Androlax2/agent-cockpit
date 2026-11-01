# cockpit-cli

Owns the `cockpit` command line: it parses verbs and renders their results.

**Depends on:** `cockpit-client` and `cockpit-protocol` for daemon verbs, and
the capability crates whose verbs must work without the daemon, listed in
[report 02](../../docs/design/greenfield/02-crate-architecture.md) (#77). The
`cli -> client/protocol/offline capabilities only` rule of
`cockpit-workspace-rules/tests/crate_graph.rs` fails when the CLI reaches any other product crate.
**Used by:** nothing; it is a frontend.

## Verbs

| Verb | Works without the daemon | Owner |
| --- | --- | --- |
| `cockpit config show [--project DIR] [--set KEY=VALUE]...` | yes | `cockpit-config` |
| `cockpit config reload` (a running daemon applies the owner's changed session settings, #354) | no | `cockpit-sessions` |
| `cockpit project add DIR [--name NAME]`, `cockpit project list` | no | `cockpit-projects` |
| `cockpit checkout list [PROJECT]` | no | `cockpit-projects` |
| `cockpit checkout clone PROJECT BRANCH`, `cockpit checkout review PROJECT PULL_REQUEST` | no | `cockpit-projects` |
| `cockpit checkout claim CHECKOUT HOLDER`, `cockpit checkout release CHECKOUT HOLDER` | no | `cockpit-projects` |
| `cockpit checkout sweep [--dry-run]`, `cockpit checkout push CHECKOUT` | no | `cockpit-projects` |
| `cockpit ask run [--cwd DIR] [--no-wait] --reason TEXT COMMAND`, `--wait ID`, `--check` (agent socket) | no | `cockpit-approvals` |
| `cockpit ask guide [--cwd DIR]` (steps on standard input), `--wait ID` (agent socket) | no | `cockpit-approvals` |
| `cockpit approval list`, `show ID`, `approve ID --revision R --digest D`, `decline ID`, `edit ID --revision R COMMAND`, `input ID` (the line typed comes from standard input) | no | `cockpit-approvals` |
| `cockpit guide show ID`, `answer ID STEP [ANSWER]`, `stuck ID STEP NOTE`, `close ID` | no | `cockpit-approvals` |
| `cockpit version [--all]`: version and source revision; `--all` adds the release bundle's manifest (#96) | yes | `cockpit-cli` |
| `cockpit session new DIR [--agent NAME] [--prompt TEXT] [--clone BRANCH]`, `list [--ended]`, `restart ID\|--all`, `sleep\|wake\|resume\|end ID`, `end --finished` (every session the task list says finished, #157), `link ID URL` | no | `cockpit-sessions` |
| `cockpit session show ID` (in the manager's viewer, waking it when asleep) | no | the daemon's manager session |
| `cockpit provider list` (the names `--agent` takes, whether each resumes and reports `events`) | no | the daemon's wiring |
| `cockpit tui` (opens the manager session with the `cockpit-tui` installed beside `cockpit`, then attaches to it) | no | the daemon's manager session |
| `cockpit todo add TITLE [--project P] [--notes TEXT] [--kind KIND] [--priority 1-6] [--after ID]...`, `list [--project P] [--all]`, `show ID`, `start ID`, `done ID [--summary TEXT]`, `delete ID` (agent socket in a launched session, else the owner's) | no | `cockpit-tasks` |
| `cockpit todo propose TITLE --why TEXT [--project P]`, `blocked TITLE --why TEXT` (agent socket) | no | `cockpit-tasks` |
| `cockpit workflow presets`, `list [--project P] [--all]`, `show ID`, `packet ID`, `complete ID --changed T --checked T --outcome T [--needs-human T] [--evidence REF]... [--waive REQUIREMENT]...`, `release ID [handoff flags]`, `block ID ACTION --detail TEXT`, `unblock ID` (agent socket in a launched session, else the owner's) | no | `cockpit-workflows` |
| `cockpit workflow begin ID`, `checkpoint ID NOTE` (agent socket); `start ITEM [--preset NAME]`, `cancel ID` (the owner's) | no | `cockpit-workflows` |
| `cockpit check run CHECK --project P [--checkout PATH]` (the owner's: prints the run's output and receipt, exits 0 only on a pass), `gate [--project P] [--checkout PATH]` (one line per check: status, receipt, next action; exits 0 only when the gate passed), `claim CHECK STATEMENT` (agent socket; recorded unverified) | no | `cockpit-verification` |
| `cockpit todo edit ID [--revision R] [--title T] [--notes TEXT] [--kind KIND]`, `priority ID LEVEL`, `queue`/`unqueue`/`approve`/`decline`/`reopen ID`, `after`/`unwait ID ON...`, `image add ID FILE`, `image get ID NUMBER FILE`, `draft save (ID \| --new PROJECT)`, `drafts`, `draft discard` | no | `cockpit-tasks` |

`cockpit todo` picks its socket from `COCKPIT_LAUNCH_TOKEN`: a launched session's verbs go to
the agent socket, where the daemon identifies it; without the token they are the owner's.
`show` ends with `next:`, what has to happen for the item to move.

Usage errors exit 2; configuration errors exit 1 with the file and line at fault. A daemon
verb's refusal exits 1 as `cockpit: <code>: <message>`.

`cockpit ask run` and `cockpit ask guide` keep `ask-to-run`'s exit codes: the command's own
once it ran, 125 declined or the guide closed, 124 still pending after
`COCKPIT_ASK_WAIT_SECONDS` (default 540; `--wait ID` waits on), 126 the daemon unreachable
(with the fix), 3 stuck on a guide step, 1 any other refusal. `--no-wait` files the request
and exits 124 at once, so the agent goes on working and collects the answer with `--wait ID`.
`approval show` lists the kinds of hidden characters escaped, the changed build files the
command runs and the `timeout` hint; `approval list` folds a long command behind an explicit
"more". The owner's verbs print commands,
directories and reasons under the daemon's display rules, and count guide steps from 1.
