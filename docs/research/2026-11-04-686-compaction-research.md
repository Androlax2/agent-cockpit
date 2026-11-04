# Compact an agent's context before the harness does

Issue [#686](https://github.com/Androlax2/agent-cockpit/issues/686).
**Research and proposal, 2026-11-04; Theo has not decided.** Merging this document accepts
research delivery only. It authorizes no product code, no launch setting, no live provider
spending and no follow-up issue; the issues proposed at the end are not filed.

## Recommendation and decision still needed

**Do not build a Torlek-owned "compact at X %" trigger yet.** The measurements show a real
problem, but they also show that Torlek can fix most of it with smaller, safer pieces, and that
a trigger which waits for a safe moment would leave about one cycle in three to the harness.

1. **Make a compaction harmless first.** Every harness already re-runs a hook (or plugin) after a
   compaction, and Torlek already uses it for the launch brief. Extend that answer with a bounded
   task packet: goal, step, latest checkpoint, last handoff. The harness summary kept only 48 % of
   the issue and PR numbers, 51 % of the branch names and 36 % of the commit hashes it had
   seen (Claude Code, 120 summaries). Torlek holds those facts exactly and need not ask a model
   to remember them.
2. **Make compactions visible.** Torlek does not record one today: both bridges turn a
   post-compaction `SessionStart` into an unknown event. Report each compaction (trigger, tokens
   before and after, seconds) and show a note on the session card.
3. **Let the owner set the threshold through each harness's own knob**, off by default: Codex
   compacts after a final response at a set percentage of its window
   (`model_post_turn_compact_threshold_percent`), OpenCode 2 keeps a share of the window free
   (`compaction.buffer`), Claude Code takes a window (`--autocompact`). Only Codex's is a safe
   moment by construction.
4. **Gate a Torlek-sent compaction** (Option B below) behind a quality check that the research
   could not run without live provider spending. Do not replace a harness's summary prompt, and do
   not default to a fresh session with a handoff (Option C).

- [ ] Theo decides whether to file the five issues listed under
  [Proposed follow-up issues](#proposed-follow-up-issues-not-filed), and whether to authorize the
  quality check (it spends provider tokens).

Until Theo decides, the harness defaults stay as they are.

## What was measured

Method, scripts and limits are in [the measurement appendix](686/measurements.md). All figures
come from the harnesses' own logs on this machine, read by script. Only token counts, timestamps,
record types and counts of references were taken; no message text, path or identifier is
recorded here, and nothing the harnesses stored as text was copied. They are Theo's sessions of all kinds, not only Torlek-launched ones. They are
observations, not a benchmark; no live provider call was made for this research.

### How full sessions get, and when each harness compacts today

| | Claude Code | Codex | OpenCode |
| --- | --- | --- | --- |
| Sessions with usage / period | 334, 2026-10-07 to 11-04 | 81, 2026-10-08 to 11-03 | 566, older models mostly |
| Peak context, p50 / p90 / max (tokens) | 165,116 / 280,885 / 967,920 | 106,881 / 244,638 / 449,775 | 50,546 / 165,100 / 967,603 |
| Sessions that compacted | 82 (25 %) | 15 (19 %) | not counted per session |
| Compactions | 120: 112 automatic, 8 manual | 55 | 33 (30 flagged automatic) |
| Context when it fired, p50 | 269,001 tokens (automatic) | 243,752 tokens, 94.3 % of the window | 169,819 tokens |
| Fired while work was running | 89 of 102 right after a tool result; 7 right after a user prompt, 4 after assistant text, 2 unknown | 54 of 55 inside a turn | 19 of 33 after a step that ended in tool calls; 14 unknown |
| Context after, p50 | 23,678 tokens | 30,525 tokens | summary of 1,686 output tokens |
| Time, p50 | 76 s (p90 106 s) | not recorded | 86 s |

- **The premise holds.** In 89 of the 102 Claude Code automatic compactions that a model call
  followed (87 %; 10 of the 112 had none) the previous record was a tool result: the agent was
  still working. In Codex a turn was open for 54 of 55 (98 %).
- **Claude Code's threshold is Theo's setting.** 78 of the 112 automatic compactions fired between
  215,000 and 317,000 tokens, mostly at 266,000 to 273,000: 89 % of the `autoCompactWindow` of
  300,000 in `~/.claude/settings.json`. 34 fired above 760,000, mostly near 968,000, the documented
  default for models with a native 1M window
  ([default thresholds](https://code.claude.com/docs/en/model-config.md#default-auto-compact-thresholds)).
- **Codex runs close to its limit.** Its 55 compactions all fired at 90.6 to 95.2 % of the window,
  15 sessions produced them, and 16 of 81 sessions reached 90 % of the window. The harness does
  not leave a margin to choose a moment.
- **A compaction is a pause of over a minute.** Claude Code's 102 measured automatic compactions
  took 2.2 hours together. The first model call after one wrote a median of 48,864 tokens to the
  prompt cache, against a post-compaction context of 23,678 tokens.
- OpenCode's database does not store a model's window, so no percentage is given for it.

### Is there a safe moment before the harness compacts?

For the 78 automatic compactions on Claude Code's 300,000-token window, did an assistant turn end
(`end_turn`, the agent back at its prompt) after the context crossed a threshold and before the
harness compacted anyway?

| Threshold | Cycles with a turn end first | Tokens past the threshold at the first turn end, p50 / p90 | Seconds after crossing, p50 / p90 | Model calls left before the harness compacts, p50: all cycles (cycles with no turn end) |
| --- | --- | --- | --- | --- |
| 50 % (150,000) | 52 of 78 | 41,140 / 88,934 | 502 / 1,627 | 63 (50) |
| 60 % (180,000) | 52 of 78 | 29,856 / 63,872 | 323 / 1,037 | 50 (38) |
| 70 % (210,000) | 52 of 78 | 16,970 / 40,950 | 183 / 713 | 34 (26) |
| 80 % (240,000) | 42 of 76 | 12,526 / 25,316 | 113 / 472 | 16 (13) |

A model call is one API response; the transcript writes several records for one response, which
the script merges (a cycle is 99 calls at the median).

- **A rule "at 60 %, compact when idle" would have caught 67 % of cycles** (52 of 78), at a median
  of 70 % of the window, five minutes later. The other 26 cycles ran a median of 38 more model
  calls with no turn end. Those still reach the harness's compaction mid-turn.
- Of 2,512 turns that ended with at least 150,000 tokens of context and were followed by a user
  prompt, 449 (18 %) waited five minutes or more for it, 50 an hour or more and 10 eight hours or
  more. Claude
  Code's own idle compaction acts on the one-hour cases (below).

### What the harness summary loses

Claude Code keeps its summaries in the transcript, so references can be counted. For each of 120
compactions: of the concrete references the conversation had mentioned, how many appear again in
the summary (median summary: 19,032 characters)?

| Reference | Mentioned before | Appear in the summary | Older half / newer half of the mentions |
| --- | --- | --- | --- |
| Issue and PR numbers (`#123`) | 1,939 | 934 (48 %) | 35 % / 61 % |
| Branch names | 251 | 129 (51 %) | 47 % / 55 % |
| Edited file names | 771 | 584 (76 %) | 69 % / 81 % |
| Commit hashes | 752 | 270 (36 %) | 25 % / 45 % |

This is a proxy: a reference missing from a summary may be one nobody needs again, and a present
one may be misdescribed. It does show the shape of the loss: exact identifiers fade, and older
ones fade faster. The task, decisions and open questions the issue asks about live in prose,
so a count cannot grade them. Codex and OpenCode summaries were not counted: OpenCode keeps them
in the same private database, and Codex's sit in a replacement-history field that was not parsed.
The documented survival rules are the other half of the answer:

| After a Claude Code compaction ([source](https://code.claude.com/docs/en/context-window.md#what-survives-compaction)) | |
| --- | --- |
| Re-injected from disk | project-root `CLAUDE.md`, auto memory, skill descriptions, the plan-mode plan |
| Re-run | `SessionStart` hooks that match the `compact` source, adding their output |
| Re-read, at most five files | files read or edited, most recent first; over 5,000 tokens comes back as a path |
| Capped | invoked skill bodies, 5,000 tokens each and 25,000 in total |
| **Summarized with everything else** | **context that hooks added earlier**, path-scoped rules, nested `CLAUDE.md` |

That last row matters to Torlek: anything it delivered once (a queued message, an approval
answer, a packet the agent fetched) is summarized away, but anything a `SessionStart` hook
returns again survives exactly.

## What each harness lets Torlek do

Documentation read on 2026-11-04. Installed here: Claude Code 2.1.318, Codex 0.166.0, OpenCode
v2.2.5. Names marked ✓ were found in the installed binary; none was exercised.

| Need | Claude Code | Codex | OpenCode 2 |
| --- | --- | --- | --- |
| Native percentage or window | `autoCompactWindow`, `--autocompact`, `CLAUDE_CODE_AUTO_COMPACT_WINDOW` (100k to 1M tokens); `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` can only lower it ✓ | `model_auto_compact_token_limit` (+ `_scope`) ✓; **`model_post_turn_compact_threshold_percent`: compact after a final response** ✓ | `compaction.buffer` ("15%": share of the window kept free), `compaction.keep.tokens` ✓ |
| Compaction at a safe moment, natively | idle compaction: after about an hour idle, on the one-hour cache, from 200,000 tokens (`CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS`), `idleCompaction` ✓ | the turn-end percentage above | `POST /api/session/{id}/compact` with `delivery: steer` runs "at the next step boundary"; `experimental.background_compaction` writes the summary ahead ✓ |
| Torlek sends a compaction | type `/compact` at the prompt, with a focus: `/compact focus on …` (adapter today: bare `/compact`, only while waiting) | app-server `thread/compact/start`, returns at once, progress as a `contextCompaction` item ✓ (adapter today: unsupported) | the endpoint above (adapter today: unsupported) |
| Torlek steers the summary | `/compact <focus>`; a "Compact Instructions" section in `CLAUDE.md`; automatic runs get no instructions (`custom_instructions` is `null`) | `compact_prompt` or `experimental_compact_prompt_file` replace the prompt, at launch ✓ | v1 plugin hook `experimental.session.compacting` adds context or replaces the prompt; not found in the installed v2 binary |
| Torlek re-injects after it | `SessionStart` with `source: "compact"` → `additionalContext` (used for the brief) | the same, also for a compaction in the middle of a turn | system-prompt transform (used for the brief) |
| Torlek observes it | `PreCompact` (can block) and `PostCompact` (receives `compact_summary`) | `PreCompact` and `PostCompact` (`trigger`: `manual` or `auto`), `thread/tokenUsage/updated` ✓ | events `session.compaction.started`, `.ended`, `.failed` ✓ |
| Context size to Torlek (#479) | last assistant `message.usage`: input + cache read + cache creation | `token_count` `last_token_usage.input_tokens`, `model_context_window` | assistant `tokens`: input + cache read + cache write |

Sources: Claude Code
[hooks](https://code.claude.com/docs/en/hooks.md) (PreCompact, PostCompact, SessionStart),
[settings](https://code.claude.com/docs/en/settings-reference.md) (`autoCompactWindow`,
`idleCompaction`),
[environment variables](https://code.claude.com/docs/en/env-vars.md),
[model configuration](https://code.claude.com/docs/en/model-config.md#set-the-auto-compact-window),
[context window](https://code.claude.com/docs/en/context-window.md),
[prompt caching](https://code.claude.com/docs/en/prompt-caching.md). Codex
[configuration reference](https://developers.openai.com/codex/config-reference),
[hooks](https://developers.openai.com/codex/hooks),
[app-server](https://developers.openai.com/codex/app-server). OpenCode
[configuration](https://opencode.ai/v2/docs/config),
[API](https://opencode.ai/v2/docs/api),
[v1 plugins](https://opencode.ai/docs/plugins#compaction-hooks).

What Torlek's adapters do today: Claude Code types `/compact` only while the session waits
([`cockpit-claude/src/lib.rs`](../../crates/cockpit-claude/src/lib.rs));
Codex and OpenCode refuse `Compact`
([`cockpit-codex/src/delivery.rs`](../../crates/cockpit-codex/src/delivery.rs),
[`cockpit-opencode/src/lib.rs`](../../crates/cockpit-opencode/src/lib.rs)); nothing in the daemon
calls the action. Both hook bridges map a post-compaction `SessionStart` to an unknown event
([Claude](../../crates/cockpit-claude/src/hooks.rs), [Codex](../../crates/cockpit-codex/src/hooks.rs)).
Context size is not reported yet; #479 is open.

## Deterministic content: what Torlek can put back exactly

The brief already comes back after every compaction
([decision 419](../decisions/2026-11-01-419-brief-delivery-per-harness.md)): 1,115 bytes, plus a
276-byte skill catalog entry (`torlek brief --measure`, 1,391 bytes always loaded; the manual is
9,862 bytes on demand). It names the workflow, step and attempt and says whether a checkpoint
exists, but carries none of their content. The agent has to run `torlek workflow packet`, one
extra retrieval turn that it only makes if the summary remembered to.

| Fact | Owner | After a compaction today |
| --- | --- | --- |
| Session, project, workspace, grants revision, manifest | brief | Back exactly |
| Workflow, step, attempt, checkpoint exists | brief | Back exactly |
| Goal title and notes, step requirements, decisions from finished items | task packet | Only if the summary kept them |
| Latest checkpoint note, every handoff | task packet | Only if the summary kept them |
| Issue, PR, branch, commit identifiers | goal notes, handoff evidence | Partly; see the retention table |
| The agent's working notes and open questions | the model | Summary only |

The last row is the only one a summary must carry, and the model's own checkpoint
(`torlek workflow checkpoint`) is its durable form. This matches the issue's idea: Torlek
re-injects what it knows; the model summarizes only its notes. The packet needs a bound before it
joins every context start (the handoff limit is already 2 KiB); its size was not measured because
no workflow run existed to read one.

## Options

**Option A — configure the harnesses' own triggers, and make a compaction harmless.**
Follow-ups 1 to 3 below. Pros: no new input path into a running agent, no new authority, no dependence on
reaching a safe moment (a third of Claude Code's cycles never do), and nothing a harness owns is
changed.
Cons: a Claude Code or OpenCode threshold still fires mid-turn; only Codex's is a turn-end rule.

**Option B — Torlek sends the compaction at a safe moment** (percentage from #479 reports, agent
at its prompt, no tool running). Pros: one rule for all three harnesses; a focus can name the
packet. Cons: needs context reporting, idle detection and three delivery paths first; catches 67 %
of cycles at 60 % (above); typing into a pane can answer a dialog by mistake, which is why the
adapter refuses unless the activity is known to be waiting. Cost of earlier compaction, as
arithmetic over the medians above (Claude Code, window 300,000, post-compaction 23,678, 76 s and
48,864 cache-written tokens per compaction), per million tokens of new work:

| Compact at | Compactions | Time in compaction | Cache written after them | Summarizer input per new token | Mean carried context |
| --- | --- | --- | --- | --- | --- |
| 60 % | 6.4 | 8.1 min | 313,000 | 1.15 | 101,839 |
| 70 % | 5.4 | 6.8 min | 264,000 | 1.13 | 116,839 |
| 89 % (Theo's 300,000 window today) | 4.1 | 5.2 min | 200,000 | 1.10 | 145,339 |

Earlier compaction costs about 2.9 more minutes and 113,000 more cache-written tokens per million
new tokens, in exchange for a context about 30 % smaller on every call: that mean carried context
is what each call re-reads from the cache, so cache reads fall by about the same share. Whether that is cheaper
depends on prices the research did not use, and whether the agent works better at 60 % than at
89 % was **not measured**. Compacting while the cache is still warm avoids paying to re-read a
cold prefix, which is why Claude Code ships its own idle compaction at the one-hour mark.

**Option C — end the session and start a fresh one with a handoff.** Pros: predictable content,
smallest restart (the brief and packet instead of 23,678 to 30,525 tokens of summary).
Cons: the agent must first write the handoff, a turn at 60 % or more of the window; the harness
conversation, its cache and its queued messages are gone; session identity, links and cards must
carry over (the placement transaction of
[decision 44](../decisions/2026-11-03-44-task-handoff.md) is for tasks, not for this). Use it at
step boundaries, where workflows already hand off, not as a context policy.

**Rejected for now.**

- *Replace the summary prompt* (Codex, OpenCode 1): it makes Torlek responsible for a harness's
  summary format, and Claude Code offers no equivalent for automatic runs. A re-injected packet
  gives the same protection on all three.
- *Block Claude Code's mid-turn compaction with `PreCompact` to wait for a turn end:* the
  [documented](https://code.claude.com/docs/en/hooks.md#precompact) result is an uncompacted
  conversation, and a block of the recovery path surfaces the API's context-limit error.
- *One universal percentage in Torlek's own settings:* the harness windows differ (258,400 and
  475,000 for Codex here, 300,000 and about 1,000,000 for Claude Code) and OpenCode's is not
  stored; a percentage needs the window from #479 first.

## Owner control

Proposed, none exists. A per-project setting with an off switch, default off (harness defaults
stay); a per-launch mapping to the native knobs in the table above, refused for a harness
version that lacks one; and a card note after any compaction: "compacted automatically at
268,364 tokens, now 30,470, 90 s", plus what Torlek re-injected. The owner's own harness
settings must stay visible: Theo's `autoCompactWindow` is what set Claude Code's 267,000-token
point, and Torlek would otherwise pick a number against it unknowingly.

## Proposed follow-up issues (not filed)

1. **Re-inject a bounded task packet with the brief at every context start.** Claude Code and
   Codex `SessionStart` (all sources), OpenCode system transform. Goal, step and requirements,
   latest checkpoint, last handoff, a command for the rest; fixed byte cap with a test that the
   cap holds. Owner of delivery: #419's adapters. Test: pinned `source: "compact"` payload per
   adapter returns brief and packet; a daemon test shows the packet equals `torlek workflow
   packet`. About a day.
2. **Record compactions and show them on the session card.** `PreCompact` and `PostCompact`
   (Claude Code, Codex), `session.compaction.started/ended/failed` (OpenCode); store trigger,
   tokens before and after, seconds; keep a post-compaction `SessionStart` from counting as a
   wait. Also add to #479 the exact size formulas in the table above. Depends on #479. About two
   days.
3. **Owner setting for a native threshold, off by default.** Launch mapping to Codex
   `model_post_turn_compact_threshold_percent`, OpenCode `compaction.buffer`, Claude Code
   `--autocompact`; shown on the card; refused when the installed version lacks the option.
   Depends on 2. About two days.
4. **Gated: Torlek-sent compaction at a safe moment.** `Action::Compact` carries an optional
   focus; Claude Code `/compact <focus>`, Codex `thread/compact/start`, OpenCode
   `POST /api/session/{id}/compact`; only for a session known to be waiting; capabilities pinned
   to tested versions. Start only if follow-up 5 shows a benefit.
5. **Quality check of earlier compaction (needs Theo's authorization, spends provider tokens).**
   Same fixed task on each harness, compacted at about 60 % at a turn end versus left to the
   harness, graded on whether the agent recovers the task, decisions, identifiers and open
   questions; report tokens, seconds and cache writes next to the grade.

## Sources, limits and what was not measured

- Measured on this machine from Claude Code 2.1.x transcripts, Codex 0.16x rollout files and the
  OpenCode 2 database; method and scripts in the [appendix](686/measurements.md).
- Not measured: the quality of any summary or of any agent after compaction; Codex and OpenCode
  reference retention; Codex compaction time; prices; the packet's size; behaviour on
  macOS or WSL2; any setting listed as ✓, which was found only as a string in the installed
  binary.
- Claude Code's 267,000-token point reflects Theo's setting, not a default; Codex's 94 % may
  change with its version; OpenCode's sessions are mostly older models.
- The OpenCode 2 compaction endpoint is documented in the v2 API reference; Torlek's adapter
  uses the v1-style `/session/...` routes, so which route serves a compaction was not tried.
- Related: #619 (turn and token efficiency), #479 (context reporting), #419 (instruction bundle),
  report [08](../design/greenfield/08-agent-onboarding-context.md).
