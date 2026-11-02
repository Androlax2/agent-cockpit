# CI efficiency: two-week census and a reproducible baseline

Part of [#397](https://github.com/Androlax2/agent-cockpit/issues/397). Measured on
2026-11-02 against checkout `ea5458970720cf24dd807c21a1cdb808085ed5a4`.

**Result:** the latest 30 merged PRs consumed **39.94 CI runner minutes per merged PR**,
including their corresponding `main` runs. The rounded, weighted comparison model gives
**143.97 minutes**, not a bill. Median opening-to-merge time was **14.575 minutes**.
These are baseline observations, not an accepted before/after improvement.

## Scope and provenance

The census covers run creation from **2026-10-19 00:00:00 UTC to 2026-11-02 00:00:00 UTC**.
Six REST pages returned 503 distinct run IDs; the earliest was 2026-10-29 14:44:16 UTC.
There were 125 merged PRs in the window. Detailed CI jobs were retrieved for the latest
30 of those PRs, merged between 2026-11-01 16:34:45 and 23:58:24 UTC: 51 PR runs and
30 `main` runs. This deliberately bounded cohort is **not** all two-week CI job time.

Retrieval was read-only, with at most two concurrent requests per collector: six census
pages, 81 CI job responses, 30 PR timelines, 12 Release job responses, two timing responses
and four job-log attempts, plus fixed repository/rules/cache metadata. Responses are cached
in this checkout's ignored `.handoff/ci-397/`; no workflows were dispatched or rerun.
The selected job and timeline responses were untruncated. Job queries used `filter=all`,
including cancelled and failed attempts; timestamps reflect evidence available at retrieval,
including completions after the census cutoff.

| Workflow/event | Two-week runs | Detailed runner minutes | Rounded weighted minutes |
| --- | ---: | ---: | ---: |
| CI / PR | 209 | 795.10 (51-run cohort) | 2751 (cohort) |
| CI / main push | 126 | 403.03 (30-run cohort) | 1568 (cohort) |
| Release / PR | 11 | 198.75 (all 11) | 1378 |
| Release / schedule | 1 | 19.35 (all 1) | 136 |
| Invalid `.github/workflows/prerelease.yml` / push | 156 | unavailable; jobs not retrieved | unavailable |

The invalid prerelease workflow is historical, already addressed by
[the earlier #397 change](../decisions/2026-11-01-397-ci-cost.md). Release jobs were measured
separately; they are not assigned to the CI-only per-PR denominator. Full two-week CI job
totals and whole-repository minutes per merged PR remain unavailable from this bounded sample.

## CI job measurements

Runner time is the sum of `completed_at - started_at`, excluding skipped jobs. Cancelled
jobs with timestamps still count. Weighted minutes round **each job attempt** up to a minute,
then multiply Linux by 1, Windows by 2 and macOS by 10. This is the issue's historical
comparison model, not a statement about current prices or actual account charges.
Medians below include successful jobs only; totals include failures and cancellations.

| Job | PR / main successes | PR / main median seconds | Total runner minutes | Weighted minutes |
| --- | ---: | ---: | ---: | ---: |
| changes | 57 / 30 | 6 / 5 | 8.03 | 87 |
| docs | 55 / 29 | 20 / 20 | 53.98 | 108 |
| greenfield | 46 / 24 | 395 / 381 | 526.48 | 568 |
| greenfield-macos | 47 / 22 | 158 / 187 | 227.32 | 2710 |
| greenfield-wsl2 | 53 / 26 | 293 / 289.5 | 382.32 | 846 |

The Linux gate step's successful median was 359 seconds (70 jobs). WSL's successful
`setup-wsl` median was 79 seconds, startup 133 seconds, and resource tests 58 seconds
(79 observations each). These step durations identify where time goes; they do not prove
how much a new cache or test split would save.

### Billing and cache evidence

The repository API reports `visibility: public`. Timing responses for
[main run 41364679637](https://github.com/Androlax2/agent-cockpit/actions/runs/41364679637)
and [PR run 41363337039](https://github.com/Androlax2/agent-cockpit/actions/runs/41363337039)
report zero billable milliseconds for every listed platform. This is available billing
evidence for those two runs only. Account allowance, remaining included minutes and the
two-week billed total were not retrieved; repository-scoped credentials are not account
billing authority. The analyzer therefore leaves actual billed minutes unset.

The cache inventory contained **17 entries, 10,674,907,763 bytes**, all on `refs/heads/main`.
Successful restore-step execution is not a hit. Three retrieved logs explicitly say
`Restored from cache key` with `full match: false`:

- [main Linux, job 125022244041](https://github.com/Androlax2/agent-cockpit/actions/runs/41364679637/job/125022244041)
- [main macOS, job 125022244038](https://github.com/Androlax2/agent-cockpit/actions/runs/41364679637/job/125022244038)
- [PR Linux, job 125020587082](https://github.com/Androlax2/agent-cockpit/actions/runs/41363337039/job/125020587082)

That is three confirmed fallback restores, zero exact matches in the retrieved sample.
The fourth requested log (PR macOS job 125018224285) failed on
`productionresultssa16.blob.core.windows.net`: `net/http: TLS handshake timeout`.
It was not retried; Cockpit blocker `a91b3844` records it. Neither a population hit rate nor
eviction frequency is measured. Cache capacity changes and another caching action are not
justified by this inventory alone.

## Reruns and merge latency

The cohort has **1.70 distinct PR CI runs per PR**, plus **0.20 explicit reruns per PR**:
57 attempts in 51 run IDs. Eleven PRs had more than one run ID; six explicit reruns occurred.
Pushes and rebases both create new run IDs; this evidence does not distinguish their causes.
Across the full census, 17 PR CI run IDs have retries, totalling 19 additional attempts.

The timelines contain no draft/ready transitions for these 30 PRs. Their **14.575-minute
median uses creation as a readiness proxy**, not a measured author-approval or CI-ready
timestamp. The tool uses the last `ready_for_review` event when present and labels the
creation fallback. Workflow conclusions in the cohort were PR: 41 success, 8 failure,
2 cancelled; main: 20 success, 5 failure, 5 cancelled. These are latest-run conclusions,
not attempt-level outcomes or an attribution of failures to TUI flakes.

| PR | PR runs | Explicit reruns | CI runner min including main | Weighted min | Opening to merge min |
| --- | ---: | ---: | ---: | ---: | ---: |
| 395 | 5 | 0 | 92.87 | 261 | 61.35 |
| 401 | 3 | 0 | 64.45 | 206 | 37.30 |
| 404 | 1 | 0 | 28.62 | 90 | 7.93 |
| 409 | 1 | 0 | 30.03 | 116 | 6.60 |
| 413 | 1 | 0 | 26.47 | 106 | 5.58 |
| 421 | 1 | 0 | 17.48 | 74 | 19.22 |
| 422 | 1 | 0 | 29.27 | 101 | 19.60 |
| 424 | 1 | 0 | 27.03 | 95 | 26.82 |
| 428 | 1 | 0 | 27.88 | 108 | 8.62 |
| 429 | 1 | 0 | 25.68 | 93 | 7.92 |
| 430 | 2 | 0 | 42.20 | 167 | 15.20 |
| 438 | 1 | 0 | 27.80 | 96 | 8.08 |
| 441 | 1 | 0 | 21.27 | 69 | 6.90 |
| 445 | 1 | 0 | 28.68 | 101 | 7.03 |
| 447 | 3 | 0 | 44.68 | 171 | 13.95 |
| 449 | 3 | 0 | 51.98 | 177 | 28.63 |
| 453 | 2 | 0 | 44.15 | 158 | 23.70 |
| 469 | 1 | 1 | 42.03 | 157 | 19.07 |
| 472 | 3 | 0 | 51.53 | 200 | 44.00 |
| 473 | 1 | 0 | 29.42 | 119 | 7.47 |
| 477 | 2 | 0 | 40.62 | 165 | 18.55 |
| 481 | 1 | 1 | 43.98 | 151 | 12.83 |
| 484 | 1 | 0 | 30.78 | 109 | 7.28 |
| 485 | 3 | 1 | 68.22 | 280 | 30.90 |
| 486 | 1 | 0 | 31.15 | 102 | 8.37 |
| 488 | 1 | 1 | 47.05 | 183 | 17.97 |
| 489 | 4 | 1 | 89.52 | 321 | 58.80 |
| 490 | 2 | 1 | 55.62 | 202 | 23.72 |
| 491 | 1 | 0 | 23.30 | 91 | 8.82 |
| 496 | 1 | 0 | 14.37 | 50 | 0.68 |

## Actual workflow and protections

The live `main` CI workflow matched the checkout. Its concurrency cancels superseded runs;
main runs every job; PRs use dependency-aware platform selection; draft PRs run checks.
Linux and macOS already use the pinned rust-cache with main-only saves. Docs already use
the checksum-verified lychee binary. These shipped in #404 at
`f25c73c5aa3605a73c1b038618539ee166a56dc2` (2026-11-01 16:36:29 UTC).
This cohort straddles that rollout; it is not a clean post-change window.

The ruleset list contained one active ruleset,
[26273395](https://github.com/Androlax2/agent-cockpit/rules/26273395): required PR,
`changes`, `greenfield`, `greenfield-macos`, `greenfield-wsl2`, `docs`, squash only,
no bypass actors, deletion and non-fast-forward protection. Crucially,
`strict_required_status_checks_policy` is **false**: merges do not mechanically force
every other PR to rebase. Auto-merge is enabled; merge commits and rebase merges are disabled.
The repository owner is a **User**, not an organization; the earlier decision records
merge queue's organization requirement. There is no active merge queue here.

## Smallest justified next actions

1. **Delivered here:** an offline, fixture-tested measurement method. It catches reused
   branches (PRs #421 and #429 share one), includes all job attempts, refuses incomplete job
   pages, and distinguishes weighted time from billing. No measured saving is claimed.
2. **Continue the existing flake lane:** #444/#483 already have an owner. Six reruns and
   failed platform jobs make reducing retries worth measuring, but no failure cause or
   potential saving is assigned without that lane's evidence. No duplicate implementation.
3. **Measure a clean comparison before further tuning:** Linux is the longest successful
   median job; macOS dominates weighted time. Path splitting and cache changes require
   changed-file/dependency attribution and cold/warm samples before claiming savings.

Morning choices, all still proposed:

| Option | Benefit | Cost / decision |
| --- | --- | --- |
| A: Keep current protections and collect matched windows (recommended) | No loss of proof; isolates whether existing changes help | Delays an acceptance claim until sufficient observations exist |
| B: Further dependency-specific platform selection | May avoid work that proves nothing for a changed path | Must first demonstrate coverage and savings; workflow push needs host approval |
| C: Merge queue or post-merge platform proof | Changes integration scheduling | Queue eligibility/setup needs Theo; post-merge proof weakens the current pre-merge guarantee and is not recommended |

No global draft suppression: #444 and generated release drafts depend on checks. Release,
prerelease and release-plz automation remain with `greenfield-493-release-delivery`.

## Reproduce and complete acceptance

The analyzer uses Python 3.11+ and makes no network requests or repository writes:

```sh
python3 tools/ci-metrics.py .handoff/ci-397 > .handoff/ci-397/summary.json
python3 -m unittest discover -s tools/tests -p 'test_ci_*.py'
python3 -m unittest discover -s tools/tests -p test_workflow_syntax.py
```

For another window, cache these GitHub CLI responses, using explicit dates and a page/request
budget before retrieval. Save raw responses rather than relying on mutable CLI summaries:

- `runs.json`: flatten `workflow_runs` from bounded pages of
  `gh api 'repos/Androlax2/agent-cockpit/actions/runs?created=START..END&per_page=100&page=N'`.
  Verify distinct IDs equal the API total; split the date range if the search cap is reached.
- `sample-pulls.json`: select a fixed merged cohort from `gh pr list --repo Androlax2/agent-cockpit
  --state merged --search 'merged:START..END' --limit 300 --json
  number,title,createdAt,mergedAt,headRefName,mergeCommit,url`. Keep the full response separately.
- `sample-runs.json`: all CI runs belonging to that cohort: PR number links when supplied;
  otherwise branch name **and creation between PR opening and merge**; main by merge SHA.
  Inspect the selection against the full census before analysis. A reused branch alone is
  not enough. Extend the census backwards if a PR opened before its start.
- `jobs-ID.json`: `gh api 'repos/Androlax2/agent-cockpit/actions/runs/ID/jobs?filter=all&per_page=100'`.
  The tool refuses truncated responses or missing attempts. Combine all pages if necessary.
- `timeline-NUMBER.json`: `gh api 'repos/Androlax2/agent-cockpit/issues/NUMBER/timeline?per_page=100'`.
  The current tool refuses 100 or more events rather than guessing whether pagination is complete;
  a larger history needs explicit pagination support before including that PR.

Raw evidence is local, not a committed archive. The PR/job links above and cohort table
identify the observations for re-fetching; API retention can eventually make them unavailable.
The analyzer does not validate that the collector selected every eligible run: census
completeness is an explicit collection check, not a claim made by a passing fixture.

**Acceptance remains open:** agree what "PR ready" means (author-ready versus CI-ready),
then record at least 15 PRs on each side of a single change with the same cost, event,
attempt and latency definitions. Include main/release allocation consistently and report
draft transitions, cancellations, failures, cache sample denominators and missing evidence.
The earlier 71-PR report and this mixed 30-PR sample are not a controlled before/after pair.
Keep platform proof and the release-tag gate intact; do not close #397 from this baseline.
