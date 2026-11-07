# Each adapter puts the launch brief in front of its agent at every context start and registers `cockpit mcp`

Issue #419. Decided by the implementing session, within report 08.

- **The brief is read live through `cockpit brief`, never baked into the launch.** Claude
  Code's and Codex's hook bridges answer every `SessionStart` (launch, resume, `/clear`,
  compaction) with the brief as `hookSpecificOutput.additionalContext`; OpenCode's plugin adds
  it to every system prompt (`experimental.chat.system.transform`, read once, asked again a
  minute after a failure, since it runs before every model call of a turn). The launch token identifies the session, so the brief is the daemon's
  current one and a compaction never drops it.
- **A brief that cannot be read never blocks the agent.** The hooks read it while the state
  report runs, within the hook's one timeout, and exit 1 naming each failure (shown, never 2);
  the plugin leaves the prompt as OpenCode made it.
- **`cockpit mcp` is registered per launch, only with the bridge**, under the name `cockpit`:
  Claude `--mcp-config`, Codex `mcp_servers.cockpit.*` on the session's app-server (with
  `env_vars` naming the launch token and the agent socket, which Codex would not pass),
  OpenCode `mcp.cockpit` in `OPENCODE_CONFIG_CONTENT` beside the owner's servers, refused
  when the owner's already names `cockpit`. The bridge carries
  the `cockpit` path the server runs from.
- **The bundle folder is readable in every sandbox.** Greywall denies reads by default, so
  `$COCKPIT_INSTRUCTIONS` joins the read-only paths (`cockpit-daemon` `serve.rs`).
- **Rejected:** passing the brief as a system-prompt flag at launch (`--append-system-prompt`,
  `developer_instructions`): it would be computed before the session binds, go stale, and
  vanish from Codex and Claude Code after `/clear`.
- **Not done here:** registering the bundle as a native skill per harness. The brief and
  `cockpit docs` already reach it; each harness's skill loading was unverified, so it stayed
  on #419 and is decided in the section below.

Evidence: `cockpit-claude` `bridge::each_session_start_adds_the_launch_brief_to_claudes_context`,
`launch::every_launch_registers_cockpits_mcp_server`; `cockpit-codex`
`bridge::each_session_start_adds_the_launch_brief_to_codexs_context`,
`launch::with_the_bridge_a_launch_registers_cockpits_mcp_server`; `cockpit-opencode`
`plugin::every_system_prompt_carries_the_launch_brief_read_once`,
`launch::a_launch_with_the_bridge_registers_cockpits_mcp_server_beside_the_owners`;
`cockpit-daemon` `sandbox::a_launch_may_read_the_instruction_bundle_and_the_opencode_plugin`.
A live model seeing the brief stays `validate` (needs signed-in harnesses).

## Revisited 2026-11-01 (#419, native skill)

Decided by the implementing session: each harness's documented skill paths, read on
2026-11-01, leave only these choices. Claude Code and OpenCode list the bundle as a native
`cockpit` skill; Codex does not. This closes the "not done here" point above.

- **Claude Code: `--add-dir <state>/claude-skills`.** Claude Code loads the skills in the
  `.claude/skills/` of a directory given with `--add-dir` (code.claude.com/docs/en/skills,
  "Additional directory"). The daemon writes the bundle a second time at
  `<state>/claude-skills/.claude/skills/` at every start, a copy rather than a link because
  the documentation does not say links are followed. The sandbox may read that folder.
- **OpenCode: `skills.paths` in `OPENCODE_CONFIG_CONTENT`.** OpenCode 1.19.5 scans each
  folder in `skills.paths` for `**/SKILL.md` (`packages/opencode/src/skill/index.ts`, tag
  `v1.19.5`), so `$COCKPIT_INSTRUCTIONS` is added beside the owner's own paths; an owner's
  `skills` of another shape is refused, never overwritten.
