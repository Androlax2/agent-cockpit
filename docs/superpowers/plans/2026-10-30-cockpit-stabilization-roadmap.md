# Cockpit Stabilization Implementation Plan — proposed roadmap

> **For agentic workers:** After Theo approves the relevant phase, use the
> `executing-plans` skill for inline implementation task-by-task. Delegation requires
> separate authority; this review was explicitly limited to one reviewer. Checkboxes
> below are proposed work, not evidence of execution or permission to run commands.

**Goal:** Produce an exact-source verified Cockpit release whose UI/control paths remain
responsive and whose agents make measurable useful progress within existing safeguards.

**Architecture:** Retain the five crates under `apps/`. Keep authoritative registry and
Team/Chief persistence owners; move slow observation/transport work behind bounded,
explicit operations. Improve existing context/delivery contracts before considering more
crates or process separation.

**Tech Stack:** Rust, SQLite, Unix IPC, GTK/GLib, terminal/PTY clients, Python/shell tools,
Make, GitHub CI and Linux/Arch packaging; existing dependency set unless separately approved.

**Spec:** [Architecture review and options](../../audits/audit-2026-10-30.md),
[secure autonomy/performance contracts](../../audits/2026-10-30-cockpit-autonomy-performance.md)
and [pinned evidence](../../audits/2026-10-30-cockpit-evidence.md).
These are recommendations awaiting phase approval, not an already approved new design.

## Global constraints

- First supported deployment remains the current Linux environment; native package target
  is Arch x86_64. Headless does not imply cross-platform portability.
- Preserve installed commands, `apps/target/release`, compatibility exports and existing
  App/CLI entry points. Do not relocate the workspace during stabilization.
- Preserve Team/Chief connection, migrations, transactions, `Storage` errors, WAL/FULL
  durability, unread decisions/completions and authorization provenance.
- Preserve Greywall/Greyproxy, project-scoped credentials, peer/runtime validation,
  revocation, rootless-project Docker, admission, gaming limits and full-suite locking.
- No new manifest/tool dependency, install, protected policy, Git action or host activation
  is authorized by this plan. Existing user authorization remains the source of permission.
- Inspect the current owner/checkpoint before touching each area. The pinned I/L snapshots
  are research baselines, not instructions to overwrite current work.
- Call out changes to host-executed Makefiles, scripts, hooks, package/activation recipes
  and IDE/container configuration. Review final effective inputs before host execution.
- Keep designs and active plans. Any UI state/layout change follows applicable approved
  design and accessibility workflow; this is not a sidebar redesign plan.

## Review focus

- **Late result after restart/revocation:** S1/S2/S3 must reject stale runtime/generation
  results, including PID reuse, without losing valid state.
- **Failure after durable acceptance but before reply:** S3/C2 must retain idempotency
  and report uncertain outcome; a timeout cannot silently cancel or duplicate a side effect.
- **Unread backlog under churn and database failure:** C1 must converge without omitting
  decisions/completions or publishing success after a failed write.
- **Changed script after approval or mixed-source deployment:** D0/A2/R1 must bind final
  source/content identity to verification, approval and actual running artifacts.
- **Platform and concurrent-fixture differences:** D0/R1 must exercise the declared shell,
  fresh HOME, concurrent launch, missing optional tools and source-free package paths.

## Phase order and owners

```text
D0 exact baseline + CI failure evidence
  ├─ S1 investigator-owned nonblocking sidebar scheduling/display
  ├─ S2 bounded pane/store observation
  └─ S3 bounded ingress/response transport
         ↓
G1 sustained responsiveness + identity/fault gate
  ├─ C1 compact context + bounded recovery
  ├─ C2 delivery/outcome supervision
  └─ A1 sandbox checks + read-only evidence pilot
         ↓
A2 capability/receipt guidance + reviewed host batching (separate approvals)
         ↓
R1 exact-source native release validation
         ↓
G4 measure change amplification; reconsider layout/process split only if justified
```

Independence in this graph is not permission to launch parallel workers. Use the existing
owners and resource scheduler; shared-file changes need checkpoint/hunk agreement.

