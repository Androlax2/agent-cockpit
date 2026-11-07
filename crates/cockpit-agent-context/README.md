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
| `overview` | `torlek/SKILL.md` | always first: where the brief is, the topics |
| `workflow` | `torlek/references/workflow.md` | beginning, checkpointing, finishing or blocking a step |
| `checks` | `torlek/references/checks.md` | proving a result, reading a gate |
| `approvals` | `torlek/references/approvals.md` | a host command, manual steps, a decision |
| `coordination` | `torlek/references/coordination.md` | status, questions, task items |
| `sandbox` | `torlek/references/sandbox.md` | denied paths, blocked hosts, Docker, secrets |

Progressive disclosure (catalog, then instructions, then resources): a harness preloads only
the skill's name and description; [`catalog`] lists the topics; [`document`] reads one;
[`manual`] is every topic in one read (`torlek --skill`). Every command the manual names is
checked against the CLI's usage (`cockpit-cli` `tests/instructions.rs`).

[`write_bundle`] writes it under a directory, each file stamped with the release and "never
edit", each replaced by a rename (a symbolic link is replaced, never followed; a directory
that is a link is refused), files an older release shipped removed. The daemon calls it at
every start (`$XDG_STATE_HOME/torlek/agent-context`) and names the directory to every agent
in `TORLEK_INSTRUCTIONS`; a start that cannot write it fails.

## The manifest

[`manifest()`] lists every packet of the product with its path, source (`torlek`), scope,
version, SHA-256 of the file as written, trust decision, precedence and the packets it
supersedes. A hash is identity, not trust: these are trusted because the product ships them,
and rank `product-invariant`, which nothing may shadow. [`manifest_id`] (`m-` and 16 hex
digits, over each entry's path, hash and trust) changes with any packet; the written
`manifest.json` also records the earlier release's manifest it replaced (`supersedes`).

[`discover_guidance`] adds the scopes below the product's (#420), read when a session asks
for its brief, and [`session_manifest`] joins them to the product's packets:

| Scope | Files | Trust | Precedence |
| --- | --- | --- | --- |
| `user` | `~/.claude/CLAUDE.md` | `owner` | `user`, shadowing the project's guidance |
| `project` | `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md`, `AGENTS.md`, `.agents/rules/*.md`, `.claude/rules/*.md` of the checkout | `trusted` when the owner trusts the project (`projects.trusted`, or a clone of one), else `untrusted` | `project` |

Each entry carries the file's absolute path and hash; a file that exists and cannot be read
is listed without a hash. Listing is not loading: each harness decides what it reads. Not
listed yet: guidance in directories above the checkout or below it, `~/.claude/rules`, and
files a listed file `@`-imports, so changing only those does not change the manifest. The id a
brief names is the session manifest's, so it changes when a listed file, or the owner's trust
in the project, does; `instruction-catalog`, `manifest.json` and the launch record name the
product bundle's id, which has no workspace.

## The launch brief

[`brief()`] words the [`LaunchFacts`] the daemon gathers from their owners: the agent channel
(which session), sessions (its workspace), projects (its project), workflows (the step it
does, attempt, whether a checkpoint exists) and authorization (its grants and their
revision, a reference: they are re-evaluated at every action) and the guidance files above,
four named strongest first with a short hash and trust, the rest counted. It states the
invariants first (prose grants nothing, truthful evidence, the approval route), then those
facts, then the discovery and recovery entry points. [`BRIEF_TARGET_BYTES`] (2 KiB) bounds it with
long fields; a typical brief is about 1 KiB.

[`task_packet`] frames the step's packet the daemon gives with the brief at every context
start (#781): `torlek workflow packet`'s text with the last handoff only, under the command
that prints all of it, cut at [`TASK_PACKET_CAP_BYTES`] (2 KiB).

[`context_budget`] measures what Torlek adds: always loaded (the brief and the skill's
catalog entry; the daemon adds the packet) apart from read on demand (the manual). The harness's own instructions and
tool schemas are the provider's to measure.

## Channels

All served by the running daemon, so they match its release:

- CLI: `torlek --skill`, `torlek docs [TOPIC]`, `torlek brief [--measure]`.
- MCP: `torlek mcp` (stdio) offers the topics as resources and the `torlek_docs` and
  `torlek_brief` tools.
- Skills: the bundle on disk under `TORLEK_INSTRUCTIONS`. Installing or pointing each
  harness at it belongs to its provider adapter.

## Dependency edges

- Depends on: `serde`, `serde_json`, `sha2`; no Cockpit crate
  (`cockpit-workspace-rules/tests/crate_graph.rs`, rule
  `agent-context -> nothing product-level`): facts arrive as plain values.
- Used by: the daemon.

## Tests

- `tests/bundle.rs`: topics, the manual, the skill's frontmatter, the manifest and its id.
- `tests/guidance.rs`: the user and project scopes, their trust and precedence, and that a
  changed file changes the manifest id.
- `tests/write.rs`: the layout, the stamp, overwriting, removing stale files, `supersedes`,
  symbolic links.
- `tests/brief.rs`: invariants first, the dynamic fields, the size target, the budget.
- `tests/packet.rs`: the packet's frame, its cap, cuts on character boundaries.
- Through the sockets: `cockpit-daemon` `tests/agent_brief.rs` (the bundle at start,
  `TORLEK_INSTRUCTIONS`, a launched session's brief) and `tests/agent_onboarding.rs` (a
  clean-home agent finds its workflow, checks and approval route through `torlek`);
  `cockpit-cli` `tests/instructions.rs` (the verbs and the MCP server).
