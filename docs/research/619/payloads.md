# #619 evidence: payload inventory and measurements

Appendix to [the report](../2026-11-04-619-agent-token-efficiency.md). Source examined at
`fb683e93`, paths under `crates/`.

**Labels.**

- *Measured*: bytes of output from the installed `torlek` (0.1.0-alpha.2), whose bundle
  matches the source at `fb683e93` apart from its release stamp.
- *Template*: bytes computed from the format strings, with the field values shown.
- *Est.*: tokens estimated as bytes ÷ 4.

## Measurement commands

No live conversation, transcript or provider was used. Every command below is read-only.

```sh
torlek brief | wc -c                       # 1118
torlek brief --measure                     # always loaded 1394 (brief 1118, catalog 276; omits MCP instructions); on demand 9862
torlek --skill | wc -c                     # 9862
torlek docs | wc -c                        # 595
for t in overview workflow checks approvals coordination sandbox; do torlek docs $t | wc -c; done
                                           # 1691 1686 1057 1049 3270 987
torlek workflow packet 2>&1 >/dev/null | wc -c    # 5361, exit 2 (full usage)
torlek todo show 2>&1 >/dev/null | wc -c          # 5355
torlek agent report --bogus 2>&1 >/dev/null | wc -c   # 5348
torlek check gate --bogus 2>&1 >/dev/null | wc -c     # 5351
# MCP server: initialize, tools/list and resources/list over stdio, compact JSON
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' '{"jsonrpc":"2.0","id":3,"method":"resources/list"}' \
  | torlek mcp                             # instructions 114, tools 487, resources 1003
```

## Inventory of automatic agent-facing payloads

