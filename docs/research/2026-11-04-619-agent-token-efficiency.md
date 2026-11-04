# Agent token and turn efficiency

Issue [#619](https://github.com/Androlax2/agent-cockpit/issues/619).
**Research and proposal, 2026-11-04; Theo has not decided.** Merging this document accepts
research delivery only. It does not authorize any behavior change; each plan item below waits
for Theo's approval, item by item.

## Answer

What Torlek loads into every conversation is already small: 1,394 bytes, measured.
Most of the waste is in **turns**, not prompt bytes. Every extra turn sends the whole
conversation to the model again, so one avoided turn saves more than any trim to the brief.

There are three top-ranked problems:

1. **Messages between sessions**: one wake-up per message, with no batching, rate limit or
   loop guard.
2. **The `--watch` clock**: `agent sessions --watch` reprints a "stuck" line every 2 seconds
   because the line includes a seconds counter.
3. **Echoes and extra reads**: each workflow and todo action prints the whole record back,
   and `workflow begin` makes the agent ask for the packet in a second call.

The fixes are small, local and testable. None of them drops any fact an agent needs.

- [ ] Theo approves, changes or declines each item of the [plan](#prioritized-plan).

## Method and labels

- **Source.** Examined at [`fb683e93`](https://github.com/Androlax2/agent-cockpit/tree/fb683e93), under `crates/` only.
- **Payloads.** Rendered by the installed `torlek` (Torlek 0.1.0-alpha.2). Its instruction
  bundle matches the source at `fb683e93` byte for byte, apart from the release stamp.
- **No live conversations.** No owner conversation, transcript or provider usage record was
  read. Nothing was built and no provider was called.
- **Labels.**
  - *Measured*: bytes of real rendered output.
  - *Template*: bytes computed from the format strings in the code, with representative
    field values.
  - *Est.*: tokens estimated as bytes ÷ 4. That is a rough rate for English Markdown, about
    ±25%. It differs by provider and tokenizer, and none was available on this host.
- **What was not measured.** Real token counts, cached-token use and the harness's own
  context (system prompt, tool schemas). Measuring those belongs to
  [#479](https://github.com/Androlax2/agent-cockpit/issues/479), which this research does not
  duplicate.

The appendix [payloads.md](619/payloads.md) has the full inventory, the before/after payloads
and the commands used for each measurement.

## Baseline

### What Torlek adds to every conversation

| Item | Bytes | Label | Est. tokens | Delivery |
| --- | ---: | --- | ---: | --- |
| Launch brief | 1,118 | measured | ≈280 | Claude Code and Codex: `SessionStart` `additionalContext`. OpenCode: appended to the system prompt on every model call. |
| Skill catalog entry | 276 | measured | ≈70 | Claude Code and OpenCode list the skill. Codex does not. |
| MCP `instructions` | 114 | measured | ≈30 | All three harnesses |
| MCP tool schemas | 487 | measured | ≈120 | All three. Claude Code defers them in this harness, so only the tool names load until a tool is used. |
| MCP resource list | 1,003 | measured | ≈250 | Only when a harness lists resources |

- **Always loaded.** 1,394 bytes, matching `torlek brief --measure`. Add up to 601 bytes when
  the harness loads the MCP schemas eagerly.
- **Brief size.** The brief is within its 2 KiB target (`BRIEF_TARGET_BYTES`).
- **Read on demand.** The whole manual is 9,862 bytes. One topic is 987 to 3,270 bytes.

### Per workflow

Turn counts come from the code paths. They are not counts from live runs.

| Workflow | What Torlek adds | Extra turns Torlek causes |
| --- | --- | --- |
| Quiet, unchanged period | Nothing. The hooks print nothing except at `SessionStart`. The message waiter is silent until a message arrives. There are no reminders, nudges or pushed approval results. | None for a worker. A coordinator watching `agent sessions --watch` is woken by every state change of every session, and every 2 s while a message is stuck (§2). |
| Ordinary task, `code-change` preset | A placement line of 141 to 152 bytes. Then the packet, each checkpoint and the handoff. | The placement points to the packet: 1 read turn. `begin` does not print the packet: 1 more read turn. Each checkpoint or complete prints the whole workflow back: about 1.1 KB, up to 3.3 KB (template). |
| Error | A refusal is 1 to 2 lines, with an echo cap of 256 bytes. | Any argument mistake prints the whole usage: 5.3 KB, 85 lines, measured. The verb's own lines are 134 to 936 bytes. |
| Recovery: compaction, `/clear`, resume | The brief again, 1,118 bytes. This is necessary, because compaction drops it. | None. Note that the skill's "Start here" step 1 tells the agent to run `torlek brief`, which it already has. This researcher did exactly that (one observation). |
| Multi-agent coordination | 77 bytes of prefix plus up to 2,000 characters per message. | 1 turn per message. N queued messages take N turns. Nothing prevents two agents acknowledging each other back and forth (§1). |

**Who adds what.** Everything above is Torlek's addition. The harness's own instructions,
tool schemas and generated output are the provider's to count (#479). They are much larger,
and every extra turn sends them again.

## Ranked waste

Ranked by how often the cost occurs times its cost. Each claim is anchored to a line under
`crates/`.

1. **One turn per message, with no batching, rate limit or loop guard.**
   - **Cost.** Every message costs a full turn for its recipient.
   - **One per wake.** `collect` hands over the first received message and returns
     (`cockpit-daemon/src/coordination.rs:437-472`), so N queued messages wake the
     recipient N times.
   - **No caps.** Starts have an hourly cap. Messages have no per-hour or per-pair limit
     (`message()`, `:274-395`).
   - **Reply loops.** The payload never says whether a reply is wanted, so two agents that
     acknowledge each other can keep waking each other.
   - **Extra poll.** The sender's confirmation points to `torlek agent sessions`, which
     invites a follow-up poll (`cockpit-cli/src/coordination.rs:84-101`).
2. **A clock in a watched line.**
   - **The bug.** The "stuck" note embeds `{waited} s`
     (`cockpit-daemon/src/coordination.rs:1226-1232`). `--watch` reprints any line that
     differs (`cockpit-cli/src/coordination.rs:216-229`), every 2 s.
   - **Cost.** A 227-byte note on a line of about 380 bytes, 1,800 times an hour, for as long
     as the message stays stuck. A coordinator's Monitor is woken by every reprint, or is
     stopped for flooding.
   - **Ordinary churn.** Every working/waiting flip of any other session also reprints that
     session's line.
3. **Action replies echo the whole record.**
   - **Workflows.** `begin`, `checkpoint`, `complete`, `release` and `block` each return the
     whole workflow (`cockpit-daemon/src/workflows.rs:409-457`, rendered by
     `cockpit-cli/src/workflow.rs:219-238`). That includes every step's checkpoint note, up to
     1,000 characters each.
   - **Todos.** Every todo action returns the whole item, with notes of up to 4,000
     characters (`cockpit-cli/src/todo.rs:322-368`). That echoes back text the agent just
     wrote.
   - **Extra read.** `begin` does not print the packet, so a step costs one more read turn.
   - **Keep.** The echo's revisions, `--step/--attempt` and `next:` are needed and must stay.
4. **The whole usage on any argument error.** `usage_error` prints all 85 lines
   (`cockpit-cli/src/main.rs:303-307`), owner verbs included: 5,348 to 5,361 bytes,
   measured. This costs about 1.3k est. tokens per mistake. The relevant verb's lines would
   be 2% to 18% of that.
5. **The placement line is only a pointer.** "Torlek gives you step … read its task packet
   with `torlek workflow packet {run}`" (`cockpit-scheduling/src/model.rs:288-300`) costs a
   guaranteed read turn. That is a fair trade: the line stays short and never goes stale.
   Combined with item 3, though, a step takes two read turns where one would do.
6. **Output with a line limit and no byte limit.**
   - **The gap.** A finished `ask run` returns the last 120 lines from a 1 MiB tail with no
     byte cap (`cockpit-approvals/src/pty.rs:96-97`, `cockpit-cli/src/ask.rs:323-342`).
     Minified or long-line output can reach the agent nearly whole.
   - **The contrast.** `check run` already caps its output at 16 KiB.
7. **The same rules in two places, and a redundant first step.**
   - **Rules.** The brief's three rules (≈430 bytes) and the skill's "Rules that nothing
     overrides" (549 bytes) say the same thing. Both are in context once the skill loads.
   - **Step 1.** "Start here" step 1 asks for `torlek brief`, which already arrived.
   - **Cost.** Low and once per context, but step 1 can cause a turn.
8. **The packet grows with attempts.**
   - **What grows.** It prints every handoff of every attempt (no `LIMIT`,
     `cockpit-workflows/src/runs.rs:500-511`; up to 2 KiB each) and every decision, then
     the item's notes.
   - **Bound.** The presets have 3 steps, which keeps it in check. A rework loop makes it
     grow.

**Already efficient, keep as is:**

- **Delivery.** The brief arrives without a turn. Hooks print nothing for events other than
  `SessionStart`. Compaction re-injects only the brief.
- **Pull, not push.** Approval results are pulled, not pushed
  (`cockpit-agent-channel/src/answers.rs`). There are no reminder or nudge paths.
- **Scheduling.** Placements wait 5 minutes of idle time and are never redelivered.
- **Bounded output.** Refusals are terse. The check gate is bounded by its checks.

Sandbox denials carry no Torlek text: the kernel or greyproxy words them.

## Proposed response and wake rules

These are proposals, for approval.

1. **Wake only for actionable input.** A session wakes for owner input, new work, or messages
   addressed to it. It never wakes for another session's status unless it asked to watch.
2. **One wake hands over all queued messages.** The batch is bounded to 8 KiB and delivered in
   order. Each message keeps its sender and its "not the owner" label.
3. **Messages say whether a reply is wanted.** An acknowledgement-only reply is never
   required. A message that only informs needs no answer. The coordination topic says so
   once, so each message does not have to.
4. **Watched output changes only on transitions.** It changes on state, question, delivery
   or ended, never on a clock. A "stuck" line appears once, when the message becomes stuck.
5. **Writes confirm; reads print.** A changing verb prints one line: what changed, the ids,
   revisions or `--step/--attempt` the next command needs, and `next:`. The full record stays
   one read away (`show`, `packet`).
6. **Push a pointer only when it saves more than the read it causes.** `workflow begin`
   prints the packet, so placement → begin → work takes a single read.

**Proposed budgets** (bytes of Torlek text per delivery; measured by tests):

| Delivery | Budget |
| --- | --- |
| Brief | ≤2 KiB (exists) |
| Placement line | ≤300 bytes |
| Confirmation of a change | ≤300 bytes |
| Usage error | that verb's lines only, ≤1 KiB |
| Message batch | ≤8 KiB |
| Agent-facing command output (`ask run`, `check run`) | ≤16 KiB |
| Packet | ≤8 KiB: latest handoff per step, plus a count of earlier attempts |

## Prioritized plan

Owners are crates. Times are for one agent with the focused tests. Savings are from the
templates above (*est.*), not from provider usage.

| # | Change | Owner | Expected saving | Trade-off | Regression check | Time |
| --- | --- | --- | --- | --- | --- | --- |
| P1 | Remove the clock from the stuck note; say "stuck since HH:MM" or only the transition | cockpit-daemon `coordination.rs` | Up to 1,800 reprints/hour → 1 | Exact wait seconds are no longer shown on each line | Daemon/CLI test: a stuck message prints one `--watch` line across several polls | 30 min |
| P2 | `collect` hands over every queued message for the target in one wake, ≤8 KiB, in order | cockpit-daemon `coordination.rs`, `cockpit-claude` hook | N wake turns → 1 per burst | A larger single input; a batch must fit the bound | Fixture: 3 queued messages → one inbox reply holding all 3, each with its sender | 2 h |
| P3 | Changing workflow and todo verbs print one confirmation line; `workflow begin` prints the packet | cockpit-cli `workflow.rs`, `todo.rs` | About 1.0–3.2 KB per workflow write, up to about 1.6 KB per todo write; 1 read turn per step | Agents relying on the echo must run `show` | CLI tests: the line keeps id, revisions, `--step/--attempt`, `next:`; `begin` output equals `packet` | Half a day |
| P4 | A usage error prints the verb's own usage lines, plus "`torlek` alone lists every verb" | cockpit-cli `main.rs` | About 4.4–5.2 KB per mistake | None found | CLI test per verb family: an error prints only that family's lines | 1–2 h |
| P5 | Byte-cap `ask run`'s returned tail (16 KiB, like `check run`), keeping the "not shown" line | cockpit-approvals `pty.rs` | Bounds a rare, large tail | Long single lines get cut | Test: 120 lines of 10 KiB each return ≤16 KiB plus the omission line | 1 h |
| P6 | Message rate limit and loop guard. **Needs Theo's policy**: Option A is a per-pair hourly cap like starts; Option B holds a third consecutive acknowledgement-only exchange; Option C is wording only (rule 3) | cockpit-daemon, cockpit-authorization | Stops runaway loops | A cap can hold a real burst | Fixture: two sessions exchanging messages stop at the cap with a visible refusal | 2–4 h after the decision |
| P7 | Skill "Start here" step 1 says the brief arrives at start (`torlek brief` reprints it); keep the rules in the brief and point to them from the skill | cockpit-agent-context `bundle/SKILL.md` | ≈550 bytes once per context; avoids one turn | The rules appear in one place, not two | `tests/bundle.rs`, plus `cockpit-cli` `tests/instructions.rs` (verbs still resolve) | 30 min |
| P8 | Packet shows the latest handoff per step and counts earlier attempts | cockpit-workflows, cockpit-cli | Bounds rework loops | Older attempts need `workflow show` | Workflow test with 3 attempts → 1 handoff line plus a count | Defer until rework frequency is known (#103, #479) |

**Recommendation.** Approve P1, P4 and P5 now (bug-level, no trade-off). Then P2, P3 and P7.
Decide P6's option. Defer P8.

## How to check that agents still get the same information

- **Fact checks.** Before and after each change, a test lists the facts a payload must carry
  (ids, revisions, sender, "not the owner", next action, evidence references) and asserts each
  is still present. Golden byte counts enforce the budgets above.
- **End-to-end.** `cockpit-daemon` `tests/agent_onboarding.rs` (a clean-home agent finds its
  workflow, checks and approval route) must pass unchanged. The provider conformance fixtures
  must still deliver the brief at every `SessionStart`.
- **Turn counts.** The fixtures count turns, not only bytes: wake-ups per message burst (P2)
  and read calls from placement to first work (P3).
- **Provider tokens.** Once #479 reports harness totals, the same fixtures record real tokens
  per provider, kept apart from bytes and labelled as cached or uncached.

## Coordination

| Issue | What this research leaves to it |
| --- | --- |
| [#419](https://github.com/Androlax2/agent-cockpit/issues/419) | The delivery channels, including whether MCP stays on harnesses that already load the skill |
| [#479](https://github.com/Androlax2/agent-cockpit/issues/479) | Real token measurements |
| [#103](https://github.com/Androlax2/agent-cockpit/issues/103) | Worker roles and rework rates |
| [#208](https://github.com/Androlax2/agent-cockpit/issues/208) | Watch feeds for coordinators: rules 1 and 4 apply |
| [#497](https://github.com/Androlax2/agent-cockpit/issues/497) | Model choice |
| [#618](https://github.com/Androlax2/agent-cockpit/issues/618) | Document conventions |
