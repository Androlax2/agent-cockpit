# Cockpit review — secure autonomy, context and performance contracts

Companion to [the review](audit-2026-10-30.md) and
[evidence register](2026-10-30-cockpit-evidence.md). This document proposes contracts
and acceptance criteria; it grants no capabilities and changes no enforcement.
All budgets below are **proposed initial targets**, not measured current performance.

## 1. Current trust boundaries

| Boundary | Inputs and protected assets | Inspected evidence / limit |
| --- | --- | --- |
| Repository, tool output and collaborator text → agent | Untrusted instructions, proposed commands and task claims | Treat content as data; a Chief answer or ownership release does not grant Theo's authority |
| Agent sandbox → project report/team ingress | Kernel peer identity, registered session/runtime, task/link scope | I `apps/osd/src/bin/osd.rs:672–689`; daemon `teams.rs:553–584` validates sender before ordinary actions |
| Host controls → daemon | Human approvals, grants, session lifecycle | I daemon `teams.rs:893–897`; OSD host handler `565–576` checks the daemon's PID namespace; payload identity is insufficient |
| Daemon → durable state | Team/Chief transactions, dispatch attempts, decisions, cursors | I core `teams/store.rs:90–118`: private regular file check, WAL, foreign keys, FULL sync, 250ms busy timeout; Chief shares connection ownership |
| Human-approved command → host process | Host files, credentials, desktop and services | `approval/terminal.rs:168–197` executes reviewed text through fish/PTY; arbitrary test/build scripts are code execution, not read-only inspection |
| Phone/browser → host control | Pairing secret, request identity, displayed command and approvals | I `apps/cockpit/src/bin/cockpit-remote.rs:124–225`, core `remote.rs`; paired API access and POST header checks exist; no full browser/auth pentest here |
| Sandbox → network/Docker/build resources | Greyproxy hosts, per-project credentials, rootless daemon, gaming/memory admission | Existing launcher/build contracts apply; current network topology and every effective grant were not exhaustively revalidated |
| Source → installed artifact | Reviewed diff, exact build inputs, package contents, live executable | Packaging verifies integrity/declarations; native provenance and activation remain separate release gates |

Unauthenticated phone routes observed before the paired-phone check include GET
`/health`, pairing routes `/pair/<code>` and `/pair-code`, and static application assets.
The static assets contain application code rather than session data. Naming these
surfaces is not a claim of a discovered authentication bypass. Pairing secrets, expiry,
rate limiting, cookies, authorization and origin handling need a dedicated full-path
review before broadening phone control.

### Security finding and review limits

**Medium availability risk (F4):** reachable local team socket → valid request →
synchronous response `write_all` on GLib, without an observed write deadline. Isolated
slow/non-reading-client tests and bounded response transport are the concrete remedy.
Queue saturation compounds availability risk (F3); the reader cap alone is insufficient.

No new Critical/High compromise finding was confirmed in this scoped review. This is
not a clean security bill: static secret-pattern hits inspected were test/redaction
fixtures, no history-wide secret scan or fresh CVE scan ran, and older sandbox findings
were not comprehensively retested. CI's captured dependency-policy output passed at
that source/advisory snapshot; it is not an indefinite assurance.

## 2. Approval inventory: what the evidence supports

The retained sample has 34 results, 10 nonzero, including at least six failed
clone Cargo/test commands. Complete request/decision/wait histories are unavailable.
This review itself needed four host approvals for read-only evidence acquisition.
Do not infer a global approval rate or total saved time from these observations.

