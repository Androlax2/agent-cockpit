# Worker model, effort and harness selection

Issue [#497](https://github.com/Androlax2/agent-cockpit/issues/497).
**Research and proposal, 2026-11-07; Theo has not decided.** Merging this document accepts
research delivery only. It activates no routing policy, switches no running session, adds no
product code and authorizes no paid or long-running benchmark. Every "Proposed" item below
waits for Theo's decision, item by item.

## Answer

Recommended: **go on recording, no-go on automatic selection for now.**

1. **Build the record first.** Nothing in Cockpit today stores which model, effort and harness
   did a task, what it cost and whether the result was accepted without rework. The coordinator's
   [trials log](../../.agents/skills/coordinator/trials-log.md) is the only data, 34 rows kept by
   hand, and its sample is too small to rank models (see [Local evidence](#local-evidence)).
2. **Start with an advisory shortlist** per task kind (Option A below), shown to the Chief and to
   the owner, with the evidence count and age next to every entry. This is the same conclusion as the
   legacy research (todo `c40433a5`, as the issue summarizes it) and it still holds on the greenfield
   architecture: the explicit choice mechanism already exists ([#701](../decisions/2026-11-04-701-session-model-effort.md)),
   so a selector only has to fill `--model` and `--effort`, not become a second router.
3. **Rules the owner approves (Option B) come second**, once a task kind has at least 10 accepted
   records per configuration. **A learned policy (Option C) is a no-go**: external evidence says
   learned routers often fail to beat a simple baseline, and Cockpit has tens of rows, not
   thousands.
4. **Local models are a real option for a narrow set of tasks and a hard "stays on this machine"
   constraint**, not a general replacement. On the one machine probed (16 GB GPU, about 30 GiB
   RAM) the comfortable class is roughly 8 to 12 billion parameters. Evidence for agentic coding at
   that size is thin and mostly vendor-reported. A local benchmark set is specified in
   [the local-models appendix](497/local-models.md); no result exists yet and none is claimed.
5. **Do not change the current rule on this evidence.** The coordinator's rule (Opus for
   security and data-loss, Sonnet for contained work, Fable rarely) is not contradicted by anything
   found. It is also not confirmed: the data cannot tell.

Decisions for Theo, in the order I suggest taking them (details in [Decisions](#decisions-for-the-owner)):

- [ ] D1: approve the outcome record fields (section [Record and scoring](#record-and-scoring)).
- [ ] D2: approve Option A as the first step, and Option B as the stated next step.
- [ ] D3: decide whether a local-only task constraint is wanted at all, and whether Theo runs the
  local benchmark himself.
- [ ] D4: decide whether any paid paired trial is allowed, and the ceiling (Phase 2 of the pilot).

## Method and labels

- **Fetched 2026-11-07.** Every external page was read on that date; page dates are given
  where the page states one. Dates in this document are 2026 unless written out.
- **Local sources.** The repository at the branch base `728c3fff`, the merged pull requests
  [#496](https://github.com/Androlax2/agent-cockpit/pull/496) and
  [#498](https://github.com/Androlax2/agent-cockpit/pull/498), `git log` trailers, the coordinator's
  trials log, and a read-only probe of this session's host.
- **Commands.** Trailer counts: `git log main --format='%(trailers:key=Co-Authored-By,valueonly)'`
  grouped by line. Trial counts: the table rows of `trials-log.md` grouped by model and kind;
  intervals are Wilson at 95%. Hardware: the read-only probe in the appendix.
- **Not found.** The coordinator's `.handoff/h2h-154/`, `.handoff/h2h-494/` and
  `.handoff/astra-trial/` folders named in the issue are not on this host (searched under the
  home folder). The head-to-head account below is rebuilt from the pull requests and issues that
  survive. No timings, token counts or costs from those runs could be recovered.
- **Labels.**
  - *Observed*: read or computed here from the repository or the host.
  - *Vendor*: published by the model's maker. Conflict of interest by construction.
  - *Third party*: an evaluator, publisher or practitioner who does not sell the model.
    Their own interests are noted per row.
  - *Secondary*: a page that repeats another source; the primary was not available.
  - *Est.*: computed by me from a stated formula; not measured.
- **Search limits.** Search results disagreed on which models exist. Model facts below come from
  Anthropic's documentation pages fetched directly; third-party model lists were not used where
  they conflicted with those pages.

## Local evidence

### The head-to-head runs (#154 and #494)

What the surviving record shows (*Observed*):

- **#154, portability inventory.** [#496](https://github.com/Androlax2/agent-cockpit/pull/496)
  "merged four independent inventories and their independent row fact-check into report 21":
  88 distinct assumptions, 148 added lines, one medium `/code-review` by a fresh sub-agent that
  approved after citation and table fixes. The pull request does not say which model produced which
  inventory.
- **#494, nushell approval check.** The inventory run found the defect
  ([#494](https://github.com/Androlax2/agent-cockpit/issues/494): the syntax check could run an
  unapproved command in nushell). [#498](https://github.com/Androlax2/agent-cockpit/pull/498) states:
  "the base implementation comes from the independent Opus solution; the approval lifecycle, edit,
  guide, and symlink-retarget regressions were adapted from the independent Astra solution." The
  merged change (396 added lines) is a combination of the two. A fresh independent review found
  nothing; the regression test was shown to fail on `main` and pass with the fix.
- **Astra configuration.** The 2026-10-30 [audit](../audits/audit-2026-10-30.md) records GPT-6
  Astra as `openai/gpt-6-astra`, variant `xhigh`, run through OpenCode. That is the only
  configuration named for it in the repository.

What this supports: two strong models given the same security task produced **complementary**,
not interchangeable, work, and combining them plus a blind review produced the merged result. It
does not support any ranking: one task per run, no repeat, no recorded time or usage, and the
reviewer saw the merge, not the individual attempts.

### The trials log

[trials-log.md](../../.agents/skills/coordinator/trials-log.md) has 34 rows, one per merged worker
PR (#745 to #887, logged from 2026-11-04; *Observed*, hand-kept by the coordinator). Counted by me from the
file, rows with a known CI-fix count only:

| Model | Rows | Rows with a CI fix | Share (95% interval) | Kinds |
| --- | ---: | ---: | --- | --- |
| Opus | 14 (12 counted) | 5 | 42% (19 to 68%) | 9 risky, 3 contained, 2 research |
| Sonnet | 15 (13 counted) | 4 | 31% (13 to 58%) | 9 contained, 5 research, 1 docs |
| Fable | 5 (5 counted) | 1 | 20% (4 to 62%) | 3 contained, 1 risky, 1 research |

Intervals are Wilson. They overlap completely. The kinds differ by model (Opus got the risky work
on purpose), so even a clean difference would not be about the model.

What the notes say about the 10 CI-fix rows: five are attributed to flaky tests or main drifting
under the branch (#800, #766, #406 p3, #406 p5, #825), three to a lint rule (#136, #845, #860). None
is attributed to a wrong design; the other two name a rebase (#713) and a PR opened before its review (#754). So **CI-fix count measures the repository's flakiness more than
the model**, and the log cannot separate the two until a failure is classified.

Other signals in the log (*Observed*, anecdotal):

- Every row has one review pass except #767 (two); the required `/code-review` is uniform and so
  gives no signal.
- Stalls (idle mid-task, needing a nudge) are listed for three PRs: #777 Sonnet, #766 Opus, #406
  p4 Fable. That is a human-intervention cost that no column captures.
- Open-to-merge time is missing in 8 rows and confounded by a reboot in 3 (#713, #800, #406 p2).

Two limits to carry forward. First, **`git` trailers cannot replace the log**: on `main` to
2026-11-05, 344 of 1,088 commits carry `Co-Authored-By: Claude Opus 5.5`, 32 Sonnet 5.5, 7 Fable
5.1 and 705 carry none, so the trailer undercounts and names no effort or harness. Second, the
coordinator keeps a longer private log (its working notes, not in the repository, not verified
here). Its 2026-11-06 comparison of about 93 rows reports the same pattern: security work on Opus
needed CI fixes in 10 of 11 PRs and the notes attribute that to sandbox CI difficulty, and Fable
had 4 of 10 PRs needing a nudge after idling. That is *Secondary* evidence; the repository copy
should be synced before anyone relies on it (follow-up F1).

## External evidence

### Evidence table

| Source and date | Type | What it measured | Finding that matters here | Limits |
| --- | --- | --- | --- | --- |
| [Anthropic, models overview](https://platform.claude.com/docs/en/models/overview) (fetched 2026-11-07) | Vendor docs | Lineup, prices, latency, default effort | Fable 5.1 $10/$50 per MTok, Opus 5.5 $4/$20, Sonnet 5.5 $2/$10, Haiku 5.5 from $0.10/$0.50. Default effort: Opus 5.5 and Haiku 5.5 `medium`, others `high`. Every model ID is a pinned snapshot. | List prices, not what a subscriber pays. |
| [Anthropic, choosing a model](https://platform.claude.com/docs/en/about-claude/models/choosing-a-model) (fetched 2026-11-07) | Vendor docs | Selection guidance | "Tuning effort is often a better lever than switching models"; build your own evals; start from the default effort and adjust "based on your evals". | Advice, not data. |
| [Anthropic, Sonnet 5.5 announcement](https://www.anthropic.com/claude-sonnet-5-5) (2026-09-28) | Vendor | Terminal-Bench 4.0, FrontierCode 1.1, CursorBench 4.0 and others | Sonnet 5.5 70.6% on Terminal-Bench vs Opus 5.5 66.4% (Opus at `xhigh`); Opus leads FrontierCode (54.4% vs 52.1%) and CursorBench (57.8% vs 55.5%). The text says Opus 5.5 remains clearly stronger on complex, open-ended work needing sustained judgment (paraphrase). | No trial counts or error bars on the page; footnote says Sonnet at `max` scored below its `xhigh` on FrontierCode because it ran the review skill more. |
| [Digital Applied, effort and cost per task](https://www.digitalapplied.com/blog/sonnet-5-5-or-opus-5-5-effort-level-cost-per-task) (2026-09-28) | Secondary of the vendor charts | 40 chart points, Sonnet 5.5 and Opus 5.5, five effort levels | Terminal-Bench: Opus `high` 64.2% at $3.88 beats Sonnet `xhigh` 61.5% at $5.30; Sonnet `max` 70.6% costs $12.54 vs Opus `xhigh` 66.4% at $7.35. FrontierCode: Sonnet `max` $20.78 for 46.2%, below its `xhigh` 52.1% at $1.59. | Vendor numbers, unreplicated; the publisher sells routing services. |
| [Anthropic, optimizing for cost and intelligence](https://platform.claude.com/docs/en/about-claude/models/optimizing-for-cost-and-intelligence) (read 2026-11-07, first half) | Vendor | Advisor and orchestrator pairings | Opus 5.5 with Fable 5.1 as advisor: +1.7 points "at the edge of run-to-run noise" for about 2.1 times the cost. Fable 5.1 alone cost 2.2 to 3.0 times Opus 5.5 at `high` on a defect-search task and "scored no higher". Haiku subagents cut cost 32% and lost about 4 points, "an alternative to lowering effort, not better than it". Page advises a pairing only if it beats the single model's whole effort-cost curve. | Internal, directional, list prices; second half of the page was not read. |
| [The Decoder](https://the-decoder.com/benchmarks-disagree-on-gpt-6-astra-but-its-human-beating-efficiency-on-arc-agi-3-pulls-chollets-agi-forecast-forward/) (2026-09-04) | Third party reporting Artificial Analysis and Epoch | Coding Agent Index, overall indices | Astra 67 vs Fable 5.1 70 on the Coding Agent Index; Astra uses about a third of the compute steps of another OpenAI model and under half the cost per task of Fable 5 at similar coding scores; Epoch has one Astra coding score, at medium effort. Headline: "benchmarks disagree". | Secondary; the live [index](https://artificialanalysis.ai/agents/coding-agents) shows no rows in the text fetched, and scores whole agent products (harness plus model plus settings), so not a model ranking. |
| [Insight Tech APAC, "Token price is the wrong number"](https://blog.apac.insight.com/2026/07/06/cost-to-a-merged-feature) (2026-07-06) | Practitioner | One real feature (OAuth device flow) on a frozen repo, 23 arms and 29 runs, a fixed Opus 4.8 reviewer looped until merge | Cost to merge ranged about $2.81 to $33 and time 26 to 176 minutes; the same model cost about 2.5 times more on one harness than another; the arm with internal self-review needed 1 review cycle vs 3 for the same model without it; run-to-run spread was large (an objective score ranged 11 to 31 of 38). | One task, mostly one run per arm, older models (Opus 4.8, Fable 5, Sonnet 5), author notes unfamiliarity with some harnesses; the reviewer is itself a model. |
| [METR, Becker et al.](https://arxiv.org/abs/2507.09089) (2025-07-12) | Third party, randomized trial | 16 experienced developers, 246 tasks, AI allowed or not | AI made tasks take 19% longer while developers believed it made them 20% faster. | Early-2025 tools, mature open-source repositories, 16 people. |
| [METR time horizons](https://metr.org/time-horizons/) (page last updated 2026-09-08, "no longer actively updated") | Third party | Task length at which an agent succeeds 50% or 80% of the time | Measurements above 16 hours are unreliable; tasks are "self-contained and well-specified", cleaner than real work; human baselines likely overestimate human time. | Mostly software, ML and security tasks; not about model choice for a given repository. |
| [RouteLLM](https://arxiv.org/abs/2406.18665) (2024-06-26, v4 2025-02-23) | Academic | Routers choosing between a strong and a weak model, trained on preference data | "Over 2 times" cost reduction in certain cases without losing response quality. | Chat quality benchmarks, not coding agents. |
| [LLMRouterBench](https://arxiv.org/abs/2601.07206) (2026-01-12) | Academic | 10 routing baselines, over 400,000 instances, 21 datasets, 33 models | "Many routing methods exhibit similar performance"; "several recent approaches, including commercial routers, fail to reliably outperform a simple baseline"; a large gap to the oracle remains, "driven primarily by persistent model-recall failures". | Not agent tasks; the abstract does not say which simple baseline. |

### Where the sources disagree

- **Sonnet 5.5 vs Opus 5.5.** Sonnet is ahead on Terminal-Bench 4.0 at its best setting and behind
  on every other agentic-coding row, and the Terminal-Bench lead exists only at `max` effort, where
  it costs about 1.7 times as much as Opus's best (vendor charts above). **Which is better depends
  on the benchmark and on the effort setting**, so a model-only ranking is unsound.
- **Token price vs cost per task.** Sonnet is half the list price per token, but at `max` it
  needs more tokens per task and costs more than Opus at `high` for a similar score. Insight's
  study shows the same shape one level up: harness and review style moved cost 2.5 times for the
  same model.
- **Fable.** The Artificial Analysis index puts Fable 5.1 first; Anthropic's own defect-search
  data shows it 2.2 to 3.0 times Opus 5.5's cost "and scored no higher"; Cockpit's 5 Fable rows
  show no CI signal either way. All three can be true: a premium model helps on the hardest
  long-horizon tasks and is wasted on contained ones.
- **Defaults.** Anthropic made `medium` the default for Opus 5.5, while the coordinator runs Opus
  at `high`. Without a Cockpit-side sweep neither is known to be right for this repository.
- **Perception vs measurement.** METR found developers' own speed estimates 39 points off.
  A routing system that learns from worker or owner impressions inherits that error; it needs
  measured time to accepted completion.

### What transfers to Cockpit and what does not

- **Transfers (reasonably well).** Effort is a first-class lever alongside model; cost per
  accepted task, not price per token, is the number to compare; self-review and harness change
  cost and cycles as much as the model; single runs are unreliable (Insight, METR).
- **Transfers weakly.** Benchmark scores (Terminal-Bench, FrontierCode, CursorBench). They are
  vendor-chosen suites on other repositories; Cockpit tasks are Rust, protocol and sandbox
  work with a project-specific gate.
- **Does not transfer.** Router accuracy numbers from chat benchmarks (RouteLLM), and any
  absolute dollar figure from a pay-per-token study to a subscription-funded Cockpit session.
- **Uncertainty to keep.** Model versions turn over in weeks (Opus 5.5 on 2026-09-22, Sonnet 5.5
  on 2026-09-28), so any measured cell is evidence about one pinned snapshot.

## Matching tasks to configurations

*Proposed* starting shortlist, to be shown as advice. It reflects the current coordinator rule,
the vendor guidance and the thin local data; **confidence is low everywhere** and the third column
says what would change it.

| Task kind | Proposed first choice | Escalate to | Evidence basis | Would change it |
| --- | --- | --- | --- | --- |
| Bounded fix, one file, clear test | Sonnet 5.5, `medium` | Opus 5.5 `high` after a failed review | 4 counted Sonnet-medium contained rows, 2 with a CI fix (both flake or main drift) | 10 records per cell |
| Documentation, mechanical | Sonnet 5.5, `low` or `medium` | none | 1 docs row (#777), clean | A defect found only after merge |
| Synthesis and research report | Sonnet 5.5, `high` | Opus 5.5 `high` | 4 of 5 Sonnet research rows had no CI fix | A reviewer finding unsupported claims |
| Ordinary implementation, contained | Sonnet 5.5, `high` (up to a few hundred lines) | Opus 5.5 `high` | Mixed; CI-fix share 3 of 7 for Sonnet, 1 of 2 for Opus | Failure classification |
| Architecture and cross-crate routing | Opus 5.5, `high` | Fable 5.1 `high` or `xhigh` only if Opus at `xhigh` falls short | Vendor guidance; no local contrast | A paired trial |
| Security, concurrency, data loss | Opus 5.5, `high`; never lowered | a second independent model for review | #154/#494 complementary work; the owner's rule | Nothing downward without Theo |
| Independent review | A different model from the author, one step above its effort if affordable | none | The merged #498 used two models and a blind review | A measured miss rate |

Two constraints apply to every row. **The harness is not free:** today only the Claude adapter
reports the `Model` and `Effort` capabilities (`crates/cockpit-claude/src/lib.rs`; Codex and
OpenCode do not, so a start that chooses one is refused by design, per the #701 decision). And
**effort and model are fixed at start** (#701), so escalation means ending the session and
starting another, which costs the work in progress.

## Record and scoring

### Proposed outcome record

One row per accepted or abandoned task attempt, written by the system, not typed by a worker.
Fields marked * are not collected anywhere today.

| Group | Fields |
| --- | --- |
| Task | Issue or task id; task kind (the seven above); size class from the final diff; risk flags (security, data loss, cross-crate); source revision |
| Configuration | Harness and version; model id as requested and as reported*; effort as requested and as effective* (a harness default is not the same as "not set"); brief hash |
| Outcome | Accepted yes or no; time from start to accepted completion; review findings by severity*; CI-fix rounds, each classified as own-fault, flake or main drift*; human interventions (nudges, questions answered, restarts)*; whether a defect surfaced after merge* |
| Usage | Input, output and cache tokens*; the usage window share consumed* (from #58, with provenance); metered cost only when the account is metered; otherwise the field says "not metered" |
| Provenance | Who or what recorded it; when; whether it was backfilled |

Three accounting rules, from the issue:

- **Three cost states, never merged:** metered spend, allowance share, unknown. A `$0.00` display
  and an API-equivalent price are not billing.
- **A local run costs machine time and energy**, recorded as such, not as free capacity.
- **Count to accepted completion**, including every review and fix cycle, not first response.

### Hard constraints and preferences

Hard constraints filter; they are never traded off. *Proposed*: the harness and model are
available to the account and the project (permissions, #59); the adapter reports the capability
(#701); a "runs locally" or "no external data" requirement of the project or task; the owner's
budget and window state allow the start (#412, #42); an owner override on the task.
Preferences rank what remains: expected accepted-completion probability, wall time, rework,
interventions, and allowance share.

### Scoring

Do not collapse to one number. Show each candidate's **measured accepted-completion rate with its
interval, median time to accepted, median interventions and median allowance share**, and let the
rule or the owner choose. A single weighted score would hide exactly the disagreements this
research found. A tie-break weight is allowed only when Theo sets it.

## Routing approaches compared

| | A. Advisory shortlist (manual) | B. Owner-approved rules | C. Learned or adaptive policy |
| --- | --- | --- | --- |
| What it does | Shows a ranked shortlist with evidence count and age; the Chief or Theo picks | A table `task kind and risk` to `model, effort`; the Chief applies it; every use is explained | Estimates success and cost per cell and picks, possibly exploring |
| Cold start | The current rule, labelled "no data" | Same | Needs priors; no data to give them |
| Evidence needed | None to start; the record fills it | At least 10 accepted records per cell | Hundreds per cell to beat a fixed rule (see LLMRouterBench) |
| Failure mode | Ignored | Stale rule after a model update | Silent drift, unexplained choices, wasted exploration spend |
| Owner control | Total | Rules are owner-approved text | Needs an override layer anyway |
| Cost | A record and a view | A rules file and a check | A data pipeline and an exploration budget |
| Verdict | **Recommended now** | **Next, after the record has data** | **No-go** |

### Cold start, expiry, escalation (all *Proposed*)

- **Cold start:** use the current rule and label it "rule, no data".
- **Cell key:** task kind, model id, effort, harness and version. A new model id starts a new cell.
  Because model ids are pinned snapshots, the old cell's records stay true for the old model and
  are shown as a flagged prior (not counted) for the successor.
- **Strength of evidence:** under 3 records: none. 3 to 9: weak, shown but never a rule. 10 or
  more: supported. A record older than 90 days, or from a harness version two minor releases back,
  is shown dimmed and not counted toward the 10.
- **Escalation after failure:** after two blocking review findings or two own-fault CI failures,
  the Chief proposes one step up (effort, then model). It asks Theo when the step exceeds the
  budget. It never switches silently, which matches the statement already in #412.
- **When an expensive reviewer pays:** when the expected cost of an escaped defect (rework,
  incident, trust) times the miss rate of the cheaper review exceeds the cost of the stronger review.
  Cockpit cannot evaluate that without the "defect found after merge" field; until it is collected,
  keep the existing one medium `/code-review` per PR and use a second model only for security work.
- **Automatic small trials:** not worthwhile yet. Each costs real usage, the paired signal is weak
  (see the intervals above), and the owner's allowance is the scarce resource. If ever added, a trial
  must be opt-in per project, capped by a stated budget, run on copies with no production side
  effects, and stop on the first budget breach.
- **Overrides and explanation:** every selection stores why (rule id, evidence counts) and shows
  it in the session view; an owner override always wins and is recorded as one.

### Integration points (no duplication)

| Existing piece | Role for selection |
| --- | --- |
| #701 (decided, [record](../decisions/2026-11-04-701-session-model-effort.md)) | The mechanism: `--model` and `--effort`, provider words, fixed at start, shown to the owner in the start approval. Selection only supplies values. |
| Chief ([design 07](../design/greenfield/07-teams-and-chief.md)) | Reads the shortlist when starting a worker; records the outcome when ending it. |
| #58 usage, [design 18](../design/greenfield/18-usage-quota-and-accounts.md) | Source for tokens and window share, with provenance. Selection does not read quotas itself. |
| #412 pacing, #158 resilience | May lower effort or model only where the owner allowed, never silently; selection supplies the candidate, pacing decides. |
| #59 account routing | Account choice stays at launch and quota-stop restart; the model and effort are independent of it. |
| #42, #43 admission and scheduling | Decide whether a start happens now; a local model adds GPU and RAM as admitted resources. |
| Provider capabilities | Claude reports `Model` and `Effort`; Codex and OpenCode must report them before any rule can name them. |

## Local models

The long version, with commands and estimates, is in
[the local-models appendix](497/local-models.md). In short:

- **Probe, this host (*Observed*, read-only, nothing sent anywhere):** AMD Ryzen 7 7800X3D, 16
  threads; about 30 GiB RAM with 20 GiB available while sessions ran; one Radeon RX 9070-class GPU
  with 16,304 MiB VRAM; 550 GB free disk. No NVIDIA GPU.
- **What it can run (*Est.*):** about 8 to 12 billion parameters comfortably at 4-bit with a
  useful context; a 14 GB class (27B at 4-bit, gpt-oss-20b) fits the GPU only with a small context
  and is the borderline case; anything needing 48 GB or more (Qwen3-Coder-Next, gpt-oss-120b) cannot
  run.
- **Quality (third party and vendor, weak):** vendor-reported agentic scores for the newest open
  27B model are well below the Claude 5.5 family's on comparable suites, and no independent
  coding-agent result for a model that fits this machine was found. Treat local models as
  candidates for bounded, well-tested tasks (documentation, mechanical fixes, first-pass review),
  pending the benchmark.
- **Harness support (docs, 2026-11-07):** OpenCode discovers Ollama and LM Studio on localhost
  and needs explicit tool settings for vLLM; Codex CLI has `--oss` and custom providers; Claude
  Code through a local endpoint is described by a third-party guide, but Anthropic states it does not support routing
  Claude Code to non-Claude models. Cockpit would need the `Model` capability on those adapters, a
  supervised model server with resource admission, and a localhost forward in the sandbox.
- **As a constraint:** "runs locally" should be a hard task or project filter, enforced by what the
  harness can reach (the sandbox's egress rules), not by a model name.

## Pilot plan

All *Proposed*. Phase 0 spends nothing beyond normal delivery.

| Phase | What | Budget | Success | Stop |
| --- | --- | --- | --- | --- |
| 0, record | Backfill the 34 log rows into the record; add the fields; classify CI failures | Normal work; about two days for the schema and backfill | Every merged worker PR has a complete row for 4 weeks | A field cannot be filled without reading private content |
| 1, advise | Show the shortlist beside each worker start | Normal work | Theo follows or overrides it knowingly; no cell is presented as supported below 10 records | Theo finds the display misleading |
| 2, paired trials (optional, paid) | For 4 task kinds, 2 configurations, 3 repeats each: 24 task-runs on matched revisions, equal briefs, executable checks and blind review | A ceiling Theo sets in allowance share or dollars; I give no figure because Cockpit has no measured usage per accepted PR yet. As an order of magnitude only, Insight's study reports about $3 to $33 per merged feature at API-equivalent prices on older models | At 10 runs per cell each interval is about 26 points either side, so only differences near 50 points between two cells show; Phase 2 tests for large effects only | The ceiling is reached, or a trial causes a production side effect |
| 3, rules | Write Option B from supported cells, owner-approved | None | At least one task kind has a supported cell and a rule beats the baseline on accepted-completion and allowance share | A model update invalidates the cell |

**Baseline:** the current coordinator rule on the same task kinds. **Evidence before activating
automatic selection:** 10 or more accepted records per configuration in at least two task kinds,
classified failures, usage fields filled, and Theo's approval of the written rules.

## Go or no-go and follow-ups

- **Go:** the record (F1, F2), the advisory display (F3), capability reporting (F4).
- **No-go until Phase 3 evidence:** automatic selection of any kind, learned policies, automatic
  trials.
- **Go, separately and on Theo's agreement:** the local benchmark (F5) and the local probe (F6).

Proposed follow-ups. **None is filed**; filing them is Theo's call.

| Id | Proposed issue | Depends on |
| --- | --- | --- |
| F1 | Sync the coordinator's trials log into the repository and add the missing columns (effort effective, failure class, interventions) | none |
| F2 | The outcome record in the state store, written at worker end, with the usage fields | #58, #701 |
| F3 | Show model, effort and the shortlist in `session show`, `agent sessions` and the TUI (the item #701 deferred) | F2 |
| F4 | Codex and OpenCode report `Model` and `Effort` when their harness supports them | #701 |
| F5 | The local benchmark runner and its first results on Theo's machine | D3 |
| F6 | A read-only hardware probe in the readiness report, local only | #38 |

## Decisions for the owner

- **D1, the record.** Approve, trim or change the fields in [Record and scoring](#record-and-scoring).
  Recommended: approve as written; the starred fields are what is missing.
- **D2, the approach.** Option A now, Option B after the record has data, Option C not pursued.
  Recommended: yes.
- **D3, local.** (a) Is a "local only" constraint wanted on tasks and projects? (b) Will Theo run
  the local benchmark himself on this machine, at no money cost, bounded as the appendix says?
  Recommended: yes to both, but separately.
- **D4, paid trials.** Allow Phase 2 at all, and set its ceiling. Recommended: not yet; decide after
  Phase 0 shows what a normal accepted task costs in allowance share.

## What this does not show

- No ranking of models. The local data is 34 rows, the head-to-head runs left no timing or usage,
  and external scores are mostly vendor-published and unreplicated.
- Nothing about Codex or OpenCode workers' quality in Cockpit: there are no trial rows for them.
- No local-model result. The benchmark is specified, not run.
- The legacy research (`c40433a5`) is cited through the issue's summary; its text is not in this
  repository.
