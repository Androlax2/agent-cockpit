# Benchmarking worker models and effort levels

Issue [#1264](https://github.com/Androlax2/agent-cockpit/issues/1264). For the owner and the
coordinator deciding how model and effort trials run from alpha 8.
**Research and proposal, 2026-11-09.** Merging it accepts the research and moves the trials
database into the repository ([`1264/`](1264/README.md)). It changes no product code and no
coordinator prompt, and starts no paid run. Each "Proposed" item waits for the owner's decision.

## Answer

Our method can tell a model that fails often from one that rarely fails. It cannot rank models
that are close, and three parts of it measure something other than the model. Ranked fixes,
cheapest first:

1. **Read the data we have correctly** (done in this pull request). Count only CI failures the
   worker could have prevented, log idle stops separately from nudges, compare models only within
   the same task kind and size, show a 95% interval on every share, and drop the claim that 10
   rows make a cell "supported": at 10 rows a share is known to within about ±22 points.
2. **Pick trials by a fixed rule, not by judgement.** Example: the issue number modulo 3. A
   coordinator who chooses which start becomes a trial also chooses which tasks each model gets.
   Cost: none.
3. **Blind and anchor the reviewer.** Remove the author model's name and the words "model trial"
   from the reviewer prompt, and give it a fixed checklist of what counts as blocking. Cost: about
   30 minutes once.
4. **Give the reviewer a baseline** (D1, decided A). Run the same independent review on about one
   default pull request in four. Without it, "blocking findings per PR" can only be measured on
   trial PRs and has nothing to compare against. Cost: about 1.5 to 2 hours of agent time per alpha.
5. **Decide each cell with a written stopping rule** ([below](#stopping-rule)), not by
   re-comparing every 10 rows, which is a form of peeking.
6. **Before a new model's first live trial, run it on a small replay set of past issues**
   (decision D3), paired against the merged result. Cost: about 4 hours per new configuration,
   not per alpha.
7. **Add a reviewer from another model family on a sample** once Codex trials start
   (decision D2). Today's reviewer is always Claude and is likely to be lenient on Claude authors.
   Cost: about 1 hour per alpha.

Not recommended at our volume: bandit algorithms, Elo or pairwise rankings, learned routers,
pass^k on live work, and public benchmark scores as the deciding evidence (see
[Survey](#survey)). Parts 3 to 7 together cost about 3 hours of agent time per alpha, plus 4 hours
whenever a new model enters.

Decisions for the owner, one at a time ([details](#decisions-for-the-owner)):

- [x] D1: control reviews on 1 in 4 default PRs (A, recommended) or trial PRs only (B).
  **Decided 2026-11-09: A.**
- [x] D2: a second reviewer from another family on 1 in 4 reviewed PRs once Codex trials start
  (A, recommended) or one Claude reviewer only (B). **Decided 2026-11-09: A.**
- [ ] D3: a 4-issue replay before a new model's first live trial (A, recommended) or live
  rotation only (B).

## Method and labels

- **Fetched 2026-11-09.** Every external page was read on that date; page dates are given
  where the page states one. Two pages could not be fetched (noted in place); Reddit was read
  only through mirrors and is not cited.
- **Local data.** The coordinator's trials database, 163 rows, one per merged worker pull request
  from 2026-11-04 to 2026-11-09, now at [`1264/model-trials.csv`](1264/model-trials.csv). The
  reviewer gate prompts and the model card were read from the coordinator's private notes; only
  their rules are described here. The selection rule is in
  [the #497 research](2026-11-07-497-worker-model-selection.md).
- **Statistics.** Shares carry a 95% Wilson interval. Sample sizes for two shares use the
  pooled normal formula at α = 0.05, power 80%. Stratified comparisons use the Mantel-Haenszel
  odds ratio over task kind × size band (≤100, 101-500, 501-1000, over 1000 added lines, unknown).
- **Labels.** *Observed*: computed here from our rows. *Vendor*: published by the maker of the
  model or tool measured, a conflict of interest by construction. *Third party*: an evaluator who
  does not sell the thing measured. *Practitioner*: a team describing its own practice, often
  self-reported. *Academic*: a paper. *Est.*: my estimate from a stated basis.

## Survey

How others choose and measure a model and effort level for coding agents, grouped by method.

### Methods compared

| Method | What it needs | What it costs | Known failure modes | Fit for us |
| --- | --- | --- | --- | --- |
| Public benchmark scores (SWE-bench, Terminal-Bench, SWE-bench Pro) | Nothing; read a leaderboard | Free | Contamination, broken tests, harness dependence, noise between adjacent ranks | Background only |
| Internal eval set from real failures or merged PRs, replayed on frozen commits | A task bank, a clean environment per run, a grader (tests, rubric) | One agent run per task and configuration, plus grading | Leaking state between runs, graders that reject valid work, contamination for public repos | Yes, small (D3) |
| Paired A/B on the same task | Two configurations on the same task and base | Twice the runs | Few tasks; run-to-run variance | Yes, inside the replay set |
| Production metrics (time to merge, reverts, review findings) | A record per PR | Bookkeeping | No control group; process changes with the model; confounded by task | Yes, it is our rotation |
| LLM-as-judge or LLM review rubric | A rubric, a judge, ideally a human-labelled calibration set | One judge run per item | Self and family preference, position and length bias, run-to-run variance, low recall | Yes, with fixes (D2, part 3) |
| pass@k and pass^k | k repeated runs per task | k times the runs | Assumes independent runs; consistency is not correctness | Replay set only |
| Bandits (Thompson sampling) | A reward per pull, many pulls, random assignment | Free to run, costly in bad pulls | Stops exploring; breaks when models change; confounded when the choice depends on the task | No |
| Elo or Bradley-Terry | Many pairwise votes on the same prompt | Thousands of votes | Unequal sampling, style bias, private testing | No |
| Cost per completed task, cost-accuracy frontier | Cost and outcome per task | Bookkeeping | Explodes when success is rare; list prices are not subscription costs | Yes, once usage is recorded |

### Sources by method

**Internal and replay evals.**

- [Anthropic, "Demystifying evals for AI agents"](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents)
  (2026-01-09, *Vendor*). Start with "20-50 simple tasks drawn from real failures"; grade code by
  tests; run several trials; read transcripts. Names state leaking between trials (an agent read
  git history left by an earlier one) and graders that reject valid work.
- [Stet, GPT-5.5 reasoning curve on 26 real tasks](https://www.stet.sh/blog/gpt-55-codex-graphql-reasoning-curve)
  (2026-05-07, *Practitioner*, author sells the eval tool, numbers self-reported). 26 merged PRs
  replayed on frozen snapshots, one run per effort. Low and medium both passed tests on 21 of 26,
  but equivalence to the human PR rose from 15% to 42%; `xhigh` cost about 2.2 times `high`. Tests
  alone hid the difference between effort levels.
- [Faros AI, scoring code without running tests](https://www.faros.ai/blog/ai-model-routing-evaluation-framework)
  (2026-07-14, *Vendor*). Replays a company's merged PRs; a per-task weighted rubric is written
  before scoring and a blinded judge answers each item yes or no. Admits "good patches can fail
  tests, and poor patches can pass them".
- [CursorBench](https://cursor.com/blog/cursorbench) (2026-03-11, *Vendor*). Tasks traced from
  committed code back to the agent request; offline set refreshed every few months, plus online
  checks for "consistent movement" across metrics.
- [Aider polyglot](https://aider.chat/2024/12/21/polyglot.html) (2024-12-21, *Practitioner*).
  Kept only the 225 exercises that 3 or fewer of 7 models solved: dropping tasks every model solves
  keeps a small set discriminating.
- [SWE-smith](https://arxiv.org/abs/2504.21798) and [SWE-Gym](https://arxiv.org/abs/2412.21139)
  (2025, *Academic*). One container per frozen commit; Python-only training sets. The idea
  transfers, not the tooling.

**Why public scores mislead.**

- [METR, SWE-bench-passing PRs would not be merged](https://metr.org/notes/2026-03-10-many-swe-bench-passing-prs-would-not-be-merged-into-main/)
  (2026-03-10, *Third party*). Four maintainers blind-reviewed 296 passing agent PRs: merge rates
  about 24 points below the grader's.
- [Epoch AI review of SWE-bench Verified](https://epoch.ai/benchmarks/swe-bench-verified/review)
  (2026-09-03, *Third party*): verdict "Flawed"; reports OpenAI's finding that frontier models had
  seen problems and solutions, and that 59.4% of audited tasks had flawed tests. The OpenAI post
  itself returned HTTP 403.
- [The SWE-Bench Illusion](https://arxiv.org/abs/2506.12286) (2025, *Academic*): models name the
  buggy file from the issue text alone 76% of the time on Verified, 53% outside it. Our repository
  is public, so its history may be in training data.
- [SWE-rebench](https://arxiv.org/abs/2505.20411) (2025, *Academic*; [leaderboard](https://swe-rebench.com/)):
  fresh tasks with contamination tracked against model release dates.
- [SWE-Bench Pro](https://arxiv.org/abs/2509.16941) (2025, *Academic*): held-out and commercial
  sets to resist contamination.
- [Terminal-Bench leaderboard: luck or skill?](https://www.lesswrong.com/posts/GiPmLmmbT6DyrwYkH/terminal-bench-leaderboard-rankings-luck-or-skill)
  (2026-08-05, *Third party*): over 52,104 trials, 24 of 25 adjacent ranks differ only by noise;
  treating trials as independent halves the interval width; the same model moved 15 to 22 points
  with the harness.
- [Anthropic, infrastructure noise in agentic coding evals](https://www.anthropic.com/engineering/infrastructure-noise)
  (2026-02-05, *Vendor*): CPU and memory limits alone moved Terminal-Bench 2.0 by 6 points; pass
  rates varied with time of day.
- [On Randomness in Agentic Evals](https://arxiv.org/abs/2602.07150) (2026, *Academic*): 10 runs
  per task on SWE-bench Verified; single-run scores vary by 2.2 to 6.0 points, gains of 2 to 3
  points may be noise.

**Production metrics.**

- [Augment Code, software factory](https://augmentcode.com/blog/beyond-ai-coding-agents-how-we-built-augments-software-factory)
  (2026-09-11, *Vendor*, self-reported): 17,200 merged PRs; median creation-to-merge 11.2 h to
  3.1 h and 14-day revert share 1.9% to 0.4%, with no control group.
- [Specific, Real-SWE](https://withspecific.com/benchmarks/real-swe) (2026-10, *Third party* with
  a commercial stake) and its [Hacker News thread](https://news.ycombinator.com/item?id=49676820):
  25 tasks from licensed private codebases, pass@1 over 5 runs, top models' intervals overlap.
  In the thread, practitioners describe building private evals by splitting git history just
  before a change and grading against the accepted PR, and checking for contamination each time.
- [mabl, picking a model for our agents](https://fab-dev.mabl.com/posts/2026-06-08-lauren-picking-a-model-for-our-agents)
  (2026-06-08, *Practitioner*): "a handful of sessions tells you almost nothing"; "more thinking
  isn't always better".
- [Alam, Mondal, Roy: why agent fix PRs stay unmerged](https://arxiv.org/abs/2602.00164) (2026,
  *Academic*): 8,106 agent PRs; test failures and duplicate fixes lead the 12 reasons.
- [METR, early-2025 developer study](https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/)
  (2025-07-10, *Third party*): developers felt 20% faster and were 19% slower. Impressions are
  not measurements.

**Statistics of small samples.**

- [Miller, "Adding Error Bars to Evals"](https://arxiv.org/abs/2411.00640) (2024, *Academic*,
  Anthropic author; [summary](https://www.anthropic.com/research/statistical-approach-to-model-evals)):
  cluster standard errors on the unit of randomization (up to about 3 times the naive error),
  compare models on paired per-task differences, plan sample size by power analysis.
- [tau-bench](https://arxiv.org/abs/2406.12045) (2024, *Academic*): pass^k, the chance all k runs
  succeed; retail pass^8 under 25%. Needs repeated runs of the same task.
- [Brown, Cai, DasGupta](https://asaip.psu.edu/?p=677) (2001, *Academic*): Wilson or Jeffreys
  intervals for small n.
- [Evan Miller, "How Not To Run an A/B Test"](https://www.evanmiller.org/how-not-to-run-an-ab-test.html)
  (*Practitioner*): stopping at the first p < 0.05 within 150 observations gives a 26.1%
  false-positive rate. [Always-valid inference](https://arxiv.org/abs/1512.04922) (2015,
  *Academic*) is the formal remedy.
- [A Tutorial on Thompson Sampling](https://arxiv.org/abs/1707.02038) (2017, *Academic*): it
  minimises regret, not best-arm identification, and under drift "should never stop exploring".
  [Bandits with unobserved confounders](https://www.causalai.net/r26.pdf) (2017, *Academic*):
  when the chooser's judgement drives both choice and outcome, naive averages mislead.
- [Li et al., unbiased offline evaluation](https://arxiv.org/abs/1003.5956) (2010, *Academic*):
  logged choices support unbiased comparison only when the arm was picked at random with a known
  probability.
- [Chatbot Arena](https://arxiv.org/abs/2403.04132) (2024, *Academic*): about 8,000 votes per
  model. [The Leaderboard Illusion](https://arxiv.org/abs/2504.20879) (2025, *Academic*): unequal
  sampling and selective disclosure bias rankings.
- [Simpson's paradox, clinical example](https://pmc.ncbi.nlm.nih.gov/articles/PMC9960320/)
  (2023, *Academic*): a treatment better in both strata ranks worse pooled because it got the hard
  cases. [Austin, propensity scores](https://pmc.ncbi.nlm.nih.gov/articles/PMC3144483/) (2011,
  *Academic*): needs every task type to have a chance of every model.
- [Cost-of-Pass](https://arxiv.org/abs/2504.13359) (2025, *Academic*) and
  [AI Agents That Matter](https://arxiv.org/abs/2407.01502) (2024, *Academic*): compare cost per
  success on a cost-accuracy frontier; reported scores often fall outside their own 5-run range.

**LLM reviewers as instruments.**

- [Panickssery et al.](https://arxiv.org/abs/2404.13076) (2024, *Academic*): judges recognise and
  favour their own outputs. [Play Favorites](https://arxiv.org/abs/2508.06709) (2025, *Academic*):
  GPT-4o and Claude 3.5 Sonnet favour their own and same-family outputs after a human-quality
  adjustment. [Who Judges Matters](https://arxiv.org/abs/2609.17857) (2026, *Academic*, open-weight
  families): same-family lift of 3.4 to 8.4 points.
- [Pombal et al., self-preference in rubric evaluation](https://arxiv.org/abs/2604.06996) (2026,
  *Academic*): even with objective rubrics, judges can be over 50% more likely to wrongly pass
  their own failed items, including on LiveCodeBench; ensembles reduce, not remove, it.
- [Chen et al., do evaluators prefer themselves for a reason?](https://arxiv.org/abs/2504.03846)
  (2025, *Academic*): harmful self-preference "persists when evaluator models err as generators";
  reasoning before judging reduces it.
- [Mahbub and Feng, authorship obfuscation](https://arxiv.org/abs/2512.05379) (2025, *Academic*):
  light perturbation reduces self-preference; it cannot be removed fully.
- [Preference leakage](https://arxiv.org/abs/2502.01534) (2025, *Academic*): judges favour models
  trained on related models' data, so family lines blur.
- [Replacing Judges with Juries](https://arxiv.org/abs/2404.18796) (2024, *Academic*, Cohere): a
  panel from disjoint families beats one large judge, with less intra-model bias.
- [CodeJudgeBench](https://arxiv.org/abs/2507.10535) (2025, *Academic*): every code judge shows
  significant randomness; accuracy shifts with which model wrote the code.
- [Determinism in LLM code review](https://arxiv.org/abs/2502.20747) (2025, *Academic*): five
  identical runs at temperature 0 still differ.
- [Greptile benchmark](https://www.greptile.com/benchmarks) (2025, *Vendor*): 50 re-introduced
  bugs, best tool 82% by its own count; [Tenki](https://tenki.cloud/blog/benchmarking-ai-code-reviewers)
  (2026-05-20, *Vendor*, a competitor) re-annotated the same PRs to 122 bugs: recall 0.25 to 0.69,
  precision 0.16 to 0.51.
- [Anthropic, Code Review for Claude Code](https://claude.com/blog/code-review) (2026-03-09,
  *Vendor*): PRs over 1,000 lines average 7.5 findings, under 50 lines 0.5; about 20 minutes per
  review. Findings scale with diff size.
- [SmartBear, peer code review](https://smartbear.com/learn/code-review/best-practices-for-peer-code-review/)
  (*Vendor*): defect finding falls beyond 200 to 400 lines at a time.
- [Why are commits being reverted?](https://swag.uwaterloo.ca/publications/why-are-commits-being-reverted-a-comparative-study-of-industrial-and-open-source-projects.html)
  (2016, *Academic*): 1 to 5% of commits are reverted, many for reasons other than defects.

**Effort levels.**

- [Anthropic, effort](https://platform.claude.com/docs/en/build-with-claude/effort) (fetched
  2026-11-09, *Vendor*): Opus 5.5 and Haiku 5.5 default to `medium`; Sonnet 5.5 levels are
  "recalibrated", so the same label does not mean the same work across versions; for agentic
  coding, Sonnet 5.5 `medium` for well-specified tasks and `high` for harder ones; run an effort
  sweep on your own evals.
- [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning) (fetched
  2026-11-09, *Vendor*): `medium` default on GPT-6.1 Sol; `xhigh` named for security and code
  review, "only use when your evals show a clear benefit".
- [Prompt-induced waste in coding agents](https://arxiv.org/abs/2608.01347) (2026, *Academic*,
  25 tasks per cell): Opus 5 resolved 17, 18 and 16 of 25 at `low`, `high` and `xhigh`, at $0.75,
  $1.89 and $3.62 per resolved task; only Sonnet 5 `high` vs `low` was significant.
- [Holistic Agent Leaderboard](https://arxiv.org/abs/2510.11977) (2025, *Academic*): higher
  reasoning effort reduced accuracy in the majority of runs.
  [Inverse scaling in test-time compute](https://arxiv.org/abs/2507.14417) (2025, *Academic*) and
  [The Danger of Overthinking](https://arxiv.org/abs/2502.08235) (2025, *Academic*): more
  reasoning can lower accuracy.

### What the survey says about our questions

- **Every serious method compares configurations on the same tasks** (replay, paired runs, fixed
  scaffold) or randomizes which task gets which configuration. Our rotation does neither yet.
- **Small samples only show large differences.** Practitioners with 20 to 50 tasks use them to
  catch large regressions, not to rank close models.
- **Tests and CI hide quality differences**; a review or rubric shows them, but an LLM reviewer
  is a noisy, biased instrument that needs a baseline, a rubric and a cross-check.
- **Effort is a configuration, not a dial that only improves results.** Its labels change between
  versions and vendors.

## Stress test

Each claim our method relies on, tested on our 163 rows (*Observed* unless marked).

### 1. "Reviewer blocking findings per PR are the main quality signal"

- **Data:** `reviewer_model`, `reviewer_blocking` and `regressions` are empty in all 163 rows.
  The signal has no data yet.
- **Design gap:** the gate reviews trial PRs only. Default PRs, which are most rows, never get
  the same review, so a trial's blocking count has no baseline. A trial model with 0.8 blocking
  findings per PR could be better or worse than Opus; nothing says which.
- **The reviewer is not blind:** its prompt names the author model and says "model trial". Opus
  never reviews Opus (the reviewer must differ from the author), so the proven model is never
  measured by the same instrument.
- **Size:** findings grow with diff size (Anthropic's vendor data: 0.5 per PR under 50 lines, 7.5
  over 1,000). Our models get different sizes: median added lines are Opus 717, Fable 429, Sonnet
  200.
- **Fix:** control reviews (D1); a blind prompt and a blocking checklist (part 3); report
  blocking findings within size bands. **Cost:** about 20 minutes per control review (*Est.*,
  from Anthropic's figure and our `/code-review` runs), about 1.5 to 2 hours per alpha at one
  default PR in four of about 25 to 40 merged PRs per alpha (163 rows over the six logged days).

### 2. "CI fix rounds measure the model"

- **Causes**, classified from the coordinator's notes (new column `ci_cause`): 46 rows needed a
  CI fix. A `check` cause (lint, spelling, docs, CodeQL alert) appears in 19, `flake` in 9,
  `drift` (main moved) in 6, `own` (a defect CI caught) in 4, `platform` in 2, and the notes do
  not say in 11. Only 23 of 46 have a cause the worker could have prevented (`own` or `check`).
- **The card's claim that Opus security CI fixes are "sandbox CI difficulty"** does not hold:
  of 16 Opus security rows with a CI fix, 2 name the platform; 8 are checks the worker could have
  run first (mostly CodeQL alerts on logs in tests, and Markdown lint); 6 are unexplained.
- **Simpson's paradox in our data.** Pooled, Opus needed a CI fix more often than Sonnet (26 of
  75, 35%, against 15 of 68, 22%). Restricted to the kind and size strata both models share
  (contained work in every size band, research work of unknown size), the direction reverses:
  Opus 3 of 23 (13%), Sonnet 14 of 55 (25%), Mantel-Haenszel odds ratio 0.42, Opus better. Both
  intervals are wide; neither direction is significant. The pooled figure compares Opus on
  security work with Sonnet on docs.
- **Fix:** count only `own` and `check` (done in the card); keep `flake` and `drift` for the
  repository's own health. **Cost:** about a minute per row at close-out.

### 3. "Nudges measure how reliable a model is"

- **Counting by hand drifts.** 2 of the 8 rows whose notes give an explicit nudge count had it
  wrong after backfill (#973 and #979, fixed). 8 rows whose notes report an idle stop count no
  nudge, because the coordinator noticed late or the worker woke on its own.
- **Idle stops are a date effect more than a model effect.** On 2026-11-06, 15 of 43 rows
  (35%, 22-50%) record an idle stop; on every other day together, 9 of 120 (8%, 4-14%). Sonnet
  idled 9 of 27 that day and 1 of 43 on all others. The likeliest cause is the worker prompt of
  the time (waiting on CI without a blocking loop), not the model.
- **The card's claim that Fable idles more** is weakly supported. Off that day: Fable 2 of 11
  (18%, 5-48%), Opus 5 of 65 (8%, 3-17%), Sonnet 1 of 43 (2%, 0-12%). The intervals overlap.
- **Fix:** a separate `idles` column (done); record the worker prompt version on each row
  (Proposed: the Torlek instructions id at start). **Cost:** none beyond the row.

### 4. "Re-compare every ~10 rows" and "10 records make a cell supported"

- **Width.** At an observed 80% success, a 95% interval is 49-94% at 10 rows, 58-92% at 20, and
  65-90% at 40.
- **Rows needed per arm** to detect a difference with 80% power (pooled normal formula,
  α = 0.05):

  | Success rates compared | Difference | Rows per arm |
  | --- | ---: | ---: |
  | 90% vs 70% | 20 points | 62 |
  | 80% vs 60% | 20 points | 82 |
  | 95% vs 85% | 10 points | 141 |
  | 90% vs 80% | 10 points | 199 |

  Our largest cell, Sonnet `high` contained, has 29 rows. At about 9 trials per alpha spread over
  6 or more trial cells, no cell reaches 60 rows in a month.
- **Peeking.** Re-comparing every 10 rows and acting on the first difference that looks real is
  repeated testing; the false-alarm rate grows with each look (Miller's example: 26% instead of
  5%).
- **Clustering.** 44 rows come from 12 issues with several PRs (#412 alone has 10, all Opus);
  those rows are not independent, so each such issue should count once.
- **Fix:** the [stopping rule](#stopping-rule); evidence labels tied to it (`none` under 5 rows,
  `screening` under 20, `decidable` from 20) beside every interval; one
  issue counts once. **Cost:** none.

### 5. "Trials are a fair comparison"

- **Rate.** 13 of 163 rows (8%) are trials, not one in three.
- **Choice of task.** The coordinator chooses which start becomes a trial. Fable's 8 trial rows
  are contained work except one docs and one research row; Fable never had a security
  task; Sonnet never had a risky or security task; Opus never had docs. Where a model never got a
  kind, nothing in the data can compare it there (no overlap).
- **Effort.** Sonnet `medium` against Sonnet `high` on contained work: a worker-preventable CI
  failure in 0 of 22 against 5 of 28, but median sizes are 47 and 424 lines. Within size bands the
  gap sits in large changes (≤100 lines: 0 of 12 against 0 of 3; 101-500: 0 of 3
  against 1 of 10; over 500: 0 of 3 against 4 of 9). No band has more than 3 rows on one side,
  and effort was chosen by judged difficulty. Our data cannot say whether `medium` is enough.
- **Fix:** decide which start is a trial by a fixed rule, logged before the start; never on
  security or data-loss work. **Cost:** none.

### 6. "The reviewer judges every model fairly"

- **Not testable on our rows** (no reviews yet). The survey says a Claude reviewer is likely to
  be lenient on Claude authors where they erred, which makes Claude look better than Codex. LLM
  reviewers also miss most defects (recall 0.25 to 0.69 on one vendor-run benchmark) and vary
  between runs, so "no blocking findings" is weak evidence of a clean PR.
- **Fix:** blind prompt and checklist (part 3); a second reviewer from the other family on a
  sample (D2); count regressions found after merge. Agreement between the two reviewers on the
  same PRs, split by author family, measures the bias directly. **Cost:** about 20 minutes per
  second review (*Est.*).

### 7. "Time to merge says something about the model"

- `open_to_merge_h` is empty in 29 rows and includes reboots, CI queues and rebases (#792, 6.1
  hours "reboot + rebase inside"). Medians are Opus 0.58 h, Sonnet 0.3 h, Fable 0.3 h, with
  Opus doing larger work. It measures the pipeline more than the worker. Keep it, do not rank by
  it.

### 8. "Model names identify the model"

- The `model` column says `Opus`, not `Opus 5.5`. Vendors recalibrate effort levels between
  versions, and the same name can change behaviour. **Fix:** write the version when a newer one
  is first used; a new version starts new cells. **Cost:** none.

## Recommended method

A cell is model × effort × task kind. Every row is logged as now. Decisions use only `trial`,
`control` and `replay` rows, because those were not picked by judgement.

1. **Assignment.** A start is a trial when the issue number modulo 3 is 0 and the kind allows
   it (never downward on security or data loss). The trial configuration is the least-sampled
   eligible cell for that kind. A default start whose issue number modulo 4 is 0 is a control
   (D1).
2. **Review.** Trial and control PRs get the independent review with the author's model removed
   from the prompt and a fixed blocking checklist: a missed acceptance criterion, a bug with a
   failure scenario, a missing regression test, a security or permission boundary, a broken
   repository rule. On a sample, a second reviewer from the other family (D2).
3. **Outcome per PR.** A PR has a defect when it has a blocking finding, a CI failure caused by
   `own`, or a later regression. `check` failures, idles and time to merge are reported beside
   it, not folded in.
4. **New model or version.** Before its first live trial, run it on the replay set (D3); stop if
   it fails two of four.
5. **Card.** `python3 -I docs/research/1264/model_card.py` prints counts, intervals and evidence
   labels. Read it at each alpha, not every 10 rows.

### Stopping rule

Per trial cell and per model version, with one issue counted once:

- **Early stop for harm:** 3 or more defects in its first 5 rows stops trials of that cell. If
  its true defect rate were 20%, this would fire about 6% of the time; at 60%, about 68%.
- **Decision at 20 rows:** the cell becomes eligible as a default for its kind when its defect
  share is at most the control share for that kind plus 10 points, and it is cheaper or faster.
  Otherwise the incumbent stays. At 20 rows against a similar control this only rules out
  differences larger than about 40 points; say so when reporting it.
- **Cap:** no more than 20 trial rows per cell and version. A new version starts again.

### Replay set (D3)

Four merged issues from this repository, chosen to discriminate and kept fixed: one contained
fix with a clear test, one cross-crate (risky) change, one security change and one docs or
research change. Each is rerun from its base commit with the original issue text as the brief, in
a fresh clone, and reviewed by the same blind reviewer as live trials, against the merged PR.
Tasks merged after the candidate model's training cutoff avoid most contamination. **Cost**
(*Est.*): 4 runs of about 40 minutes plus 4 reviews of about 20 minutes, about 4 hours per new
configuration.

### Budget per alpha (*Est.*)

| Part | Agent time |
| --- | --- |
| Trial reviews (already decided for alpha 8) | as now |
| Control reviews, 1 in 4 defaults (D1) | 1.5 to 2 h |
| Second-family reviews, 1 in 4 reviewed PRs (D2) | about 1 h |
| Blind prompt and checklist, once | 0.5 h |
| Replay, per new configuration only (D3) | about 4 h |
| Row bookkeeping and card | minutes per row |

## Decisions for the owner

Each is asked one at a time with `torlek agent report --needs-input`; each answer is recorded
on [#1264](https://github.com/Androlax2/agent-cockpit/issues/1264) when it arrives.

- **D1, control reviews.** A (recommended): also run the trial reviewer on 1 in 4 default PRs,
  chosen by number, logged as `trial=control`, so blocking findings per PR have a baseline; about
  1.5 to 2 hours per alpha. B: review trial PRs only; cheaper, and the main quality signal has no
  baseline. **Owner chose A on 2026-11-09.**
- **D2, second reviewer from another family.** A (recommended): once Codex trials start, a Codex
  reviewer at `high` also reviews 1 in 4 reviewed PRs with the same checklist, to measure
  family bias; about 1 hour per alpha. B: one Claude reviewer only; Codex authors may be judged
  more harshly than Claude authors and we would not know. **Owner chose A on 2026-11-09.**
- **D3, replay set.** A (recommended): before a new model's first live trial, run it on four
  fixed past issues, paired against the merged result; about 4 hours per new configuration. B:
  live rotation only; cheaper, and a weak new model is found only after it has written live PRs.

## Database moved into the repository

[`1264/`](1264/README.md) holds the CSV, the card script and how a row is added. Changes from
the coordinator's private copy:

- **New columns:** `ci_cause` (why CI failed) and `idles` (idle stops, nudged or not), both
  backfilled from the notes. `trial` takes two new values, `control` and `replay`.
- **Corrections:** #1016 and #1023 had 5 CI fix rounds from a backfill that read "5 batched"
  questions as CI rounds; their notes say one Markdown lint fix and clean, now 1 and 0. Nudges on
  #973 (1, not 2) and #979 (2, not 1) now match their notes.
- **Removed:** the #892 row, a no-PR audit, which is not a pull request.
- **Notes:** names of people replaced by "owner", quoted session text paraphrased.
- **Script:** the one-off backfill from the old Markdown log is gone; the card validates every
  row, counts only worker-preventable CI failures, prints intervals, and labels evidence from the
  issues with a `trial`, `control` or `replay` row, as the stopping rule counts them.

## What this does not show

- No model ranking. Our rows cannot rank Opus, Sonnet and Fable on any kind; the survey's
  numbers are on other repositories and mostly vendor-reported.
- Nothing about Codex or Haiku in this repository: there are no rows.
- No measured reviewer bias or recall here: no reviews have run yet.
- Agent-time costs are estimates; none was measured per review in this repository.
- The `ci_cause` and `idles` backfill reads the coordinator's notes, which were not written for
  it; 11 CI causes stay `unknown`.