| Unit | Existing owner / coordination | Bounded deliverable | Planning estimate |
| --- | --- | --- | --- |
| D0 | Workflow `5fd3baec`, integration owner `ses_e7e1238ddfferBKBXaGITxBLCs`, package `1a7c54da` | Current CI failure disposition plus one candidate-source/runtime ledger | 1–3 engineer-days |
| S1 | Separately assigned freeze investigator; coordinate original sidebar `56954d2c` | Responsive scheduling/display with authoritative action checks | 1–3 days after diagnosis |
| S2 | Pane `c005677d`, reconciliation `153066df`, diagnostics `8e473428` | One observation class at a time, bounded and identity-checked | 2–4 days per first class |
| S3 | Daemon owner agreed with S2/diagnostics; do not assume an unowned area | Queue/response bounds with peer-fault proof | 1–3 days |
| C1 | Reconciliation/context owners `153066df`, `c1ecab09`, `b90b30c5` | Revision deltas and bounded convergent recovery | 3–5 days |
| C2 | Supervision `5b91edcf`, autonomy `b77a0881`, quota owners | Admission/outcome distinction and visible retry/expiry | 2–4 days |
| A1 | Workflow owner plus sandbox/credential owners by explicit scope | One project-local check/evidence pilot | 3–5 days |
| A2 | Skills `03501782`, batching `ad70761f`, pending approvals `53e8ad63` | Reliable discovery/receipts; approved guidance/batching updates | 1–3 days per approved slice |
| R1 | Package `1a7c54da`, workflow `5fd3baec` | Native lifecycle/provenance and approved live verification | 2–5 days plus approvals |

Ownership status is a dated observation, not reassignment. Recheck supported task/session
state before execution. Phone cache `614fa5a0`, gaming `d021dc46` and diagnostics
`8e473428` remain protected parallel scopes. No new duplicate tickets were filed here.

## Phase 0 — establish the delivery truth

### D0: Repair the exact CI failure and reconcile the release candidate

**Files:** `.github/workflows/ci.yml`, `Makefile`, `tools/quality.py`,
`apps/cockpit/tests/greywall_launcher.rs`, the launcher/shim owner found by the failing
entry point, `docs/DEVELOPMENT.md`, `docs/ARCHITECTURE.md` and existing release ledger.
Do not edit all these files automatically: only the proven failure owner and necessary
evidence/doc wiring belong in the change.

**Interfaces:** consumes exact checkout/task delta, CI run and runtime receipts;
produces one candidate identity and a verification ledger with source, tool/environment,
command, pass/fail/unrun counts and deployment state. The ledger is evidence, not authority.

- [ ] Refresh base/head, current owners, index/worktree/untracked artifacts and live-only
  changes. Compare the live Chief wake behavior explicitly before selecting the candidate.
- [ ] Preserve run `41079202527` and reproduce its concurrent-launch failure in the declared
  Linux environment with fake host commands and independent fixture roots. Determine
  whether shared shim publication is product behavior, fixture setup, or both.
- [ ] Repair the actual owner. Add the narrow entry-point regression for concurrent
  launcher startup; keep the existing model-catalog POSIX regression. Do not disable,
  weaken or rerun-away a failing test.
- [ ] Verify the affected launcher/menu targets, then the final candidate's canonical
  gate under admission. Record zero-selection/unrun tests explicitly. Verify remote checks
  against the candidate/merge result, not just an earlier head.
- [ ] With the workflow owner, inspect actual required-check configuration and propose
  any necessary policy change to Theo. An empty ruleset query is not full protection evidence.

**Acceptance:** exact current failure explained and corrected; source changes and fixtures
reviewed; final local/remote gate outcomes visible; live-only behavior reconciled.

**Compatibility/rollback:** preserve launcher commands and installed paths. No schema or
live activation in D0. A failed candidate stays undeployed; do not reset user branches.

**Stop/go G0:** no structural/runtime migration atop an unidentified source or known-red
required gate. Urgent incident diagnosis may continue read-only with the existing owner.