- **Codex: no native skill.** Codex reads skills only from `.agents/skills` in the project
  and its parents, `$HOME/.agents/skills`, `/etc/codex/skills` and its own; no flag or
  config key adds a folder (`[[skills.config]]` only turns one off). Writing into the
  owner's home or the project would change files Cockpit does not own, so Codex agents keep
  the brief, `cockpit docs` and `cockpit mcp`.
- **Rejected:** a symbolic link from `.claude/skills/cockpit` to the bundle (unverified that
  Claude Code follows it), and laying the whole bundle out as `.claude/skills/` (OpenCode's
  scan skips dot folders below a skill path, and `$COCKPIT_INSTRUCTIONS` would change shape).

Evidence: `cockpit-claude` `launch::a_launch_adds_the_instruction_skills_root_so_claude_lists_the_cockpit_skill`,
`cockpit-opencode` `launch::a_launch_adds_the_instruction_bundle_to_the_owners_skill_paths`,
`cockpit-daemon` `sandbox::a_claude_launch_adds_the_instruction_skills`. A live
model listing the skill stays `validate` (needs signed-in harnesses).

Through the real daemon, the delivered hook, MCP server and skill path work from the agent's
environment, unsandboxed and in Torlek's sandbox: `cockpit-daemon`
`agent_onboarding::a_claude_launch_adds_the_sessions_brief_as_its_context_starts`,
`agent_onboarding::the_mcp_server_a_claude_launch_registers_answers_the_sessions_brief`,
`sandbox::a_sandboxed_claude_gets_the_sessions_brief_as_its_context_starts`,
`sandbox::a_sandboxed_claudes_mcp_server_answers_the_sessions_brief`,
`sandbox::a_sandboxed_claude_reads_the_skill_its_launch_adds`.

## Revisited 2026-11-07 (#781, task packet)

Decided by the implementing session, applying follow-up 1 of the
[#686 compaction research](../research/2026-11-04-686-compaction-research.md#proposed-follow-up-issues-not-filed):
harness summaries kept about half of the issue numbers and branch names, so every context
start now carries the step's packet beside the brief.

- **The packet rides on `agent-brief`, as its own `packet` field.** `torlek brief` (and the
  `torlek_brief` MCP tool) print the brief, then the packet; the hooks and the plugin print
  what it prints, so no adapter reads anything new. The brief keeps its 2 KiB target; the
  packet has its own, and `--measure` counts both as always loaded.
- **It is `torlek workflow packet`'s text, trimmed.** The rendering moved from `cockpit-cli`
  to `cockpit-protocol` (`WorkflowPacketView::text`) so the two cannot drift. The daemon
  keeps the last handoff only and drops the decisions, frames it with the command that
  prints all of it, says its text requests and grants nothing, and cuts it at
  `TASK_PACKET_CAP_BYTES` (2048) on a character boundary. The notes come last, so they go
  first; a handoff of more than about a kilobyte is cut too, the cut saying so.
- **A packet the daemon cannot read never costs the brief.** It is replaced by a line saying
  it is unreadable and which command shows why.
- **OpenCode reads it again once a compaction's end reaches the plugin**
  (`session.compaction.ended`), since its plugin otherwise reads the brief once per process:
  that is OpenCode's context start. Between context starts it shows the packet as last read,
  as the brief's own step line already was.
- **Rejected:** folding the packet into the brief text (the brief's size target and
  `--measure` would lose their meaning), and a separate `cockpit brief --packet` call per
  adapter (three adapters changed for what one daemon answer gives).

Evidence: `cockpit-daemon`
`agent_onboarding::a_compacted_claude_context_gets_the_packet_torlek_workflow_packet_prints`,
`agent_brief::the_packet_with_the_brief_keeps_only_the_last_handoff_within_its_cap`;
`cockpit-agent-context` `packet::a_long_packet_is_cut_at_the_cap_and_says_so`; the pinned
compaction payloads: `cockpit-claude` and `cockpit-codex`
`bridge::a_compactions_session_start_adds_the_brief_and_task_packet`, `cockpit-opencode`
`plugin::a_compactions_end_reads_the_brief_and_task_packet_again`.