| Class | Concrete evidence/example | Correct direction |
| --- | --- | --- |
| Unavoidable host authority | Applying code, installing packages, changing services, credentials, sandbox policy or external access | Keep explicit human consent, exact reviewed inputs and receipts |
| Environment/prerequisite failure | Missing offline Cargo metadata and host-specific fixtures in historical logs; protected checkout inaccessible | Reproducible sandbox prerequisites or approved immutable source export; do not lift filesystem restrictions wholesale |
| Missing supported evidence API | Four approvals to read source/CI/runtime identity for this review | Project-scoped read-only evidence capability with bounded output |
| Credential/configuration friction | Sandbox GitHub `HTTP 401`; tests warning about Git identity and certificate paths | Repair the precise scoped credential/configuration path; preserve trust roots and deny-by-default network policy |
| Repeated approval noise | Repeated verification/source identity requests; no complete frequency series available | Reuse valid same-source receipts; expose discovery/status; measure before batching |
| Real resource or authority hold | Gaming/memory admission, unknown/stale session identity, owner-held files | Keep the hold; show reason, owner and next permitted action |

Future accounting should separate requested, displayed, declined, accepted, executing,
finished and expired, with human wait and execution time separately. An exit code 100,
124 or a nonzero test result is not automatically a human rejection or a security block.

## 3. Proposed capability/approval matrix

“Ordinary” means within existing authorization and sandbox grants, not permission to
edit/build just because the tool exists. **New capability scopes require Theo's approval.**

| Action | Proposed execution surface | Human boundary | Required evidence |
| --- | --- | --- | --- |
| Read project source, diff and local docs | Existing project sandbox | Existing file grants | Checkout/base/head and dirty-file scope |
| Run authorized focused checks | Project sandbox or its rootless Docker | Task authorization plus existing dependency/build/resource rules | Exact command, source identity, environment, selected tests, exit and output |
| Read project CI/issue/check metadata | C1 scoped evidence broker or existing repo-only credential | Approve narrow capability once; no repo-settings/write permission | Repo, ref/SHA, fetched-at, query/result bounds |
| Read approved other-checkout source | C1 immutable tracked-source export | Explicit checkout/project scope; no arbitrary file read | Archive hash, tracked tree identity, exclusions |
| Read own runtime/admission/review status | C3 typed discovery/status | Valid current runtime identity; no authority to change the status | Capability/version, reason, owner, expiry and receipt IDs |
| Message/recruit/dispatch within an existing grant | C4 existing typed Team/Chief API | Existing task, link, native identity, admission and rate limits | Submission key, target runtime/epoch, delivery/outcome state |
| Host apply/install/restart or run modified host scripts | Existing `ask-to-run`; C5 reviewed batching only if approved | Explicit human action bound to final command/content | Diff/content hash, command/cwd, decision, exit and post-activation identity |
| Broaden network, filesystem, Docker, credentials or gaming policy | Existing protected host workflow | Theo-only scope decision | Exact grant change, rationale, audit record, expiry/revocation |

No proposal adds a general shell broker, writable cross-project mount, main Docker
socket, broad token, automatic approval click or model-selected relaxation of guards.

### C1 — Read-only project evidence broker

- **Scope:** approved GitHub read operations, runtime/build identity, bounded sanitized
  diagnostic summaries, and tracked-source snapshots from named approved checkouts.
- **Trust boundary:** sandbox caller → host-owned data and scoped network credential.
- **Abuse case:** path traversal/symlink substitution, requesting another project's
  private state, secret exfiltration through logs, or expensive recursive reads.
- **Mitigations:** fixed typed operations; validate canonical checkout/project against a
  host-owned allowlist; pin source revision; reject links/out-of-root traversal; cap
  bytes/rows/runtime/concurrency; redact secrets; bind caller runtime and grant expiry;
  log query identity/output digest; deny arbitrary SQL, paths, URLs and shell arguments.
- **Residual risk:** permitted source/log excerpts can contain sensitive project data;
  redaction cannot classify every secret. Start with metadata/source, not raw transcripts.
- **Theo decides:** permitted repositories/checkouts, output classes, retention and expiry.
  Proposed pilot success: eliminate repeated approvals for identical pinned metadata
  while cross-project and expired-grant tests remain denied.

### C2 — Reproducible project-local verification

- **Scope:** existing Make/focused checks run in an approved sandbox environment or
  per-project rootless Docker with pinned prerequisites and isolated fixture state.
- **Trust boundary:** repository/build scripts → controlled compute, filesystem and network.
- **Abuse case:** tests execute injected code, mutate shared caches, access other projects
  or starve gaming resources; root inside a container is misrepresented as host authority.
