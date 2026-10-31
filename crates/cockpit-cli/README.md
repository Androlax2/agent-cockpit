# cockpit-cli

Owns the `cockpit` command line: it parses verbs and renders their results.

**Depends on:** `cockpit-client` and `cockpit-protocol` for daemon verbs, and
the capability crates whose verbs must work without the daemon, listed in
[report 02](../../docs/design/greenfield/02-crate-architecture.md) (#77). The
`dependency_edges` test fails when `Cargo.lock` shows the CLI reaching any other product crate.
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

Usage errors exit 2; configuration errors exit 1 with the file and line at fault. A daemon
verb's refusal exits 1 as `cockpit: <code>: <message>`.
