# Sources and measurements for #1327

Appendix to [the agent host practices report](../2026-11-10-1327-agent-host-practices.md), for
a reader checking a claim. Every repository is pinned to the commit read on 2026-11-10; paths
are relative to that commit. Nothing downloaded was run.

## Repositories read

| Project | Repository at the commit read | Files the report cites |
| --- | --- | --- |
| Claude Squad | [smtg-ai/claude-squad@ce1ffb43](https://github.com/smtg-ai/claude-squad/tree/ce1ffb4392b01f38e2c4599c7c84d2a93973b138) | `app/app.go`, `session/tmux/tmux.go`, `session/instance.go`, `daemon/daemon.go` |
| Crystal | [stravu/crystal@1e18e0bc](https://github.com/stravu/crystal/tree/1e18e0bc981225f75b5226f82a300fa741970c6f) | `main/src/utils/contextCompactor.ts`, `main/src/services/mcpPermissionBridge.ts`, `main/src/services/gitStatusManager.ts`, `main/src/database/database.ts` |
| Claude Code Agent Farm | [Dicklesworthstone/claude_code_agent_farm@d88ba78e](https://github.com/Dicklesworthstone/claude_code_agent_farm/tree/d88ba78ea5e50abed8e7f60e2cf00ce678b6375d) | `claude_code_agent_farm.py` |
| container-use | [dagger/container-use@2e43e625](https://github.com/dagger/container-use/tree/2e43e625e95216b719ec9338f4034fd3a0be2734) | `mcpserver/tools.go`, `mcpserver/args.go`, `mcpserver/singletenant.go`, `cmd/container-use/agent/configure.go`, `repository/git.go` |
| Vibe Kanban | [BloopAI/vibe-kanban@72b3e817](https://github.com/BloopAI/vibe-kanban/tree/72b3e817e528fb59163471286ccbebbaa300ebde) | `crates/mcp/src/task_server/mod.rs`, `crates/executors/src/executors/claude/client.rs`, `crates/executors/src/executors/claude.rs`, `crates/executors/src/executors/codex/client.rs` |
| Sculptor | [imbue-ai/sculptor@12fbfe93](https://github.com/imbue-ai/sculptor/tree/12fbfe93a302fce36ed2c5dca1611e8189bbb99a) | `sculptor/sculptor/agents/default/claude_code_sdk/mcp_result_formatters.py`, `sculptor/sculptor/services/ci_babysitter_service/coordinator.py`, `samples/terminal_agents/claude-code/claude-code-hooks.json` |
| Agent Orchestrator | [ComposioHQ/agent-orchestrator@84e351e1](https://github.com/ComposioHQ/agent-orchestrator/tree/84e351e19131afe8bc771e1c66c851922704605d) | `backend/internal/service/report/service.go`, `backend/internal/service/report/coordinator.go`, `backend/internal/lifecycle/reactions.go`, `backend/internal/observe/usage/parser.go`, `backend/internal/session_manager/manager.go` |
| Beads | [gastownhall/beads@9d4ebf42](https://github.com/gastownhall/beads/tree/9d4ebf42f0496cdd0f0d7fdc6062c0981f374a70) | `cmd/bd/prime.go`, `cmd/bd/errors.go`, `cmd/bd/if_revision.go`, `docs/integrations/claude-code.md`, `docs/cli-reference/prime.md`, `integrations/beads-mcp/CONTEXT_ENGINEERING.md`, `integrations/beads-mcp/src/beads_mcp/server.py` |
| Gas Town | [gastownhall/gastown@4b24a16e](https://github.com/gastownhall/gastown/tree/4b24a16e5195faa01ffc052db611a01530484854) | `internal/nudge/queue.go`, `internal/cmd/prime.go`, `internal/cmd/mail_check.go`, `internal/cmd/molecule_await_signal.go`, `internal/cmd/costs.go`, `docs/design/mail-protocol.md` |
| MCP Agent Mail | [Dicklesworthstone/mcp_agent_mail@9138f34b](https://github.com/Dicklesworthstone/mcp_agent_mail/tree/9138f34b1ed220235dbfeaa3f1fc9fe4444f4e72) | `src/mcp_agent_mail/app.py`, `src/mcp_agent_mail/config.py`, `scripts/hooks/check_inbox.sh` |
| Ruflo (Claude Flow) | [ruvnet/ruflo@4184c5cc](https://github.com/ruvnet/ruflo/tree/4184c5cc24f27d5831f871e6d7160e2a9322cbbb) | `v3/@claude-flow/cli/src/mcp-server.ts`, `v3/@claude-flow/cli/src/commands/doctor.ts`, `docs/benchmarks/context-delegation.md` |
| Roo Code | [RooCodeInc/Roo-Code@b867ec91](https://github.com/RooCodeInc/Roo-Code/tree/b867ec9145750d0ae1ff7f02d35406e9bf2a0b16) (archived) | `packages/types/src/mode.ts`, `src/core/webview/ClineProvider.ts`, `src/core/tools/ToolRepetitionDetector.ts`, `src/core/webview/aggregateTaskCosts.ts` |
| OpenHands SDK | [OpenHands/software-agent-sdk@ce89349b](https://github.com/OpenHands/software-agent-sdk/tree/ce89349b827c1b5df531a104ae874a6e0bdc2107) | `openhands-sdk/openhands/sdk/conversation/stuck_detector.py`, `.../sdk/utils/truncate.py`, `.../sdk/agent/acp_skills.py`, `.../sdk/agent/acp_agent.py`, `.../sdk/conversation/impl/local_conversation.py` |
| SWE-agent | [SWE-agent/SWE-agent@3ea751c0](https://github.com/SWE-agent/SWE-agent/tree/3ea751c087f32b16e039a2233dd6eefecef325d5) | `sweagent/agent/agents.py`, `sweagent/agent/history_processors.py`, `sweagent/agent/models.py`, `tools/windowed_edit_linting/bin/edit` |
| Aider | [Aider-AI/aider@5dc9490b](https://github.com/Aider-AI/aider/tree/5dc9490bb35f9729ef2c95d00a19ccd30c26339c) | `aider/repomap.py`, `aider/coders/chat_chunks.py`, `aider/coders/base_coder.py`, `aider/coders/editblock_coder.py`, `aider/io.py` |
| goose | [aaif-goose/goose@72849d8e](https://github.com/aaif-goose/goose/tree/72849d8e9c85a011b30ebac31dce8c0806cfff8c) | `crates/goose/src/agents/large_response_handler.rs`, `crates/goose/src/context_mgmt/mod.rs`, `crates/goose/src/hints/load_hints.rs`, `crates/goose/src/permission/permission_inspector.rs`, `crates/goose/src/tool_monitor.rs` |
| Cline | [cline/cline@92b9f722](https://github.com/cline/cline/tree/92b9f722a16d2a03c1153e12f9a0e759455f509d) | `sdk/packages/core/src/extensions/tools/executors/output-limits.ts`, `sdk/packages/core/src/session/services/message-builder.ts`, `sdk/packages/core/src/session/services/tool-result-cache.ts`, `sdk/packages/core/src/runtime/safety/loop-detection.ts` |
| Codex | [openai/codex@b32ada53](https://github.com/openai/codex/tree/b32ada5320b043bde03a10edab6b88fc126812b4) | `codex-rs/hooks/src/output_spill.rs`, `codex-rs/core/src/agents_md.rs`, `codex-rs/core/src/compact.rs`, `codex-rs/execpolicy/README.md`, `codex-rs/exec/src/exec_events.rs` |
| OpenCode | [anomalyco/opencode@2ab14901](https://github.com/anomalyco/opencode/tree/2ab149011f20708b4295109d4c847de7d6979fd9) | `services/site/src/docs/content/migrate-v1.mdx`, `.../content/compaction.mdx`, `.../content/mcp-servers.mdx`, `packages/schema/src/session-event.ts`, `packages/core/src/session/usage.ts` |
| sandbox-runtime | [anthropics/sandbox-runtime@f7a0f19f](https://github.com/anthropics/sandbox-runtime/tree/f7a0f19fd3b4437bcf808f9c18c1f73e9f1a9889) | `README.md` |
| Agent Client Protocol | [agentclientprotocol/agent-client-protocol@3961af94](https://github.com/agentclientprotocol/agent-client-protocol/tree/3961af94fa62d2386d993cadf1afc8b146fc799a) | `docs/protocol/v1/session-setup.mdx`, `docs/protocol/v1/tool-calls.mdx` |
| Model Context Protocol | [modelcontextprotocol/modelcontextprotocol@632a0c9a](https://github.com/modelcontextprotocol/modelcontextprotocol/tree/632a0c9a65c9b4fbc889d8aface5c9cf5edef3b7), revision 2026-07-28 | `docs/specification/2026-07-28/server/tools.mdx`, the revision's changelog |
| ccusage | [ccusage/ccusage@373fdd19](https://github.com/ccusage/ccusage/tree/373fdd19997c97c380f0490a6e377f8c65a3a39d) | the Claude and Codex loaders and the cost modes |

Public documentation read on 2026-11-10:

- Claude Code: the [hooks](https://code.claude.com/docs/en/hooks), [prompt caching](https://code.claude.com/docs/en/prompt-caching),
  [context window](https://code.claude.com/docs/en/context-window), [sandboxing](https://code.claude.com/docs/en/sandboxing)
  and [monitoring](https://code.claude.com/docs/en/monitoring-usage) pages.
- Anthropic engineering, [Making Claude Code safer with sandboxing](https://www.anthropic.com/engineering/claude-code-sandboxing).

Four repositories moved; the table names where they live now: steveyegge/beads and
steveyegge/gastown (now gastownhall), ruvnet/claude-flow (now ruvnet/ruflo), block/goose (now
aaif-goose). The OpenHands agent loop is now in software-agent-sdk; the OpenHands repository's
`main` is a front end that hosts other harnesses over the Agent Client Protocol.

## Torlek measurements

Source at `c2d3ecaa` (main, 2026-11-10). Rendered text comes from the installed Torlek
0.1.0-alpha.7, run inside a worker session. Tokens are estimated as bytes ÷ 4, the rate
[#619](../2026-11-04-619-agent-token-efficiency.md#method-and-labels) used, about ±25 %.

| Measured | Command | Bytes |
| --- | --- | ---: |
| Always loaded, all parts | `torlek brief --measure` | 2,060 |
| Launch brief (no task packet) | `torlek brief \| wc -c` | 1,670 |
| Whole manual | `torlek --skill \| wc -c` | 12,830 |
| Manual index | `torlek docs \| wc -c` | 566 |
| Topic `coordination` (source, same as installed) | `wc -c crates/cockpit-agent-context/bundle/references/coordination.md` | 5,959 |
| Topic `sandbox` (source / installed) | `wc -c` of the source; `torlek docs sandbox \| wc -c` | 2,741 / 1,449 |
| Topics `workflow`, `checks`, `approvals` | `wc -c` of each reference | 1,686, 1,057, 1,049 |
| Error inside a verb (`agent report --bogus`) | `torlek agent report --bogus 2>&1 \| wc -c` | 958 |
| Error on an unknown top-level word (`--version`) | `torlek --version 2>&1 \| wc -c` | 6,410 |
| Repository guidance a worker loads | `wc -c AGENTS.md CLAUDE.md` | 6,838 |
| Coordinator skill | `wc -c .agents/skills/coordinator/SKILL.md` | 13,608 |
| Worker prompt suffix | `wc -c .agents/skills/coordinator/prompts/worker_suffix.txt` | 2,000 |
| Coordinator watcher script | `wc -c .agents/skills/coordinator/scripts/coordinator_watch.py` | 23,123 |

Torlek calls a worker makes, counted from the instructions it is given (the brief, the worker
suffix and `repo-workflow`), not from a conversation:

| Call | How often |
| --- | --- |
| `torlek todo start ID` | once |
| `torlek agent report --doing TEXT` | once per phase; the suffix asks to keep it current |
| `torlek todo done ID --summary TEXT` | once, after the merge |
| `torlek agent report --needs-input QUESTION` | once per question |
| `torlek docs TOPIC`, `torlek brief` | not required; the brief arrives without a call |
| Torlek MCP tools | not required |

State changes (working, waiting, a dialog, a compaction) come from the harness hooks, not from
calls the agent makes.