- **Mitigations:** project-only writable mounts, no host Docker socket, egress through
  Greyproxy, approved optional CA injection, locked dependencies, isolated fixture
  HOME/runtime, admission/CPU/memory limits and one shared full-suite lock. Validate
  real GUI/host checks separately; do not silently substitute container proof.
- **Residual risk:** native desktop/PTY/system integration may differ; dependency/cache
  availability can still block. No claim of a fully hermetic build merely from a container.
- **Theo decides:** image/prerequisite installation, mounts and cache ownership. Pilot
  target: all supported host-free checks execute without new host approval; remaining
  host-only checks have a named reason rather than a fallback shell.

### C3 — Typed capability and verification-receipt discovery

- **Scope:** discover commands actually supported by this harness/runtime, source version,
  resource/admission state and existing exact-source receipts. An API/tool name in prose
  must not be advertised as executable if the harness cannot invoke it.
- **Trust boundary:** untrusted agent request → authoritative host/runtime metadata.
- **Abuse case:** replaying old receipts after edits, presenting a sandbox declaration as
  native command execution, or treating an allowed capability as blanket task authorization.
- **Mitigations:** host-owned receipt IDs, source/diff hashes, tool versions, precise scope,
  timestamps, exit/unrun counts and authenticated caller; distinguish supported, unavailable,
  held and revoked. Receipt reuse is invalid when relevant inputs change.
- **Residual risk:** exact input closure for environment-dependent tests is difficult;
  retain uncertainty rather than over-cache. A source-only match is insufficient for live proof.
- **Theo decides:** receipt retention and which check results may be reused. Pilot target:
  zero false “review executed” or stale-source “tests passed” claims in sampled tasks.

### C4 — Outcome-aware coordination within existing grants

- **Scope:** existing links, task dispatch, context reads and quota-aware continuation;
  durable outcomes extend existing admission/delivery semantics.
- **Trust boundary:** collaborator/Chief content → validated task state and native session.
- **Abuse case:** stale PID/runtime replay, duplicate side effects after timeout, invented
  completion evidence, availability churn generating endless paid wakes, or authority
  laundered through a Chief resolution.
- **Mitigations:** exact project/session/runtime/epoch; idempotency key plus payload
  fingerprint; revalidate at execution; explicit uncertain/expired/blocked outcomes;
  same-key retry only where safe; bounded backoff and quota reset awareness; preserve
  `agent_report_not_evidence` and `resolution_grants_authority:false`; existing team-size,
  gaming, turn and resource limits remain enforced.
- **Residual risk:** provider acceptance is not execution, and a provider may not supply
  complete outcome receipts. Report “unknown,” do not fabricate completion or replay work.
- **Theo decides:** any new retry/expiry policy and whether narrow additional dispatch
  capabilities are needed. No raising of budgets is assumed. Pilot target: every sampled
  failed turn has a visible classified outcome and zero duplicate durable effects.

### C5 — Review-bound batching of host actions

- **Scope:** existing owner `ad70761f`'s review windows, not general preapproval. A batch
  contains individually identified actions with declared dependencies and final contents.
- **Trust boundary:** approved plan/diff → host execution with Theo's full rights.
- **Abuse case:** command text stays fixed while invoked files change; prerequisites fail
  but dependents still run; hidden characters or later hook rewrites change the action.
- **Mitigations:** bind approval to final command/cwd and script/diff hashes; display
  effective inputs; reject changes after review; stop dependents on failure; preserve
  per-action results, decline and revocation. Security-policy changes remain separately
  explicit; a checksum itself is not authority.
- **Residual risk:** humans must review real code/content and external effects may be
  irreversible. Batching reduces interruptions, not the significance of the decision.
- **Theo decides:** eligible action categories and review-window policy. Pilot target:
  fewer repeated prompts for one approved change, with every executed action attributable
  to a current human decision and no continuation after a failed prerequisite.

## 4. Compact context and handoff contract

