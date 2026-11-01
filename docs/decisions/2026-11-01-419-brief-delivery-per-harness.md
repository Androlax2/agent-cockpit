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
  `cockpit docs` already reach it; each harness's skill loading is unverified, so it stays
  on #419.

Evidence: `cockpit-claude` `bridge::each_session_start_adds_the_launch_brief_to_claudes_context`,
`launch::every_launch_registers_cockpits_mcp_server`; `cockpit-codex`
`bridge::each_session_start_adds_the_launch_brief_to_codexs_context`,
`launch::with_the_bridge_a_launch_registers_cockpits_mcp_server`; `cockpit-opencode`
`plugin::every_system_prompt_carries_the_launch_brief_read_once`,
`launch::a_launch_with_the_bridge_registers_cockpits_mcp_server_beside_the_owners`;
`cockpit-daemon` `sandbox::a_launch_may_read_the_instruction_bundle_and_the_opencode_plugin`.
A live model seeing the brief stays `validate` (needs signed-in harnesses).
