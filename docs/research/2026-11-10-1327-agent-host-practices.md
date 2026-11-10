# How other agent hosts make agents use the host well and spend fewer tokens

For Theo, choosing which techniques from other open-source agent hosts Torlek should adopt next.
Issue [#1327](https://github.com/Androlax2/agent-cockpit/issues/1327).
**Research and proposal, 2026-11-10; Theo has not decided.** Merging this document accepts
research delivery only. It changes no code, and no issue is filed for the proposals: Theo picks
first. The repositories read, their commits and the Torlek measurements are in the appendix
[1327/sources.md](1327/sources.md).

## Answer

Torlek already does what the better hosts do: a small brief delivered without a call, a
CLI-first interface with a tiny MCP server, messages collected at a turn boundary, structured
refusals on the wire, a sandbox, and approvals that never widen authority. The gains left are
specific:

1. **Batch messages into one wake.** Gas Town and Agent Orchestrator drain every pending
   notice into one turn and coalesce repeats. Torlek still wakes a session once per message
   ([#772](https://github.com/Androlax2/agent-cockpit/issues/772)).
2. **Do not file the same ask twice.** An identical `torlek ask run` sent while the first is
   pending becomes a second owner prompt. Beads and Agent Mail make repeats no-ops.
3. **Make refusals machine-readable at the CLI.** The wire carries a code and an unblock hint,
   but the CLI prints only the message and exits 1 for every refusal outside `ask`, so "busy,
   retry later" and "refused, do not retry" look the same to the agent.
4. **Put a budget on the manual.** The brief and the task packet have tested 2 KiB caps; the
   manual and its topics have none. In six days the manual grew 30 %. The `coordination`
   topic is 5,959 bytes, 3,719 of them coordinator verbs that workers read too.
5. **Measure cost per task.** Torlek archives cost per day, account and project. Gas Town,
   Roo Code and Agent Orchestrator attribute it to the task. Without that, no trial in #497,
   #103 or #788 can show a saving.

Six of the seven terminals and orchestrators read cut owner prompts by widening authority:
auto-yes, skipped permissions, or approving everything a harness asks. Torlek should not copy
that.

- [ ] Theo picks which [ranked proposals](#ranked-proposals) to file.

## Method and labels

- **Projects.** 24 sources, each pinned to the commit read on 2026-11-10: seven multi-agent
  terminals and orchestrators, five coordination layers, five agent platforms, three
  harnesses (Codex, OpenCode, Claude Code's public docs) and four protocols and tools (MCP,
  the Agent Client Protocol, Anthropic's sandbox-runtime, ccusage). Roo Code counts with the
  coordination layers for its orchestrator mode. The appendix lists every
  file cited. Only public repositories and documentation were read; no transcript, no private
  project and no downloaded code was run.
- **Torlek.** Source at `c2d3ecaa`; rendered text (the brief, the manual) from the installed
  Torlek 0.1.0-alpha.7, which is older than that source. #619 also measured rendered text, so
  the growth figures compare like with like.
- **Labels.** *Measured*: bytes counted here. *Project's claim*: a number the project states
  without a method this research could check. *Est.*: tokens as bytes ÷ 4, the rate
  [#619](2026-11-04-619-agent-token-efficiency.md#method-and-labels) used, about ±25 %.
- **Not done.** No provider was called and no live session was measured, so savings are
  estimates from code paths, as in #619.

## What changed since #619, #686 and #620

| Earlier report | Done since | Still open |
| --- | --- | --- |
| [#619](2026-11-04-619-agent-token-efficiency.md) token and turn efficiency | Constant stuck note (#769). A usage error prints only its verb's lines, 958 bytes measured (#770). `ask run` returns at most 16 KiB (#771). `brief --measure` counts what each harness loads (#775). | One wake per message (#772); writes echo whole records (#773); no per-pair message cap (#774); no budget tests for the placement line, confirmations and usage errors (#894); the send confirmation invites a poll (#895). |
| [#686](2026-11-04-686-compaction-research.md) compaction | The task packet, capped at 2,048 bytes, is re-injected with the brief at every context start (#781). Compactions are recorded (#782). The owner sets each harness's own threshold (#783). | Quality check (#784) and Torlek-sent compaction (#785), both gated on spend. |
| [#620](2026-11-04-620-agent-workflow-conventions.md) workflow conventions | Contradictions removed (#776); `CLAUDE.md` imports `AGENTS.md` (#777). | `repo-workflow` as the issue-to-merge path (#778), research README (#779), make-command test (#780), skill trials (#788, #896). |
| Coordinator watcher (not in the earlier reports) | Prints only lines to act on; the 10-minute summary is off by default (#1286). A full-list check every 3 minutes catches questions the watch stream missed (#1298). | |

**Growth.** The brief went from 1,118 to 1,670 bytes (+49 %), within its tested 2,048-byte
target (`tests/brief.rs`). The manual went from 9,862 to 12,830 bytes (+30 %), and the source
is larger again: its `sandbox` topic is 2,741 bytes against 1,449 installed. The largest topic
went from 3,270 bytes to 5,959 (`coordination`). No test bounds the manual or a topic; #894
covers the placement line, confirmations and usage errors only.

**Not covered before.** Other hosts, the CLI's error codes and exit statuses, duplicate asks,
cost per task, and harness limits that bound Torlek's text.

## Torlek today

| Point | Torlek | Number |
| --- | --- | --- |
| Start-time injection | Brief through `SessionStart` (Claude Code, Codex) or the OpenCode plugin, once per context and again after a compaction | 1,670 bytes, ≈420 tokens est.; 2,060 bytes always loaded (measured) |
| Instructions on demand | `torlek docs TOPIC`, MCP `torlek_docs`, `torlek --skill` | Index 566; topics 1,049 to 5,959; manual 12,830 bytes (measured) |
| MCP server | Two read-only tools (`torlek_docs`, `torlek_brief`) and the manual as resources; a refusal is an `isError` result with the unblock action | Schemas 487 bytes (#619) |
| Hooks | Nine Claude Code events reported; `SessionStart` and `Stop` collect messages in the background and wake the model with exit status 2 | No text printed except the brief and a collected message |
| Status | `agent report --label/--doing/--needs-input`; working, waiting, dialogs and compactions come from hooks | Per worker: 2 required calls (`todo start`, `todo done`) plus one `--doing` per phase (instruction-derived) |
| Messages | Collected at a turn end (Claude Code) or typed at the next prompt | 1 wake per message; at most 2,000 characters |
| Refusals | Wire: `code`, `message`, `unblock {action, by}`. CLI: `refused: MESSAGE` and `to unblock (BY): ACTION`, exit 1 | The code is not printed; exit 1 for every daemon refusal outside `ask` (which uses 124 pending, 125 declined, 126 unreachable) |
| Usage errors | Inside a verb: that verb's lines. Unknown top-level word such as `--version`: the whole usage | 958 vs 6,410 bytes (measured) |
| Approvals | Sandbox first; `ask run` approves one exact command once; network hosts per request or `--always` | No deduplication of identical pending asks |
| Coordinator | Watch stream plus a watcher script that prints only actionable lines; workers wait on CI in a shell loop | Skill 13,608 bytes; watcher 23,123 bytes; PR poll every 120 s and full-list check every 3 min, neither wakes the model |
| Measurement | Cost archive per day, account and project, deduplicated by reply key; quota windows; the harness's context size | Nothing per session or per task |

## Findings per area

### Context and tokens

**What others do.**

- **Brief size.** Torlek's brief is the smallest found:
  - Gas Town's role templates are 4.6 to 23 KB (measured).
  - Agent Orchestrator's brief is 10 to 18 KB (est. from source ranges).
  - Sculptor's is 4 to 5 KB.
  - Beads' `bd prime` is about 7 KB in CLI mode (est. from source).
- **Harness caps that bound Torlek's text.**
  - Codex caps hook output at 2,500 tokens and spills the rest to a file
    (`codex-rs/hooks/src/output_spill.rs`).
  - Claude Code caps `additionalContext` at 10,000 characters (hooks docs).
  - The brief fits both with room.
- **Lighter context after a compaction or resume.**
  - Gas Town prints one recovery line plus "Continue your current task", so agents stop
    re-announcing (`internal/cmd/prime.go`).
  - Beads caps variable sections and prints how many entries it dropped, "because hosts
    truncate silently" (`docs/cli-reference/prime.md`).
- **Load instructions when needed.**
  - Agent Orchestrator tells the agent to "read SKILL.md and only the relevant file under
    commands/*.md" (`session_manager/manager.go`).
  - goose and OpenHands load path rules when a tool first touches the path
    (`hints/load_hints.rs`, `agent_context.py`).
  - Ruflo caps agent prompts at 60 lines and moves reference tables to a file read on demand.
- **Stable prefixes keep the prompt cache.**
  - The MCP 2026-07-28 revision asks servers to order `tools/list` deterministically "to
    improve LLM prompt cache hit rates"; Ruflo sorts its tools for the same reason.
  - Claude Code's prompt-caching page lists what breaks the cache: a model or effort switch,
    an MCP server connected mid-session when tools load upfront, and compaction.
- **Output kept, pointer returned.**
  - OpenHands keeps the head and tail, writes the full text to a file and names the line where
    the cut starts (`utils/truncate.py`).
  - goose and Codex spill large outputs to a file.
  - Cline's cache-miss message says "DO NOT repeat side-effecting actions to recover output"
    (`tool-result-cache.ts`).
- **Measured MCP cost.**
  - Ruflo's `doctor` estimates `tools/list` tokens and warns at 8k tokens or 20 % of the window.
  - Beads claims MCP schemas "can add 10-50k tokens to every request" against 1-2k for
    `bd prime` (project's claim, no method).
  - Beads still registers all 18 tools, so its lazy-loading saving does not happen in the code.

**Torlek against it.**

- **Already right.** The brief and the packet are re-injected at every context start, which
  Claude Code needs: its context-window page says that at compaction the "context that hooks
  added earlier" is "summarized with the rest". The OpenCode plugin reads the brief once per
  context, so the system prompt stays byte-stable between compactions. The MCP server has two
  tools.
- **Gaps.**
  - The brief and packet have tested caps; no test bounds the manual or a topic.
  - Workers read the `coordination` topic for `agent report` and `todo`. Its coordinator
    sections ("Coordinating other sessions" and "Questions from sessions you started") are
    3,719 of its 5,959 bytes (measured).
  - `ask run` returns the last 120 lines (at most 16 KiB) and has no pointer to the rest. An
    agent that needs an earlier line must ask the owner to run the command again.

### Host interface, errors and refusals

**What others do.**

- **Refusals are data.**
  - Beads returns `{code, error, mutates}` as "a frozen contract" (`cmd/bd/errors.go`).
  - Its compare-and-set failure exits 13 with the expected and current revision, and its
    primer says "never retry blindly".
  - Agent Mail returns `{type, message, recoverable, data}` with about 30 codes, and checks
    arguments before any side effect (`app.py`).
  - Agent Orchestrator separates exit statuses 2, 75 and 1.
- **Refusals name the next step.**
  - container-use's error names the next call: "You can environment_open X…, or set
    allow_replace=true" (`mcpserver/tools.go`).
  - Codex's execpolicy README asks that a `forbidden` rule name an alternative ("Use `jj`
    instead of `git`").
  - sandbox-runtime puts the reason for a denied domain into the 403 so "whoever reads [it] (a
    model included) learns why and what to do instead".
- **Loop breakers.**
  - OpenHands nudges once after 3 identical failing calls, then stops the run
    (`stuck_detector.py`).
  - Cline warns at 3 identical calls and stops at 5.
  - Roo Code blocks the third.
  - SWE-agent's lint guard says "DO NOT re-run the same failed edit command".
- **Idempotent requests.**
  - Beads' `bd ready --claim` repeated by the same agent is a no-op.
  - Agent Mail's file reservations return the conflicts and their holders instead of
    queueing.

**Torlek against it.**

- **On the wire, Torlek is as good as any of these.** Every refusal carries a code from a
  known list (an unknown one is kept as sent) and an unblock action naming who can lift it
  (`cockpit-protocol/src/error.rs`), and the MCP server returns it as an `isError` result.
- **The CLI, which is what agents use, drops half of it.**
  - It prints `torlek: refused: MESSAGE`, without the code, and exits 1 for every daemon error
    (`cockpit-cli/src/agent.rs`, `failed`).
  - So `busy` ("retrying later may work") and `refused` (a safety rule) differ only in prose.
- **Asks.** `ask run` files a new request each time (`cockpit-approvals/src/runs.rs`,
  `ask_run`). Only this repository's coordinator skill says "never send a second one"; the
  manual says only that `--wait ID` waits again. The daemon does not
  enforce it.
- **Usage errors.** An unknown first word prints the whole 6,410-byte usage. `torlek --version`
  is a likely first guess, and the verb is `torlek version`.

### Status and coordination

**What others do.**

- **Delivery at a turn boundary, batched.**
  - Gas Town queues notices in files that the `UserPromptSubmit` hook drains into one
    reminder.
  - The queue holds at most 50 per session; normal notices live 30 minutes and urgent ones
    2 hours.
  - A newer notice on the same thread replaces the queued one.
  - The batch reads "This is a background notification. Continue current work unless the
    nudge is higher priority" (`internal/nudge/queue.go`).
  - Mail shows only `id from sender: subject` lines (`mail_check.go`).
  - Its design doc says to default to the light channel and use mail only when the message
    must survive the session.
- **Priority tiers for the coordinator.**
  - Agent Orchestrator delivers needs-input and stuck at once, holds "done" for a settling
    window, batches information, and coalesces repeats as "(repeated N times)".
  - It allows one urgent interruption per worker per 3 minutes (`report/coordinator.go`).
  - Sculptor's CI babysitter wakes an agent only on a failure or a merge conflict, deduplicated
    by pipeline, and never while the agent is busy (`ci_babysitter_service/coordinator.py`).
- **Self-sufficient nudges.** Agent Orchestrator's CI nudge carries a 20-line log tail with
  "You should not need to re-fetch", deduplicated by a stored signature and capped at
  3 attempts (`lifecycle/reactions.go`).
- **Low-noise reminders.** Agent Mail's inbox hook runs at most every 120 s and prints one line
  with no message bodies (`scripts/hooks/check_inbox.sh`).
- **Summaries only, between agents.** Roo Code's orchestrator has no tools; a subtask's result
  returns as one tool result and the child's transcript never enters the parent
  (`ClineProvider.ts`).
- **Events instead of polling.**
  - The harnesses expose events: Claude Code `Notification` and `Stop`, OpenCode
    `session.status` and `session.idle`, and the Agent Client Protocol's `session/update`.
  - Claude Squad is the counter-example: it hashes every tmux pane every 500 ms.

**Torlek against it.**

- **Already right.** Messages reach Claude Code at a turn end through a background hook, never
  typed into a running turn; Gas Town gives the same reason for its queue. Worker CI waits and
  the coordinator's PR poll run in shell loops that do not wake the model. Since #1286 the
  watcher prints only lines to act on.
- **Gaps.**
  - Still one wake per message, with no time-to-live, no per-recipient cap, no replacement by
    thread and no "continue current work" framing (#772, #774).
  - The prioritising lives in a 23 KB Python script in this repository's skill, not in Torlek,
    so other projects' coordinators do not get it.
  - The script also needs a 3-minute full-list check because the watch stream can miss a
    question (#1298).

### Approvals and sandbox friction

**What others do.**

- **Sandbox first.**
  - "In our internal usage, we've found that sandboxing safely reduces permission prompts by
    84%" ([Anthropic](https://www.anthropic.com/engineering/claude-code-sandboxing); its own
    number). Claude Code then auto-allows sandboxed shell commands.
  - Codex's `prefix_rule` resolves allow, prompt or forbidden by the strictest match.
- **Narrow remembering.**
  - The Agent Client Protocol's `allow_always` belongs to the agent, an unknown outcome is
    never approval, and the agent asks again when the target changes.
  - Aider's `--yes` still answers no to model-suggested shell commands.
  - goose caches a "not read-only" verdict per tool.
  - Cline denies after 5 minutes without an answer.
- **Counter-examples.** Six of the seven terminals and orchestrators read remove prompts by
  widening authority; container-use is the exception:
  - Claude Squad presses Enter on any prompt.
  - Crystal defaults to skipped permissions and falls back to them silently.
  - Agent Farm's setup runs Claude Code with `--dangerously-skip-permissions`.
  - Vibe Kanban's default profiles skip permissions.
  - Sculptor auto-allows on a "sandboxed" claim the code does not back.
  - Agent Orchestrator's Codex default bypasses approvals and the sandbox.

  Elsewhere, Gas Town sets `bypassPermissions` and OpenHands' bridge approves every request
  from a hosted harness.

**Torlek against it.** Torlek's model is the strictest read: a sandbox, one exact command
approved once, network hosts per request. The friction worth cutting is measured from real
sessions by [#1234](https://github.com/Androlax2/agent-cockpit/issues/1234), which follows
[#1233](https://github.com/Androlax2/agent-cockpit/issues/1233). This report hands it the
sources above and proposes only the duplicate-ask fix, which widens nothing.

### Measurement

**What others do.**

- **Per task.**
  - Gas Town's `Stop` hook prices each turn's tokens from the transcript and can tag the entry
    with a work item (`internal/cmd/costs.go`).
  - Roo Code sums own, child and total cost per task (`aggregateTaskCosts.ts`).
  - OpenHands stops a run at `max_budget_per_run`, counting the condenser's calls too.
  - SWE-agent's default cost limit is $3 per instance.
- **Deduplication.**
  - Agent Orchestrator counts each `message.id` once after `stop_reason` and keeps cache tiers
    (`observe/usage/parser.go`).
  - ccusage drops replayed sidechain messages and subagents' copies of the parent history.
  - Crystal is the counter-example: it gives every tool of a message the whole message's usage.
- **What the harnesses report.**
  - Codex `exec --json` emits per-turn `usage` with cached and reasoning tokens.
  - OpenCode keeps provider-reported cost ("a reported zero is authoritative").
  - Claude Code's OpenTelemetry metrics attribute tokens by skill, plugin, agent and MCP server.
  - The Agent Client Protocol streams `usage_update {used, size, cost}`.
- **Caution.** Ruflo measured delegation and found the coordinator's payload fell 97.6 % while
  total bytes rose 3.8 %. It states it makes "no claim about … billed savings"
  (`docs/benchmarks/context-delegation.md`).

**Torlek against it.** Torlek archives cost per day, account and project, deduplicated by
reply key (`cockpit-usage/src/cost.rs`, #699), the same rule ccusage uses. It does not
attribute cost to a session or to the item a session worked on. Every saving in this report,
and the trials of #497, #103, #788 and #896, needs that attribution to be shown.

## Ranked proposals

Ranked by expected saving per hour of work. Times are for one agent with focused tests.
Savings are from code paths (*est.*), not from provider usage.

| # | Proposal | Expected saving | Cost | Risk | Issues |
| --- | --- | --- | --- | --- | --- |
| 1 | Deliver every message queued for a session in one wake, as Gas Town does. Lead with "Continue current work unless this changes it". Add a time-to-live and a per-recipient cap; a newer message from the same sender replaces a queued one | N wakes → 1 per burst. Each wake re-sends the whole conversation (cached) | 1 day (#619's two half-day estimates) | A wire change (`InboxView::Message` carries one message); a replaced message must stay audited | #772, #774, #895 |
| 2 | An identical `ask run` (same session, command and directory) sent while one is pending returns the pending id and "already asked, wait with `torlek ask run --wait ID`" | 1 owner prompt per duplicate | 2–3 h | The second reason text is dropped; a decided ask is never matched, so a real rerun still asks | #1234 |
| 3 | The CLI prints the refusal's code (`refused [busy]: …`) and a distinct exit status for "retry later" (`busy`: 75, as Agent Orchestrator and sysexits use) apart from final refusals, clear of `ask`'s 124–126. An unknown code counts as final. A refusal that changed nothing says so | One retry turn per misread refusal; scripts can branch without parsing prose | 2–3 h | Callers that test for exit 1, including the coordinator watcher, which passes Torlek's status through | None |
| 4 | Add byte budgets for the manual and each topic, beside the brief's, and split `coordination`: worker verbs stay; "Coordinating other sessions" and "Questions from sessions you started" become a `coordinating` topic for coordinators | 3,719 bytes (≈930 tokens est.) for each worker that reads the topic; stops the growth measured above | 3 h | A coordinator must read two topics; the coordinator skill points to the new one | #894, #208 |
| 5 | `ask run` writes the whole redacted output to a file in the session's temp folder and returns the head, the tail and "full output at PATH; do not re-run the command to see more" | 1 owner prompt each time an agent needs lines the tail cut | 3–4 h | The file needs the same redaction as the tail (#872) and the retention of run outputs (#406) | #771, #406, #1234 |
| 6 | Attribute archived replies to the session and to the task item it started (`todo start`), and roll up per item. Mark replies with no item "unattributed" | None directly; makes every other saving and #497's trials measurable without reading transcripts | 1–2 days | Attribution is by time window; a session that switches items mid-turn splits imprecisely | #699, #497, #103, #788, #896 |
| 7 | An actionable mode of `torlek agent sessions --watch`: print questions, dialogs, ends and idles past a grace period at once; coalesce repeats; never print `doing` changes. The watcher's session logic moves into tested code | No change in wakes today. Any coordinator, not only this repository's, gets the filter; the 3-minute full-list fallback goes once #1298's misses are fixed | 1–2 days | Fixes the policy in Torlek; the idle grace becomes configuration | #208, #1298 |
| 8 | An unknown first word prints one line naming `torlek version` or `torlek` for the verbs, not the 6,410-byte usage | ≈1.5k tokens est. per occurrence | 1 h | None found | #770 |
| 9 | A repeat guard: the third identical refused request from a session gets one line, "refused 3 times; stop and ask the owner" (OpenHands, Cline, Roo Code) | Bounds a refusal loop; how often loops happen is unknown | 3–4 h | Reads and `--wait` must be exempt, or legitimate polling trips it | None (#774 is the same idea for messages) |

**Recommendation.** File 2, 3 and 8 now: small and self-contained. 1 is already filed (#772);
fold the time-to-live, cap and framing into its scope. 4 sits beside #894. 6 before any token
trial. Decide 7 inside #208's design. Keep 9 until 6 shows refusal loops happen.

**Not ranked: OpenCode 2.** OpenCode's V2 migration guide says "V1 plugin implementations do
not run in V2" (`services/site/src/docs/content/migrate-v1.mdx`). Torlek's plugin uses the V1
system-prompt hook. Torlek tests OpenCode 1.19.5 only. On 2.x its start-up probe finds
`--port` missing and withdraws every capability, so nothing breaks unseen. Supporting 2.x
needs the plugin ported to the V2 `context` hook, and `codemode: false` for Torlek's MCP
server so its two tools stay direct. No open issue tracks it. It touches
[#732](https://github.com/Androlax2/agent-cockpit/issues/732) and
[#419](https://github.com/Androlax2/agent-cockpit/issues/419).

## What not to copy

- **Prompt cuts that widen authority.** Auto-yes, skipped permissions, `bypassPermissions`,
  approving every request a hosted harness makes: the list under
  [approvals](#approvals-and-sandbox-friction).
- **Large MCP surfaces.** Agent Mail's 51 tools and Beads' 18 cost schema bytes in every
  request unless a client defers them. Torlek's CLI-first surface with two read-only MCP tools
  is the cheaper design.
- **Compaction summaries with no bound.** Crystal rebuilds context from its database with no
  size cap. Torlek's packet is capped at 2,048 bytes.
- **Delegation as a token saving.** Ruflo's measurement shows it moves bytes rather than
  removing them. #103 already found no evidence for standing teams.