## Phase 1 — responsive UI and control

### S1: Complete the sidebar fix at the traced blocking caller

**Files:** `apps/cockpit/src/app.rs`, `apps/cockpit/src/app/continuity_ui.rs`,
`apps/cockpit/src/app/chief_ui.rs`, `apps/crates/cockpit-core/src/queue.rs`,
`apps/crates/cockpit-core/src/chief.rs`, `apps/crates/cockpit-core/src/teams/client.rs`;
tests `apps/cockpit/src/tests/sidebar_team_fetch.rs`,
`apps/cockpit/tests/sidebar_refresh.rs`, `chief_ui.rs`, `team_client.rs`.

**Ownership/evidence update:** dedicated investigator `ses_12729702f195758623a8976d`
owns diagnosis, repair and live acceptance. Already-collected evidence supports accumulated
synchronous scheduling IPC; Chief reports selecting a single-flight background worker,
with its answer queued rather than acknowledged at that update. `68e5cb51` closed source
access only. Use the evidence appendix; this audit neither repeats the investigation nor
makes its completion a prerequisite to delivering the report.

**Interfaces:** existing `dispatch_queue()` → `queue_context()` → `team_snapshot()`
is a source-confirmed synchronous observation path; its 30-second due check can fetch
before queue-enabled decisions when queued/Chief-requested work remains. Subsequent
fresh/existing-session reservation loops are action IPC, not display-only reads.
Existing `chief::host_snapshot() -> Result<Value, TeamError>` and
`teams::client::host_request_until(path, request, deadline) -> Result<TeamReply, TeamError>`
are current seams. A new display collector, if needed, returns a snapshot/error tagged
with requested generation and completion time; it does not execute host actions or
invent unavailable state. Name/shape approval belongs to the traced owner before coding.

- [ ] The existing investigator carries the caller evidence into a regression fixture
  and final-candidate identity receipt. Preserve source attribution limits and distinguish
  instrumented traces, prior 30 ms smoke checks and sustained live acceptance.
- [ ] Exercise the due dispatch path with queued work and the queue disabled/enabled,
  empty work and handed work, including many candidates with slow reservation replies.
  Bound scheduling off the UI thread with single-flight/finite pending work; avoid
  unnecessary collection where eligibility permits. Retain authoritative reservation/
  admission checks before dispatch; a stale display cache must not authorize side effects.
- [ ] Add a real sidebar input/paint regression spanning maintenance with a slow Chief
  endpoint. Prove failure before the fix; include active/inactive Chief view and restart.
- [ ] Reuse the bounded team-display pattern for the traced display path: one active
  collection, one coalesced pending refresh, absolute ≤1 s operation deadline, no socket
  waits on input/render, and explicit stale/error presentation.
- [ ] Reject late generations/runtime changes. Keep action callers' durable submission
  semantics; do not replace every `host_request` globally with display cancellation.
- [ ] Run focused tests and the approved 30-minute sustained probe across at least
  60 maintenance intervals, with latency percentiles and maximum stall recorded.

**Acceptance:** normal input p95 ≤100 ms/p99 ≤250 ms, no >1 s stall; slow daemon does
not block local navigation; old results cannot restore revoked or newer state. Exact
incident attribution and final live/source identity accompany any “freeze fixed” claim.

**Compatibility/rollback:** retain wire fields and valid cached display. A rollback is
an approved prior candidate, not a silent fallback to synchronous reads. If budgets fail,
report unresolved and stop deployment rather than hiding errors.

### S2: Bound observation while retaining one state owner

**Files:** `apps/crates/cockpit-daemon/src/registry.rs`, daemon `teams.rs`/`chief.rs`
only where their current observation calls require it, `apps/osd/src/bin/osd.rs`;
existing registry inline tests and `System` fake. Store work is owned separately by
`153066df` in `apps/crates/cockpit-core/src/chief/store.rs`.

**Interfaces:** workers consume an immutable roster/identity generation and a deadline;
they return bounded observations and errors. Registry remains the owner that validates
and applies results. Workers cannot decide authority, mutate GTK or publish session death
from a timeout. Keep the existing Team/Chief database connection owner.

