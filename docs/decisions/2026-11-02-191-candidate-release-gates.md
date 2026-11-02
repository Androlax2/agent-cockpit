# Candidate release gates (#191)

Date: 2026-11-02. Status: implemented; candidate CI and previous-release evidence pending.

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

## Sources

- [GitHub hosted runners](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
  lists `macos-26-intel` as a native Intel runner.
- [GitHub release listing](https://docs.github.com/en/rest/releases/releases#list-releases)
  requires only repository contents read access and distinguishes draft releases.
- [GitHub CLI release download](https://cli.github.com/manual/gh_release_download)
  supports explicit repository, tag and asset patterns.