| Entry point | Owner | Trigger and how often | Recipient | Payload, bound | Starts a turn? |
| --- | --- | --- | --- | --- | --- |
| Launch brief | `cockpit-agent-context/src/brief.rs`; hooks: `cockpit-claude/src/bin/cockpit-claude-hook.rs`, `cockpit-codex/src/bin/cockpit-codex-hook.rs`; `cockpit-opencode/plugin/cockpit-opencode-plugin.js:120-131` | Every `SessionStart`: launch, resume, `/clear`, compaction. OpenCode: every model call, read once per process. | Every session | Invariants, workspace, task, grants, discovery. 1,118 bytes measured, ≤2 KiB target. | No |
| Skill catalog and body | `bundle/SKILL.md`; `--add-dir` (Claude), `skills.paths` (OpenCode) | Catalog: always. Body: when the agent loads the skill. | Claude, OpenCode | Catalog 276 bytes; body 1,977 bytes plus a 122-byte stamp | No |
| MCP server | `cockpit-cli/src/mcp.rs:90-150` | Registration, every launch | All three | Instructions 114 bytes, two tool schemas 487 bytes, resources 1,003 bytes on list | No |
| Manual topics | `bundle/references/*.md` | On demand | Asking agent | 987–3,270 bytes per topic, each with the 122-byte stamp | Its own tool call |
| State hooks | `cockpit-claude/src/hooks.rs:10-18`, `cockpit-codex/src/hooks.rs:27` | Every prompt, tool use, notification and stop | Daemon, not the model | Nothing on standard output | No; a process starts each time (latency only) |
| Inbox waiter (Claude) | `cockpit-claude-hook.rs:50-74`, `cockpit-cli/src/coordination.rs:113-153`, daemon `coordination.rs:401-477` | Waiter starts at each `SessionStart`/`Stop` and polls every 2 s. It delivers one message per wake. | Claude session | `[Torlek] Message from session S (a collaborator, not the owner): TEXT`, a 77-byte prefix plus ≤2,000 characters | **Yes**, one per message |
| Typed or steered message (Codex, OpenCode, other) | daemon `coordination.rs:892-1020`; `cockpit-codex/src/delivery.rs`; `cockpit-opencode/src/server.rs:178-191` | Per message, held while busy or in a dialog. Retries every 2–30 s and sends nothing on failure. | Non-collecting sessions | Same line | **Yes**; Codex mid-turn joins with `turn/steer` |
| Work placement | `cockpit-scheduling/src/model.rs:288-300`; daemon `scheduling.rs:78-241` | Pass every 30 s. Delivered only after 5 min idle at the prompt. Never redelivered. | Selected session | "Torlek gives you item N: read it with …" (141 bytes) or "… step `S` of workflow run R …" (152 bytes, template) | **Yes**, plus one read turn |
| Sender confirmation | `cockpit-cli/src/coordination.rs:84-101` | Per `agent message` | Sender | One line, naming `torlek agent sessions` | No; invites a poll |
| `agent sessions --watch` | `cockpit-cli/src/coordination.rs:31-59,216-268`; note: daemon `coordination.rs:1215-1235` | Polls every 2 s and prints changed lines | Watching agent | About 150–400 bytes per line; the "stuck" note includes `{waited} s` | Wakes a Monitor on every change |
| Workflow and todo replies | `cockpit-cli/src/workflow.rs:219-338`, `todo.rs:322-368` | Per verb | Calling agent | The whole record on every write ([before and after](#before-and-after-templates)) | Its own tool call |
| `ask run` result | `cockpit-cli/src/ask.rs:323-342`; `cockpit-approvals/src/pty.rs:96-97` | Per finished request | Asking agent | Last 120 lines of a 1 MiB tail, no byte cap | Its own tool call |
| Refusals | `cockpit-cli/src/agent.rs:225-249`; `cockpit-protocol/src/error.rs:103-116` | Per refusal | Calling agent | `torlek: refused: …` plus `to unblock (by): …`; echo ≤256 bytes | No |
| Usage error | `cockpit-cli/src/main.rs:56,303-307` | Per argument mistake | Calling agent | All 85 lines, 5.3 KB measured | No; costs bytes |

**Not found.**

- No reminder, nudge or timer prompt reaches an agent.
- `WakeCause::Message` and `WakeCause::Todo` have no production callers.
- Approval results are pulled from files, not pushed (`cockpit-agent-channel/src/answers.rs`).
- Receipts and `Coalescing::ReplaceableObservation` are unused in production.
- Sandbox denials carry no Torlek text.

## Before and after (templates)

Representative values: workflow 12, item 340, preset `code-change`, two checkpoint notes of
400 characters each.

**`workflow checkpoint` reply** (P3): before 1,125 bytes (template), up to 3,336 bytes with
three notes of 1,000 characters. After 108 bytes.

```text
before:
workflow 12 on item 340 (intent revision 3)
project: agent-cockpit
preset: code-change v1
status: running
step 1 implement (done, sf5837f057c63) [--step 1 --attempt 1]: <400-character note>
step 2 check (running, sf5837f057c63) [--step 2 --attempt 1]: <400-character note>
step 3 deliver (pending) [--step 3 --attempt 0]
next: session sf5837f057c63 works on step 2

after:
checkpoint recorded: workflow 12 step 2 [--step 2 --attempt 1]; next: session sf5837f057c63 works on step 2
```

Kept: ids, `--step/--attempt`, `next:`. The rest stays available through
`torlek workflow show 12`.

**`todo start` reply** (P3): before 1,713 bytes with 1,500 characters of notes. After 96
bytes.

```text
after:
item 340 started (intent revision 3, state revision 7); next: session sf5837f057c63 works on it
```

Kept: both revisions, which later `todo edit` and `todo handoff` calls need.

**`--watch` while a message is stuck** (P1):

```text
before (one line every 2 s; only the seconds differ):
… message-note="stuck: session s-b waits at its prompt, but its hook has not collected it for 95 s: …"
… message-note="stuck: session s-b waits at its prompt, but its hook has not collected it for 97 s: …"
after (once, when it becomes stuck):
… message-note="stuck since 14:02: session s-b's hook has not collected it; restarting the session starts its hook"
```

Kept: the fact that it is stuck, since when, and the action to fix it.

**Three queued messages to a Claude session** (P2a; P2b joins them the same way for the other harnesses): before, three wake-ups, each one a full
turn. After, one wake-up:

```text
[Torlek] 3 messages from collaborators, not the owner:
- from session s-a: <text>
- from session s-c: <text>
- from session s-a: <text>
```

Kept: each sender, the order, and the "not the owner" label.

**Usage error** (P4): before, 5,361 bytes. After, the `workflow` lines (530 bytes) plus one
line pointing to the full list.