- [ ] Land one owner-approved observation slice first: pane reconciliation. Add fake
  `System` behavior that stalls while unrelated queue/control work must progress.
- [ ] Bound worker count, subprocess time/output and pending requests. Apply results
  only if their roster/runtime identity still matches; coalesce replaceable observations.
- [ ] Test restart/PID reuse, cancellation, stale results, permission errors and unknown
  versus observed-empty panes. Preserve current lifecycle and resource decisions.
- [ ] Measure callback duration, observation age, CPU/RSS/I/O and pending work at 1× and
  10× fixture roster. Integrate bounded diagnostics with `8e473428`.

**Acceptance:** GLib p99 callback ≤20 ms under agreed load; blocked tmux observation
does not block control; no unbounded threads, output or stale state application.

**Compatibility/rollback:** no database split or new process. Keep messages additive
and old behavior testable through existing entry points; rollback must drain/cancel
workers safely and preserve durable accepted actions. Next observation class is a new unit.

#### S2b: Recurring database work, separately reviewed

After the pane slice, `153066df` owns the equivalent bounded database slice in core
`chief/store.rs` and daemon `chief.rs`/`teams.rs`; allow another 2–4 engineer-days.
Use the same bounded-work/result contract, but retain each complete Team/Chief
transaction with its serialized persistence owner. Moving blocking persistence off
GLib must not scatter individual SQL statements across workers or introduce a second
database. Cancellation of a waiter does not cancel an already committed operation.

- [ ] Test a held SQLite write lock while unrelated non-storage control remains responsive;
  verify the caller sees the real busy/Storage outcome rather than an empty snapshot.
- [ ] Move recurring reconciliation/preparation to the bounded persistence execution
  boundary, with explicit continuation. Revalidate identity/revocation at the authoritative
  decision/commit boundary and before applying results; stale observations grant nothing.
- [ ] Exercise commit failure, restart and late completion. Verify no successful publication
  after rollback, no duplicated transaction, and no SQLite retry sleep on GLib.

Schema and cursor/output optimization remain C1. If keeping transaction/authority
ownership intact requires a wider design than this slice, stop for its review rather
than introducing an unreviewed generic executor. G1 cannot claim database isolation
until this slice's contention proof passes.

### S3: Bound IPC end to end, including responses

**Files:** `apps/osd/src/bin/osd.rs:532–721`,
`apps/crates/cockpit-daemon/src/fifo.rs`, core `teams/client.rs`; tests
`apps/cockpit/tests/team_client.rs`, `team_protocol.rs`, `chief_protocol.rs` and
existing daemon socket fixtures.

**Interfaces:** keep existing `AgentAction`, `HostRequest`, `TeamReply` and peer checks.
Specify request-count/byte limits, per-peer/global budgets, queue admission and absolute
deadline semantics before editing. An overload rejected before acceptance is distinct
from a durable action whose response was lost.

- [ ] Add isolated regressions for slow reader, trickling writer, abandoned connection,
  saturation, fairness and oversized reply; verify a second authenticated client progresses.
- [ ] Replace unbounded completed-request accumulation/drain with bounded admission
  and finite dispatch slices. Account for readers, ready work and responses together.
- [ ] Move bounded response writes outside GLib with a deadline. Preserve request/caller
  identity and durable result; report serialization/write errors through bounded diagnostics.
- [ ] Exercise same-key retry after lost reply, including changed-payload rejection,
  restart and revocation. Cancellation of a display wait must not claim action cancellation.
- [ ] Verify no auth regression and record callback/control latency under the fault matrix.

**Acceptance:** finite queue/thread/byte bounds; explicit overload; unaffected client
control ≤1 s under a stalled peer; zero duplicate durable effects; F4 fixture no longer
blocks the event loop.

**Compatibility/rollback:** additive overload/error reporting with documented old-client
behavior. Unsupported peers receive explicit failure; do not reinterpret silence as
success. Preserve stored action results across process rollback.