### Preserve the existing machinery

Chief already has frozen snapshot generations, event cursors, durable read progress,
record/page/turn limits, provenance and retry bounds. Improve its projection and work
budget rather than replacing it with a new event store or an LLM-only summary.

Availability changes currently contain the full product roster. A safe reduction is
**per-entity revision deltas plus a recovery snapshot**, not truncating arbitrary text.
Only replaceable availability observations may coalesce. Decisions, approvals,
revocations, completion/error outcomes and ownership transitions remain durable records.

### Proposed record envelope

These are logical fields for a future contract, not claimed existing wire/API names.
Map to existing fields before introducing any new schema:

```text
identity: project, task, session, runtime, epoch
event: stable ID, sequence, entity ID, entity revision, kind, observed_at
provenance: host-derived | protected-human-decision | untrusted-agent-report
delivery: queued | submitting | received | uncertain | failed | expired
outcome: not-observed | running | completed-with-evidence | failed | blocked
change: changed fields or explicit tombstone; bounded display summary
evidence: source/diff identity, receipt IDs, artifact hashes, bounded detail reference
authority: existing grant reference and scope; no authority inferred from text
continuation: generation, read-through, next cursor, remaining-work indication
```

Delivery and outcome are separate dimensions, not a single monotonic enum. “Received”
may coexist with a provider failure. A resolved question may still have an undelivered
answer. “Completed-with-evidence” is a verifier outcome, not an agent-selected label.

### Consumer and recovery rules

1. Durably apply a complete validated page before advancing its read cursor. Retry the
   same page/key after a disconnect; duplicates must not repeat effects.
2. Scope revisions to an entity/runtime generation; never compare opaque revisions as
   global clocks. A stale result cannot restore a revoked identity or overwrite newer state.
3. On a gap, request a frozen recovery generation. Bound preparation as well as page
   output; include every unread nonreplaceable event via continuation, not silent omission.
4. Show stale/unavailable separately from empty/off. Keep actionable errors and decisions
   inline; large diagnostics may be a bounded, authorized detail reference with explicit
   availability/truncation metadata.
5. Wake for a material action, due retry or recovery backlog. Do not wake an LLM merely
   to restate unchanged availability; record why a wake did or did not produce useful work.

### Handoff: one compact entry point, evidence behind it

Proposed routine handoff target: **at most 2 KiB** for the summary, excluding referenced
evidence. This is a usability target, never permission to truncate required error or
authorization details. If it cannot fit safely, emit an explicit continuation.

```text
Task and exact owner/runtime:
Current state and one next action:
Scope: included files/changes; excluded owner-held work:
Source: base/head, task commits, staged/unstaged/untracked delta:
Evidence: command + result + receipt; failed/unrun checks included:
Authority/holds: existing approval scope; unresolved decision owner:
Delivery: merged / applied / live-verified, each with its own evidence:
```

A handoff is incomplete if it only says “done,” cites the last commit while omitting
earlier task commits, or claims native review from a prose/self-review substitute.

### Context success criteria

Use the same recorded workload before/after, with matched roster size and task mix:

| Measure | Baseline | Proposed acceptance |
| --- | --- | --- |
| Serialized bytes / provider input tokens per useful Chief action | Unknown; full-roster emission is observed statically | At least 50% fewer serialized availability bytes for a one-worker change in a 100-worker fixture; report actual tokens separately |
| No-change LLM wake fraction | Unknown | Zero wakes for identical observation revisions in deterministic replay; pilot ratio reported rather than assumed |
| Durable decisions/errors/completions delivered | Existing paths and tests, no fresh full capture | 100% eventually visible in restart/retry/backlog fixtures, no duplicate effects |
| Snapshot progress during churn | Frozen-generation design exists | Finishes the chosen generation under continuous roster updates; then catches up incrementally |
| Verified useful actions / 10,000 input tokens and per wall-clock hour | Unknown | Establish 7-day pilot baseline; improve without increasing incorrect decisions, stalled age or approval violations |
| Guidance efficiency | Root/rule/skill bytes measured; skill advantage unproved | Smaller selected context only if scenario correctness and required-command execution remain intact |

