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
| `cockpit project add DIR [--name NAME]`, `cockpit project list` | no | `cockpit-projects` |
| `cockpit checkout list [PROJECT]` | no | `cockpit-projects` |
| `cockpit checkout clone PROJECT BRANCH`, `cockpit checkout review PROJECT PULL_REQUEST` | no | `cockpit-projects` |
| `cockpit checkout claim CHECKOUT HOLDER`, `cockpit checkout release CHECKOUT HOLDER` | no | `cockpit-projects` |
| `cockpit checkout sweep [--dry-run]`, `cockpit checkout push CHECKOUT` | no | `cockpit-projects` |
| `cockpit ask run [--cwd DIR] --reason TEXT COMMAND`, `--wait ID`, `--check` (agent socket) | no | `cockpit-approvals` |
| `cockpit ask guide [--cwd DIR]` (steps on standard input), `--wait ID` (agent socket) | no | `cockpit-approvals` |
| `cockpit approval list`, `show ID`, `approve ID --revision R --digest D`, `decline ID`, `edit ID --revision R COMMAND`, `input ID TEXT` | no | `cockpit-approvals` |
| `cockpit guide show ID`, `answer ID STEP [ANSWER]`, `stuck ID STEP NOTE`, `close ID` | no | `cockpit-approvals` |

Usage errors exit 2; configuration errors exit 1 with the file and line at fault. A daemon
verb's refusal exits 1 as `cockpit: <code>: <message>`.

`cockpit ask run` and `cockpit ask guide` keep `ask-to-run`'s exit codes: the command's own
once it ran, 125 declined or the guide closed, 124 still pending after
`COCKPIT_ASK_WAIT_SECONDS` (default 540; `--wait ID` waits on), 126 the daemon unreachable
(with the fix), 3 stuck on a guide step, 1 any other refusal. The owner's verbs print commands,
directories and reasons under the daemon's display rules, and count guide steps from 1.