**Stop/go G1:** S1/S2/S3 relevant entry tests and sustained/fault measurements pass on
the final candidate. A three-key smoke check cannot substitute for sustained proof.

## Phase 2 — smaller context and reliable useful outcomes

### C1: Entity deltas and bounded snapshot preparation

**Files:** core `chief/store.rs`, relevant existing children under `chief/store/`,
`apps/crates/cockpit-protocol/src/chief.rs`, daemon `chief.rs` where dispatch consumes
context; tests `apps/cockpit/tests/chief_events.rs`, `chief_store.rs`, `chief_recovery.rs`
and core transaction/context tests.

**Interfaces:** existing `availability_changed`, frozen generation, event cursor and
`read_through` remain the starting contract. Proposed deltas carry stable entity identity,
revision, changed fields and tombstone; old consumers use an explicitly supported
snapshot projection. Negotiate/version before changing meaning of an existing field.

- [ ] Record a deterministic replay with 100 workers, one-worker changes, continuous
  churn, open decisions, more than one turn of unread completions and failed deliveries.
- [ ] Add correctness regressions: every nonreplaceable item eventually arrives;
  acknowledged pages survive restart; no skipped sequence/duplicate effect; recovery
  finishes a frozen generation while new changes continue.
- [ ] Emit only changed availability entities for compatible consumers. Coalesce only
  replaceable observations; retain tombstones and durable authority/error/decision history.
- [ ] Bound snapshot construction with an explicit preparation cursor/time budget while
  preserving transactional consistency and all unread completion records. Test busy/disk
  errors and crash boundaries; never “fix” cost by silently capping away pending work.
- [ ] Compare before/after bytes, rows visited, serialization time and useful events per
  wake. Report provider tokens separately if available.

**Acceptance:** at least 50% fewer serialized availability bytes in the specified
one-change fixture; 100% durable event recovery; stable bounds under 10× backlog;
no change in approval authority or loss of error detail.

**Compatibility/rollback:** ship capability detection first; mixed-version tests cover
current/previous consumers. Keep snapshot fallback until evidence supports retirement.
Schema changes require migration approval and backup/restore proof; avoid irreversible
destructive migration during this phase.

### C2: Observe completed work separately from native admission

**Files:** core `chief/store.rs`, Team delivery/store children, daemon `chief.rs`,
`docs/chief-supervision.md`; tests `chief_dispatch.rs`, `chief_recovery.rs`,
`chief_recovery_cli.rs`, `team_lifecycle.rs` and native-provider fixtures.

**Interfaces:** existing submission key/fingerprint, target runtime/epoch, attempt and
delivery records remain canonical. Outcome evidence is an additional dimension,
not a rename of `received` to `completed` or a new grant source.

- [ ] Replay the observed sequence: native receipt succeeds, provider returns HTTP 429,
  assistant output is empty, a later permitted turn answers. Assert the task remains
  unfinished until actual evidence arrives.
- [ ] Add quota-aware bounded retry/expiry classification using the existing policy
  owner. Expose held reason/next eligible time without increasing turns or resource limits.
- [ ] Make stale runtime, unknown completion, failed admission and expired delivery
  distinguishable in the existing supervision projection. Preserve provenance labels.
- [ ] Test disconnect after acceptance, repeated same-key submission, changed-payload
  rejection, restart and revocation. Never replay an already accepted side effect merely
  because its model reply is missing.
- [ ] Run a seven-day admitted pilot with useful events/wake, completed evidence-backed
  tasks/hour, oldest stalled age and tokens/verified result where available.

**Acceptance:** each sampled turn failure has a visible outcome; zero duplicate task
starts; no false completion from `received`; retry loops respect provider/resource holds.
Do not require an invented throughput improvement before the baseline exists.

**Compatibility/rollback:** old delivery state retains its meaning. New outcome fields
are additive; missing provider evidence means unknown. Rollback preserves attempt,
decision and idempotency ledgers.

## Phase 3 — secure agent workflow pilot

### A1: Project-local verification and scoped evidence access

