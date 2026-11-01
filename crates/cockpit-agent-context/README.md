# cockpit-agent-context

Owns what an agent is told: the version-matched instruction bundle, its manifest with provenance, the launch brief assembled from owners' facts, and how much context Cockpit adds; never an authority.

Everything it says requests; `cockpit-authorization` decides, at every mediated action.
Design: [report 08](../../docs/design/greenfield/08-agent-onboarding-context.md); the
bundle-on-every-start and one-read manual come from the Herdr review on #49.

## The instruction bundle

Shipped in the binary from `bundle/`, laid out as an [Agent Skills](https://agentskills.io)
skill so any harness that reads skills can load it, and provider-neutral in its words:

| Topic | File | Read it when |
| --- | --- | --- |
| `overview` | `cockpit/SKILL.md` | always first: the brief, the rules nothing overrides, the topics |
| `workflow` | `cockpit/references/workflow.md` | beginning, checkpointing, finishing or blocking a step |
| `checks` | `cockpit/references/checks.md` | proving a result, reading a gate |
| `approvals` | `cockpit/references/approvals.md` | a host command, manual steps, a decision |
| `coordination` | `cockpit/references/coordination.md` | status, questions, task items |
| `sandbox` | `cockpit/references/sandbox.md` | denied paths, blocked hosts, Docker, secrets |

Progressive disclosure (catalog, then instructions, then resources): a harness preloads only
the skill's name and description; [`catalog`] lists the topics; [`document`] reads one;
[`manual`] is every topic in one read (`cockpit --skill`). Every command the manual names is
checked against the CLI's usage (`cockpit-cli` `tests/instructions.rs`).

[`write_bundle`] writes it under a directory, each file stamped with the release and "never
edit", each replaced by a rename (a symbolic link is replaced, never followed; a directory
that is a link is refused), files an older release shipped removed. The daemon calls it at
every start (`$XDG_STATE_HOME/cockpit/agent-context`) and names the directory to every agent
in `COCKPIT_INSTRUCTIONS`; a start that cannot write it fails.

## The manifest

[`manifest()`] lists every packet with its path, source (`cockpit`), scope, version, SHA-256
of the file as written, trust decision, precedence and the packets it supersedes. A hash is
identity, not trust: these are trusted because the product ships them, and rank
`product-invariant`, which nothing may shadow. [`manifest_id`] (`m-` and 16 hex digits)
changes with any packet; the brief names it. The written `manifest.json` also records the
earlier release's manifest it replaced (`supersedes`). Only the product scope exists so far;
user and project guidance join with their own provenance later.

## The launch brief

[`brief()`] words the [`LaunchFacts`] the daemon gathers from their owners: the agent channel
(which session), sessions (its workspace), projects (its project), workflows (the step it
does, attempt, whether a checkpoint exists) and authorization (its grants and their
revision, a reference: they are re-evaluated at every action). It states the invariants
first (prose grants nothing, truthful evidence, the approval route), then those facts, then
the discovery and recovery entry points. [`BRIEF_TARGET_BYTES`] (2 KiB) bounds it with
long fields; a typical brief is about 1 KiB.

[`context_budget`] measures what Cockpit adds: always loaded (the brief and the skill's
catalog entry) apart from read on demand (the manual). The harness's own instructions and
tool schemas are the provider's to measure.

## Channels

All served by the running daemon, so they match its release:

- CLI: `cockpit --skill`, `cockpit docs [TOPIC]`, `cockpit brief [--measure]`.
- MCP: `cockpit mcp` (stdio) offers the topics as resources and the `cockpit_docs` and
  `cockpit_brief` tools.
- Skills: the bundle on disk under `COCKPIT_INSTRUCTIONS`. Installing or pointing each
  harness at it belongs to its provider adapter.

## Dependency edges

- Depends on: `serde`, `serde_json`, `sha2`; no Cockpit crate
  (`cockpit-workspace-rules/tests/crate_graph.rs`, rule
  `agent-context -> nothing product-level`): facts arrive as plain values.
- Used by: the daemon.

## Tests

- `tests/bundle.rs`: topics, the manual, the skill's frontmatter, the manifest and its id.
- `tests/write.rs`: the layout, the stamp, overwriting, removing stale files, `supersedes`,
  symbolic links.
- `tests/brief.rs`: invariants first, the dynamic fields, the size target, the budget.
- Through the sockets: `cockpit-daemon` `tests/agent_brief.rs` (the bundle at start,
  `COCKPIT_INSTRUCTIONS`, a launched session's brief) and `tests/agent_onboarding.rs` (a
  clean-home agent finds its workflow, checks and approval route through `cockpit`);
  `cockpit-cli` `tests/instructions.rs` (the verbs and the MCP server).
