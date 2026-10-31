# cockpit-cli

Owns the `cockpit` command line: it parses verbs and renders their results.

**Depends on:** `cockpit-client` and `cockpit-protocol` for daemon verbs (once they exist), and
the capability crates whose verbs must work without the daemon, listed in
[report 02](../../docs/design/greenfield/02-crate-architecture.md) (#77). The
`dependency_edges` test fails when `Cargo.lock` shows the CLI reaching any other product crate.
**Used by:** nothing; it is a frontend.

## Verbs

| Verb | Works without the daemon | Owner |
| --- | --- | --- |
| `cockpit config show [--project DIR] [--set KEY=VALUE]...` | yes | `cockpit-config` |

Usage errors exit 2; configuration errors exit 1 with the file and line at fault.