**Files/owners:** begin with `docs/DEVELOPMENT.md`, existing `Makefile` targets,
`tools/quality.py`, `tools/full-test-lock.py`, launcher/environment fixtures and
`config/bin/.local/bin/agent-build`. Broker host code location and operation schema
must be agreed with its actual owner before implementation; no speculative new daemon
or endpoint is mandated by this plan.

**Interfaces:** implement only the approved C1/C2 capabilities from the autonomy appendix.
Inputs are typed operation, project/ref/runtime and bounded selectors; outputs include
source/time/limit/receipt metadata. No arbitrary shell, SQL, URL or filesystem path API.

- [ ] Classify one week's requests by authority, environment, evidence API, configuration
  friction and repetition. Separate human wait from execution and failures from declines.
- [ ] Select one host-free verification path and fix its environment/fixtures within
  approved sandbox grants; validate on the declared Linux shell and isolated HOME.
- [ ] Obtain explicit approval for a narrow read-only evidence pilot: pinned source/CI/
  runtime metadata only. Define caps, caller identity, expiry, redaction and revocation.
- [ ] Add wrong-project, path/symlink substitution, oversized result, stale runtime,
  expired grant and concurrent-budget tests through the broker's real entry point.
- [ ] Compare repeated approvals and successful task progress with the baseline;
  keep every host-authority action on its approved human route.

**Acceptance:** supported host-free checks need no new host execution permission;
approved same-source metadata is reusable; negative scope/revocation tests deny access.
An inaccessible protected path remains a block unless its new capability is explicitly approved.

**Compatibility/rollback:** opt-in project pilot, revocable without breaking existing
`ask-to-run`. Revoke the capability, not the firewall/sandbox. Host-only verification
remains an explicit separate gate.

### A2: Executable discovery, evidence handoff and approval batching

**Files:** I `AGENTS.md`, `.agents/rules/ci.md`,
`.agents/skills/verify-cockpit/SKILL.md`, `.agents/skills/apply-cockpit/SKILL.md`,
`docs/DEVELOPMENT.md`; existing capability/approval owners only after agreement.
Global instructions and native harness command definitions belong to their own approved scope.

**Interfaces:** the compact handoff and C3/C5 contracts in the autonomy appendix.
Discovery describes tools actually executable in the current harness; a plan or skill
listing cannot masquerade as a completed native review.

- [ ] Make current source/capability/receipt status available through the approved
  discovery mechanism. Test unavailable command and stale-source receipt paths.
- [ ] Evaluate the existing two skills against baseline guidance on real recurring
  failure scenarios. Measure loaded bytes, extra reads, incorrect commands and successful
  final-tree verification; preserve safety constraints in both cohorts.
- [ ] Update only guidance with demonstrated benefit. Link canonical policy and commands;
  put long historical execution ledgers behind links rather than mandatory loading.
- [ ] Separately, with `ad70761f` and Theo's approval, bind a batch to effective command,
  cwd and script/diff identities; test mutation after approval and prerequisite failure.
  Coordinate waiting/continuation behavior with `53e8ad63` rather than duplicating it.

**Acceptance:** no false native-command execution or stale-test claim; routine handoffs
fit the compact contract without losing holds/errors; no batch runs a changed/unapproved
action or a dependent after prerequisite failure. Token/context savings are measured.

**Compatibility/rollback:** guidance links retain existing policy; disable a pilot
discovery/batching feature without erasing receipts or changing ordinary approval behavior.

**Stop/go G2:** new capabilities pass negative security/resource tests and remain
revocable. **G3:** seven-day coordination/workflow evidence shows useful progress without
weaker authority, missing errors, duplicate actions or increased stalled age.

## Phase 4 — release proof, then reassess structure

### R1: Native package and actual-runtime acceptance

**Files:** `packaging/arch/PKGBUILD`, `package.py`, `runtime-files.txt`,
`integration-files.txt`, `preview_policy.py`, runtime assets/notices,
`docs/packaging/arch-package-staging.md`; tests `tools/tests/test_arch_package.py`,
`test_arch_runtime.py`, `apps/cockpit/tests/approval_terminal.rs`, launcher/remote tests.
Any release workflow or activation-script change is separately reviewed host-executed code.