Useful actions include delivered decisions, verified handoffs and resolved blockers;
availability-only reports do not count. Report useful events/wake separately from
verified completed tasks/budget, so a busier Chief cannot manufacture a better score.

## 5. Performance plan

### Measured facts and unknowns

The sidebar's sampled socket-wait occupancy is 23.5% over five minutes, with recurring
6–8 second waits. Native admission in one incident is fast (17–49 ms) while useful
reply delay is about 55.94 minutes. Those are distinct bottlenecks.

Already-collected trace evidence adds two complete cycles of 111 sequential host
connections and input-poll gaps of 7.825 s and 6.854 s. The investigator attributes
the groups to one snapshot plus 110 reservation attempts; exact source-line mapping
remains attributed. The audit implication is to bound accumulated scheduling IPC as
well as display collection. Detailed diagnosis, repair and live acceptance remain with
the dedicated investigator; source access and a matching binary hash are not freeze acceptance.

Current daemon CPU/RSS/I/O, startup, Team database size/growth, query plans, context
token totals and clean/warm build costs are unknown. Existing resource budgets and
admission rules continue to apply. Do not invent a cache hit-rate or speedup estimate.

### Initial acceptance budgets

The owner should calibrate these once against an agreed machine/workload, record the
choice, and then hold the same workload fixed for comparison. Tightening is welcome;
relaxation requires an explicit trade-off, not a hidden timeout increase.

| Path | Proposed budget | Verification |
| --- | --- | --- |
| Sidebar input-to-paint | Normal p95 ≤100 ms, p99 ≤250 ms; no stall >1 s | 30-minute input/paint probe over at least 60 maintenance periods, plus injected slow peers |
| Input while daemon is slow/unavailable | Visible input/local navigation ≤250 ms; stale/error state, no synchronous network wait | Delay, trickle, disconnect, malformed page, generation change and restart fixtures |
| Display fetch | One absolute deadline ≤1 s across connect/write/read/pages/retries; at most one active fetch plus one coalesced pending refresh | Fake socket with partial reads and late results; drop stale generation without clearing valid cached state |
| GLib callback work | Normal p99 ≤20 ms; no blocking subprocess/socket/DB retry wait | Bounded callback spans plus fake slow System/store/peer; loop heartbeat continues |
| Lightweight control IPC | Normal p95 ≤100 ms, p99 ≤250 ms; stalled peer does not delay other clients past 1 s | Concurrent status/control traffic with slow readers and queue saturation |
| Background reconciliation | Explicit item/time slice and continuation; no full unbounded pass on the control loop | 1× and 10× fixtures; record rows visited, duration and pending count |
| Idle daemon resources | Initial target ≤5% of one core over 10 idle minutes; RSS stable, no unexplained monotonic growth | Same roster, pinned binaries, process/cgroup CPU/RSS/I/O with diagnostics overhead reported |
| Loaded resources | Stay within existing gaming/memory/admission budget; no new unbounded thread/queue growth | Controlled admitted load; report rejections and queue age rather than overriding the guard |
| Startup | Local UI becomes interactive within 1 s without waiting for remote state; daemon readiness target ≤2 s after admitted process start | Cold/warm fixtures, unavailable peers; admission wait reported separately |
| Build/check cost | Establish cold/warm/source-only baselines; ordinary inner loop uses the smallest meaningful check | Toolchain/source/cache/job count/time/peak RSS/target bytes recorded; no unapproved builds for this report |

Queue capacities need explicit values before implementation. Suggested initial isolated
test profile: global pending request cap 256, per-peer cap 16, finite response-byte cap
derived from the existing wire limit, and dispatch slices capped at 32 requests **or**
10 ms, whichever arrives first. These are calibration candidates, not permission to
raise existing limits. A cap must cover readers + completed work + queued writers,
not just one stage. Saturation returns a visible retryable error before acceptance;
accepted durable actions cannot be silently dropped.

### Workload and fault matrix

