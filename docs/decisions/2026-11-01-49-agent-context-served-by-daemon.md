# The daemon owns the agent instruction bundle and serves it; the CLI only relays

Issue #49. Decided by the implementing session, within report 08 and the Herdr review on #49
(bundle written at start, version-stamped, overwritten, never hand-edited; a one-read
`cockpit --skill`).

- **`cockpit-agent-context` depends on no Cockpit crate.** It words plain facts
  (`LaunchFacts`) the daemon gathers from their owners (sessions, projects, workflows,
  authorization), so it can never become an authority. Rule
  `agent-context -> nothing product-level` in `crates/cockpit-workspace-rules/tests/crate_graph.rs`.
- **The daemon serves every channel; the CLI embeds nothing.** `cockpit --skill`, `docs`,
  `brief` and `mcp` ask the running daemon (`InstructionCatalog`, `InstructionsRead`,
  `AgentBrief`), so an agent always reads the instructions of the release that runs it, and
  the CLI's dependency allow-list (#77) is unchanged. Without a daemon they fail loud.
- **The bundle is written at every daemon start** under `$XDG_STATE_HOME/cockpit/agent-context`,
  each file replaced by a rename, stale files removed, and named to every agent in
  `COCKPIT_INSTRUCTIONS`. A start that cannot write it fails: a stale bundle would tell agents
  the wrong commands.
- **The brief is a reference, not a grant.** It names the grants and their revision;
  authorization still decides at every action. Target 2 KiB; the manual is read on demand.
- **The manifest covers the product scope only.** User and project guidance join with their
  own provenance and precedence later (leftover).
- **Rejected:** the CLI embedding the bundle offline, because it would add
  `cockpit-agent-context` to the CLI's allow-list and could drift from the running daemon.
- **Rejected:** writing per-provider files (`CLAUDE.md`, `AGENTS.md`, plugin config) here,
  because installing into a harness is each provider adapter's job (report 19); this crate
  stays provider-neutral.
