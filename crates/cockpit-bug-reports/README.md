# cockpit-bug-reports

Owns bug reports about Cockpit itself: drafts from agents, the daemon and the owner, their
fingerprint, the daily limit that merges repeats, the owner's edits and decisions, and the
issue text and new-issue URL a send hands the owner. Added by #427; design: report 12's
support bundle rule and report 17's agent verbs.

**Nothing is filed automatically.** A report is a draft until the owner sends it; sending
happens in the owner's browser or with the owner's own `gh` login, from the CLI.

**Depends on:** `cockpit-turso` only (rule `bug-reports -> the database only` in
`cockpit-workspace-rules/tests/crate_graph.rs`). The daemon passes in what it knows about
itself (version, platform, providers) and writes the support bundle through
`cockpit-diagnostics`, then records it here.
**Used by:** `cockpit-daemon`, which serves `cockpit agent report-bug` on the agent socket,
`cockpit bug-report` on the host-only socket, and opens drafts for the faults it sees.

## Sources and the daily limit

| Source | Opens | Limit |
| --- | --- | --- |
| The owner (`cockpit bug-report new`, the TUI) | a new draft every time | none |
| A session's own report (`cockpit agent report-bug`) | one draft per fingerprint per UTC day | also 5 new drafts per session per day |
| A session the owner asked (`--requested-by-owner`) | a new draft every time | 5 new drafts per session per day |
| The daemon (panic, stalled lane, crash at restart, repeated internal error) | one draft per fingerprint per UTC day | none beyond it |

A limited repeat of today's draft counts as one more occurrence of it, and adds its
description while the draft stays under 8000 characters. Once today's draft is sent or
discarded, a repeat opens nothing until the next day.

The fingerprint is `cockpit-<version>/<kind>/<component>`; kinds are `timeout`,
`unreachable`, `internal`, `panic`, `stall`, `crash` and `other`. A send looks for an existing
issue with the same fingerprint only when it is specific (not `other` with no component).

## Statuses

`draft` (the owner reviews and may edit it) → `sent` or `discarded`. Only a draft changes.

## Sending

`issue()` renders the title, the body `gh issue create` files, and the prefilled URL of the
`bug-from-cockpit.yml` template, whose field ids are `FIELDS`. The URL is cut to 8000 bytes,
the description and steps shrinking first, with a pointer to `cockpit bug-report show`. The
issue names the bundle's file name and contents, never the owner's path; the owner attaches
the file.