**Interfaces:** existing `release.json` version/source/target/components/files declaration,
payload digest and package identity. Add build provenance evidence without interpreting
the declaration as proof. Keep learned grants, credentials, user data and enabled units
outside the package payload.

- [ ] Resolve required/optional runtime classification, including fish required by
  `approval::terminal::run`. Verify missing optional tools produce honest capability errors.
- [ ] Produce the approved exact-source native artifact with locked inputs, dependency
  notices, component pins, compiler/target identity and successful required-check receipts.
- [ ] In an isolated Arch environment, install/start/upgrade/uninstall with no checkout,
  compiler or implicit downloader. Exercise approval, sidebar, daemon, phone and provider
  launcher paths using controlled fixtures where host services are not authorized.
- [ ] Test policy preview against actual approved policy inputs; inspect proposed diff
  without activation. Verify refusal on stale hashes, bad paths, malformed policy and
  source/package mode ambiguity.
- [ ] Only through approved host activation, identify running executables, installed
  hashes/source, service start and post-activation test/latency results. Run sustained
  sidebar acceptance on the actual installed candidate, preserving gaming admission.

**Acceptance:** native lifecycle complete; required runtime commands work; no source
fallback; user state/grants intact; source/build/package/runtime identities reconcile;
required checks and sustained behavior pass. Do not call staging an AUR/release delivery.

**Compatibility/rollback:** retain the previous verified package and matching state backup;
review schema compatibility before upgrade. Revert through the approved package/activation
route, never forced Git history changes or automatic restore over newer user data.
Do not publish an artifact whose checks belong to another revision.

**Stop/go G4:** a stable release and two weeks of bounded operational measurements.
Then compare actual change amplification, affected check time, onboarding time and
cross-platform/headless demand. Approve Option B/C only if a concrete measurable
benefit outweighs migration. Otherwise the five-crate layout is the finished structural choice.

## Verification selection and final evidence

Commands below are existing entry points on the integration source, not commands run
by this research session. Use the current checkout and authorized resource wrapper;
confirm the test selection is nonzero. Full verification runs once on a settled tree,
then only repeats when a relevant input/failure changes.

```sh
python3 -m unittest discover -s tools/tests -p 'test_crate_boundaries.py'
python3 -m unittest discover -s tools/tests -p 'test_module_boundaries.py'
python3 -m unittest discover -s tools/tests -p 'test_arch_package.py'
python3 -m unittest discover -s tools/tests -p 'test_arch_runtime.py'
python3 config/bin/.local/bin/agent-build cargo nextest run --release --locked --manifest-path apps/Cargo.toml -p cockpit --test greywall_launcher
python3 config/bin/.local/bin/agent-build cargo nextest run --release --locked --manifest-path apps/Cargo.toml -p cockpit --test sidebar_refresh
python3 config/bin/.local/bin/agent-build cargo nextest run --release --locked --manifest-path apps/Cargo.toml -p cockpit --test team_client --test team_protocol --test chief_protocol
python3 config/bin/.local/bin/agent-build cargo nextest run --release --locked --manifest-path apps/Cargo.toml -p cockpit --test chief_events --test chief_recovery --test chief_dispatch
make ci
```

Select the commands applicable to the unit; this list is not a request to run all of
them or bypass an unavailable prerequisite. `make ci` retains the OSD prerequisite,
language checks, dependency policy and serialized full-test path. Package, protocol,
workspace and host-policy changes need their complete affected-consumer proof.

For every completed implementation unit, the parent records full task delta (not only
last commit), native `/code-review` and `/simplify` invocations/results, fixes and
final-tree verification. If unavailable, blocked or still running, report that state
and do not claim completion. Retain source owners and user changes. This research-only
roadmap did not execute either native completion command or any implementation checks.

Each evidence handoff distinguishes **implemented → locally verified → remote verified
→ merged → applied → live verified**. A later stage never retroactively invents an
earlier missing check, and a closed todo is not a substitute for the appropriate gate.
