# Candidate release gates (#191)

Date: 2026-11-02. Status: implemented; candidate CI and previous-release evidence pending.

## Alpha1 platform scope amendment

Theo selected Linux and WSL2 only for Alpha1 on 2026-11-02 (#191/#97). This
supersedes the macOS distribution/acceptance requirements below for this release;
the earlier measurements remain historical evidence. #527 and #564 stay open,
deferred under #568. Public support/guide documentation is owned by the separate
documentation change for #568.

`release-plan` selects only Linux x86_64 and aarch64 build/install matrices for the
current `0.1.0-alpha.N` attempts and `0.1.0-dev.N` candidates. WSL2 installs the
x86_64 archive and retains its independent checks. Baseline fetching and upgrade
matrices consume that same plan. Packaging refuses a macOS Alpha1 payload before
creating output; no experimental macOS download is promised. Other alpha cores
are refused pending a reviewed scope decision, rather than inheriting this exemption.

The future macOS matrix definitions and native-Intel execution check remain. Beta
retains those requirements and now explicitly waits for owner-approved, verified
macOS restoration (#568), as well as the unproven previous-channel running-session
upgrade. Stable remains disabled. Ordinary developer CI, including its required
macOS context, is unchanged by this policy.

Publication still depends on all nine existing prerequisite jobs succeeding. The
new `verify-gates` entry point also checks their actual `needs` results: skipped,
failed, cancelled or absent Linux/WSL2, compatibility, responsiveness, gate or
same-candidate state checks are refused. macOS has no Alpha1 matrix entry and no
separate publication dependency. The existing tag-only publication predicates,
draft guards and ready-for-review triggers remain in place.

## Bounded termination-evidence retention

The latency candidate explicitly captures the fixture daemon's four known rotating
logs before its temporary home disappears, including panic unwinding. Only the two
correlated #537 lifecycle messages qualify; retained JSON contains timestamp,
session, execution and diagnostics, never arbitrary logger fields or provider output.
The report is appended after its last rewrite (or during unwinding), preserving the
original verdict. Reads are capped at 4 MiB plus one overflow byte per file; output
is capped at 32 records and 256 KiB, including status text.

Missing/unreadable current logs, malformed/partial/uncorrelated records and truncation
are labelled incomplete. A normal capture error fails visibly; the unwind fallback
reports an artifact-write failure without replacing the original panic. A clean scan
with no matching records is explicitly not a successful-termination verdict.

Verification before final review: six new fixture tests failed against the stub;
all twelve retention/verdict/deadline/cleanup fixtures then passed, as did latency
Clippy and Rust formatting (host approval `65e6b8e7bf02`). Offline resolution first
failed in the session (`tempfile`) and host (`crc32fast 1.5.4`); host approval
`e578c814cf23` authorized the locked dependency fetch and focused compile. No
sustained workload was selected. #537's historical root cause remains unproven.

The Alpha1 plan/publication contract tests initially failed (52 assertions), then
`make release-check` passed: 71 release tests, 10 plan/publication tests and 13
upgrade tests. Eight workflow tests and changed-file Ruff checks pass. The draft
integration initially exposed ten expected failures for the five added candidate
jobs; their explicit guards now pass. An existing workflow-test parser read only
the first job property; it now reads the whole job when checking dependencies.
Actionlint is unavailable and **NOT RUN**. Final review and the separately approved
single 300-second candidate execution remain required before delivery acceptance.

The independent full-delta `code-review medium` against `63a8194`, staged tree
`a8a9404`, found missing `candidate` module wiring in the incoming native-instruction
test, retryable evidence capture, duplicate failed-startup cleanup and duplicate
candidate hashing. Regression fixtures reproduced both Python failures; host request
`b1132950199f` reproduced the missing-module compile error and failed capture-retry
fixture. Capture now consumes its one attempt before reads/output, startup cleanup
has one owner, and candidate hashing runs once while required binaries/helpers remain
checked. Native-instruction module wiring is explicit.

Host request `c57df3de6e44` passed the native-instruction compile check, all 13 focused
release fixtures (three other tests skipped), all-target daemon Clippy with warnings
denied, and workspace formatting, offline with two build jobs and one test thread.
The combined release Python suite passed 135 tests; changed-file Ruff check/format
and diff checks passed. The first Ruff check caught import order and a late import
in the new regression tests; both were corrected. The session's identical missing
`tempfile` failure after approved restart `8e18098dd17e` remains separate evidence;
no repeated restart or candidate workload was used to obtain these results.

## Decision

Use the archive produced by the release workflow for native Intel macOS execution,
protocol skew, killed-daemon restart and bounded responsiveness. The shared daemon test
fixtures select that archive only when `COCKPIT_CANDIDATE_ROOT` is explicit; ordinary tests
keep Cargo's binaries. Linux also checks the running process's executable path. Fixture
homes stay private and use stand-in providers and sandbox engines.

The responsiveness gate runs for five minutes in CI, with at most two build stand-ins and
two database writers. It enforces report 03's existing latency targets and rejects missing
or failed load operations. Artificial-sample tests cover verdicts locally; no sustained
local load is authorized. Linked migration/interrupted-restore fixtures run at the candidate
source revision and are not described as archive execution.

A read-only baseline job fetches the newest published earlier release of the same channel,
its platform archive and checksums. Upgrade jobs on Linux, macOS and WSL2 validate these
before execution. Old and new daemons never overlap: each process group is stopped and
reaped before the next opens the temporary state. The fixture registers a Git project,
retains its task unchanged, then verifies a new task can be written after startup. No
credential or user configuration is passed to either daemon.

Alpha remains exempt from a previous-release upgrade. Development dry runs are not a
release channel. Both still exercise the state fixture using the same candidate twice and
report the cross-version upgrade as not run. Missing earlier beta releases fail explicitly;
there is no fallback to a draft, alpha or synthetic baseline. Stable releases remain disabled.

## Evidence and limits

- `make release-check`: release packaging/gate wiring and isolated upgrade CLI fixtures.
- `release_targets_*`: threshold boundaries, missing/failed samples and failed load verdicts.
- Actual Linux archive from run 41383904831, revision
  `1830cfcd9b84ae1c1e46ecbf84abad5942585c9b`: the new state fixture passed in a private home.
- Native platform execution and the five-minute measurement require this PR's CI results.
- No previous release is published yet. Cross-version upgrades remain unmeasured; this
  fixture covers project/task persistence, not running sessions across different versions.
  Same-candidate restart/reattach coverage does not close that gap; it remains a named beta
  publication blocker in `GATE_CHECKS`. #191 stays open.
- No tag, release publication, tool installation on Theo's host or host configuration change
  is part of this implementation. Workflow pushes retain host approval.

## Review record

The requested `$code-review medium` skill was loaded from
`/home/theo/.codex/skills/code-review/SKILL.md` and run inline on base
`58205740fe18fa4d11313cbcfb7f4d38a45a27bb` plus the working #191 delta. It found missing
release-trigger paths, an overstated archive-coverage description, and a verdict that could
accept wholly failed load operations. Those are corrected here. This preliminary review
is separate from the fresh independent review of the completed branch delta.

A fresh independent reviewer loaded and ran the same actual `$code-review medium` skill
on `58205740fe18fa4d11313cbcfb7f4d38a45a27bb..0c004c618b5e06c14f11c3a05e559b505e126a70`: all
eight inline angles, deduplication only, no verifier pass, findings `[]`. No actionable
correctness or repository-rule findings. It ran no tests, load, edits or Git operations.
This review does not claim the pending native-platform or sustained CI acceptance.

## CI follow-up

The first PR run failed Linux Clippy before tests (run 41387562381, job 125091720841):
`launch_root_order` imports both `common` and `real_daemon`, so their private declarations
loaded `candidate/mod.rs` twice. The preliminary focused lint targets and independent
review missed that composition. No test or lint was disabled.

Each daemon test entry point now declares `candidate` once; both fixtures import that root
module. The check expanded to `cargo clippy --locked -j 2 -p cockpit-daemon --tests -- -D warnings`
and passes, including the previously failing target. The initial mechanical edit missed
module declarations on line 1; the expanded check caught and resolved those imports before
push. Relevant archive entry-point tests are rerun for the corrected wiring.

## Responsiveness timeout follow-up

At revision `b87cf3e`, required CI passed (run 41388141390), after one host-approved macOS
rerun for the existing #483 TUI failure. Release run 41388141560 passed every archive,
install, compatibility and same-candidate state job, including native Intel macOS and WSL2.
The responsiveness job 125098073369 exhausted its 25-minute limit: compilation took 85
seconds, then the test ran for over 23 minutes without a report. This is not acceptance.
Auto-merge remains off until the corrected candidate jobs are measured.

An idle fake socket reproduced an independent harness deadline bug: a 100 ms sampling
window kept waiting beyond two seconds. The loops previously drained all scheduled
requests even after the measurement window, each with a ten-second timeout. They now
stop at the window plus one second and record incomplete sampling as a failure. Thresholds
and workload stay unchanged. Both held and fresh connection paths have fixture coverage.

Phase progress is persisted before setup, sampling and cleanup. Samples are saved before
load shutdown; load joins have a common two-request deadline plus grace. Build groups use
an explicit negative-PID delimiter and bounded reaping. The existing kill syntax worked
in the local idle-process experiment; it is not claimed as the CI timeout's cause. An idle
process fixture verifies cleanup, and a report fixture rejects a completed-run claim while
load/restart results are pending. No local workload or stress test was run. The precise CI
stall phase remains unknown until the next instrumented run.

A second fresh independent `$code-review medium` was warranted by the substantial harness
follow-up. It reviewed the full `5820574..b87cf3e` PR delta plus the four changed working
files (latency source SHA-256 `ceb680819a31906e0531089e9272473b272ce5efd2a37b77e9176d8bd6289d23`).
It ran all eight inline angles, deduplication only, no verifier, and returned `[]`.
Six focused fixture/verdict tests and latency Clippy pass; formatting, docs and diff checks
pass. The measured candidate CI result is still required.

## Measured candidate result and remaining blocker

Required CI run 41393259764 passed at `b7c4c12` on its first attempt: Linux recorded 2,219
passed and 35 skipped tests; macOS, WSL2, docs and changes also passed. Release run
41393259723 passed every archive, install, compatibility and same-candidate upgrade job.
Native Intel macOS and WSL2 archive commands are measured, not inferred from the build.

The corrected responsiveness test finished in 306.154 seconds and retained its
[report artifact](https://github.com/Androlax2/agent-cockpit/actions/runs/41393259723/artifacts/13119610104).
Actual packaged source revision: `1ae7643d6fa8b3090019dd14e724d15af8bc64a1` (the PR merge
checkout). All 6,000 held and 300 fresh probes answered: held p95 8.9 ms, p99 12.7 ms,
maximum 151.3 ms; fresh maximum 12.3 ms. The killed-daemon restart was ready after 251 ms,
reattached every session and kept 619 launches unchanged.

The overall gate **failed**, as intended: the restart load had one failed operation out of
61 and session churn had one out of 1,228. Both reported processes still present after
SIGKILL. The two item writers completed 46,712 and 46,760 operations without failures.
The report cannot distinguish live survivors from zombie observation or a lifecycle race.
[Alpha 1 blocker #537](https://github.com/Androlax2/agent-cockpit/issues/537) records the
failure and owning process/session paths for Chief to route. No threshold, load-success
requirement or ownership check is relaxed. PR #523 auto-merge stays off; a corrected
candidate must pass bounded CI before acceptance. No unchanged rerun or local stress loop
is used to replace this failed evidence.

## Sources

- [GitHub hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
  lists `macos-26-intel` as a native Intel runner.
- [GitHub release listing](https://docs.github.com/en/rest/releases/releases#list-releases)
  requires only repository contents read access and distinguishes draft releases.
- [GitHub CLI release download](https://cli.github.com/manual/gh_release_download)
  supports explicit repository, tag and asset patterns.