Run in fixtures first. Live sampling requires its own approved host step; no production
load generator or game-resource override is authorized by this plan.

| Scenario | Required outcome |
| --- | --- |
| Normal roster, sidebar open 30 minutes | Input latency budgets hold through maintenance ticks; no recurrence dismissed as “not reproduced in three keys” |
| Queued work present; queue disabled/enabled; slow snapshot and reservation replies | Due scheduling cannot stall input even across many candidates; disabled queue produces no dispatch; enabled actions retain authoritative identity/admission checks |
| Slow/trickling/abandoned socket; tiny receive buffer | Whole-operation deadline holds; unrelated control completes; response memory is bounded |
| Queue flood from one peer alongside legitimate traffic | Per-peer/global limits hold; fairness prevents starvation; reject before side effects; no authorization shortcut |
| tmux/proc observation stalls | Worker deadline/resource cap holds; UI/control loop continues; stale observation cannot kill/reassign a session |
| SQLite busy, disk-full, malformed row, failed commit | Explicit `Storage` error; rollback and durable cursor consistency; no successful publication from failed persistence |
| Daemon/client crash between persist/send/receipt | Same-key retry is idempotent; unknown delivery is reconciled; no duplicate task start or authority grant |
| Runtime restart/PID reuse/grant revocation during fetch or approval | Old result/proof rejected; revalidation uses current session/runtime/epoch |
| Large unread backlog and continuous availability churn | Recovery converges in bounded pages; decisions/completions are not starved or truncated |
| HTTP 429/provider outage | No immediate duplicate wake loop; retry reason/time visible; admission remains distinct from completion |
| Concurrent admitted builds/tests and phone polling | Shared full-suite lock and admission preserved; no startup shared-path race or unbounded polling work |
| Package fresh install/upgrade/uninstall without source/compiler | Required approval/runtime commands work; user state/grants survive; no automatic policy/service activation |

### Data growth and I/O

Measure Cockpit and provider stores separately. For each owned store record main/WAL
bytes, row counts by record kind, oldest unread item, retention policy, busy duration,
rows scanned and query plan on an approved snapshot. Avoid diagnostic whole-database
copies; the earlier 3.77 GB provider-store refusal is a model to preserve.

Use keyset/sequence continuation and indexes justified by actual `EXPLAIN QUERY PLAN`
on fixtures. Do not split Team/Chief databases, relax synchronous durability or purge
unread decisions to improve a benchmark. Retention/compaction must preserve unresolved
delivery, approval audit, recovery cursors and referenced evidence; age alone is insufficient.

Phone conversation cache work already belongs to `614fa5a0`. Measure unchanged-source
bytes read and invalidation behavior before accepting a cache. Cross-runtime identity,
revocation, errors and explicit empty state remain part of the cache key/contract.

### Diagnostics and resource accounting

Use the existing diagnostics owner `8e473428`. Record monotonic spans and correlation
IDs for queue wait, handler, observation, DB, serialization, native admission and outcome.
Bound cardinality and retention; never log credentials or whole transcripts by default.
Account for diagnostics overhead with an on/off comparison. Histograms need count,
window and workload labels, not p99 alone.

Build/test measurement retains `agent-build`, the shared full-suite lock, SCHED_IDLE/
existing scheduling controls and gaming/memory protections. Separate admission wait,
compile time and test execution. A cold build, a warmed dependency graph and a changed
one-file rebuild are different measurements. Shared writable target/cache directories
need an explicit trust/concurrency design; do not add them as an unmeasured speed fix.

## 6. Theo-only decisions

1. Approve Option A and the acceptance workload/budgets; keep existing owners.
2. Approve each new capability scope, especially evidence access and receipt retention.
3. Decide required-check/release policy with the workflow owner after configuration evidence.
4. Approve native package lifecycle tests and any host activation separately.
5. Approve instruction-policy reductions or new retry/expiry rules only after pilot evidence.

Routine module/API details belong to the scoped implementer. Changes to authority,
credentials, network access, resource protection and deployment never follow merely
from this report's recommendation.
