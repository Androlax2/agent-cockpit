# #433 live sandbox enforcement implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Measure denied paths and destinations plus allowed controls across four real daemon launches on Linux and WSL2 for Alpha 1, and retain bounded owner-reported observations with the exact launch. macOS acceptance is deferred under #568.

**Architecture:** `cockpit-sandbox` owns observation validation/storage in the existing launch transaction domain. `cockpit-sessions` serializes recording with lifecycle operations; the daemon's existing owner-only boundary admits the request. A standard-library Python CI harness uses verified published engines and real CLI/daemon/service entry points in an isolated home; local checks use fixtures only.

**Tech Stack:** Existing Rust/Turso/serde/tokio dependencies, Python standard library, existing systemd/launchd/curl/tmux tools; no new dependencies or manifest edits.

**Spec:** Issue #433 and `docs/design/greenfield/11-security-isolation-platforms.md`; the lane brief supersedes the stale source-build route with `engine-pins.json` and the managed installer. #495 is the prerequisite implementation in PR #528.

## Global constraints

- No probe of the user's services, credentials, data, or legacy installation. Live engine enforcement runs only in explicitly isolated CI.
- No grants, provider profile domains, client scopes, private-state denials or policy expansion.
- No workflow push, installation, tags/releases or host setting changes without separate host approval.
- Existing per-user ownership and launch identity remain the authority. Owner-provided observations are labelled as such; engine capability reports are not represented as enforcement proof.
- Corrected published Greyproxy after fork PR #8 remains an Alpha 1 prerequisite. macOS
  support and the public-CA fix #527 are deferred under #568, not Alpha 1 blockers.
  Unsupported capabilities still fail closed with a typed reason; ordinary required
  development CI remains intact, and Linux/WSL2 live acceptance remains unmeasured.
- Fixed managed ports imply one profile per host network namespace. Four launches are sequential, with no stress loop.
- #397 owns its CI optimization; release lane owns release.yml, tools/release.py and RELEASING. Coordinate any ci.yml delta before its protected push.

## Review focus

- A stale or replayed report must not land on a replacement launch; an exact duplicate is idempotent and a conflicting second report refuses.
- Agent-socket callers and invalid/oversized/control-character observations refuse before storage.
- A denied network probe must show an exact launch-bound proxy block decision alongside a successful allowed control, never treat timeout/DNS/TLS failure as successful enforcement.
- Fixture-home and managed-service cleanup must target only the run's owned resources, including early setup failures.
- Missing real engine capability must stay visibly blocked; fake-provider fixtures and hosted-platform tests do not establish a live platform pass.

## Task 1: Launch-bound owner observations

Files: cockpit-sandbox evidence/observation module and real-database tests; cockpit-sessions lifecycle method; protocol request; daemon owner route; existing plain-text CLI session command and real-daemon tests. No crate/dependency edge change.

- [x] Add a real CLI/daemon test that records observations for the current sandbox client, displays them through `session sandbox`, and retains them after restart of the daemon.
- [x] Observe the command's expected initial refusal before adding its implementation.
- [x] Implement `SessionSandboxRecord { session, client_identity, observations }` and `session sandbox-record SESSION --client CLIENT --observed TEXT...`. Return the existing `SessionSandbox` view. Bound to 16 unique nonempty single-line observations of at most 512 UTF-8 bytes, client identity at most 256 bytes; preserve engine observations.
- [x] Validate and append only once, in the caller's existing write transaction, prefixed `owner probe:` plus a space. Same batch is an idempotent success; changed batch or stale/wrong/unsandboxed launch refuses. The latest launch's client identity is the binding; no timestamp or caller-supplied session grants authority.
- [x] Entry-point tests cover unauthorized agent, bounds/malformed input, identical duplicate, conflicting duplicate and stale identity after session restart; retain existing read-only views and wire compatibility.
- [x] Focused nextest with two threads and affected fmt/Clippy. Commit follows final review/checks on the fresh #433 branch.

## Task 2: Bounded live runner and fixture checks

Files: tools/verify-sandbox-enforcement.py, tools/tests/test_verify_sandbox_enforcement.py and a dedicated Make target. Keep outside the default local full suite: live execution requires a disposable CI environment and an explicit flag.

- [x] Write fixture tests for result validation, exact four-launch accounting, denial versus infrastructure failure, clean environment, selected profile and early-failure cleanup. Watch the missing runner behavior fail, then implement it.
- [x] Use temporary homes outside /tmp for engine path enforcement, a short separate private runtime directory, fake provider commands, a fixture Git checkout and fixture ~/.ssh/id_* files. Mask every supported provider on PATH; never retain credentials.
- [x] `cockpit --profile enforcement sandbox install` consumes the merged manifest and published assets. Start the real daemon. Refuse a missing manager/capability instead of using a fake in live mode.
- [x] The probe agent reads/writes an allowed checkout file, cannot read fixture private keys or the daemon database, reaches existing-profile `https://api.github.com/`, and cannot reach `https://example.com/`, with the owned proxy recording its exact launch-bound block decision. Bounded curl commands do not follow redirects or print headers/credentials.
- [x] Run one initial session launch and three sequential restarts. Bind each measured result to its daemon-reported client identity, then record and read it via CLI. Save redacted JSON and `session sandbox` text artifacts; no success on unmeasured or partial runs.
- [x] Stop only the owned session/proxy/daemon/private tmux server, including failure paths. Never manipulate a foreign listener or trust store.
- [x] Run only the fixture self-tests locally, plus compile/static checks; record live checks as not run until approved CI.

## Task 3: Reviewed CI proposal and acceptance evidence

Files: proposed ci.yml steps coordinated with #397; Make targets; coverage map and dated #433 decision. No release workflow changes.

- [x] Initially prepare Linux/macOS/WSL2 workflow steps with bounded preflights and artifacts.
  Superseded for source delivery: retain fixtures only in ordinary CI; separately approved
  Alpha 1 live execution targets Linux+WSL2. macOS acceptance is deferred under #568;
  preserve its existing typed-capability-refusal fixture and required development checks.
- [x] Keep the workflow unpushed until a real host ask-to-run approval of the reviewed commit/push. Document any CI package/service prerequisites in that command's effects.
- [x] Run actual Codex `$code-review medium`, address findings, and one fresh independent full-delta review (second only after substantial fixes).
- [x] Required affected final checks and one shared-lock full workspace suite. Update coverage with only evidence actually established.
- [ ] Source PR uses `Part of #433`; source delivery finishes after verified merge, while
  live acceptance remains open until measured. Report exact release/approval blockers
  and current PR through cockpit-session-doing. Draft skips are not tested-code evidence.

## Execution ledger

- Observation entrypoint RED: unknown CLI verb. Implementation GREEN: 29 focused daemon/CLI tests passed, including persistence, stale/duplicate refusal, agent-socket refusal and bounds.
- Runner RED: missing command/modules (one assertion failure, five missing-file errors). Initial six fixture checks then passed.
- Ruling: Linux TUN can obscure the SOCKS refusal as a transport error. Add bounded read-only queries of the fixture-owned proxy's existing authenticated logs; require exact client, destination, port and outcome plus a successful allowed control. No policy change. A timeout alone is not evidence. New log-validation tests RED then GREEN.
- Runner wiring/cleanup RED: private tmux socket remained. Cleanup through its exact private socket fixed it; nine fixture tests passed. No live services or network probes run.
- Native `$code-review medium` completed on base `d1f7b0a` plus full uncommitted delta (SHA256 `4e7479643bc893838876836f6da4c39354f02d4a1951bb25021a3aca5bf87ead`). Five findings: bound output while commands run, preserve cleanup on a second timeout, save the qualifying rule decision, validate malformed decision fields, and put the focused platform check in Make. Runtime regressions were reproduced before fixes; 13 fixture tests passed afterward. No independent-review claim is made for that inline workflow.
- Fresh independent full-delta review of snapshot `b2b992ddc46b976c7aad14eb88a22ae012d7518b969788009d17afc560db761d` found two runner integration issues: systemd preferred the empty private runtime endpoint, and the manager bus became a desktop delegation. Explicit session-bus routing plus the disposable owner's existing desktop-withheld setting fixed both. Regressions were RED then GREEN (14 passed). The same reviewer approved narrow follow-up `8e05f776fd1fedd4a742edff396dabc1b71086ac2725d5c445a70ef671ba1326`; no second full-delta review was claimed.
- One shared-lock final workspace run passed 2,250 tests, with 34 skipped (224.208 seconds). Affected Clippy, Rustdoc, no-persistence build, formatting, Python fixtures, CI bootstrap/job-selection fixtures, Markdown, offline links and spelling passed. The new plan's initial Markdown trailing-space error was corrected. No live enforcement run or complete Actions syntax check was performed; actionlint is unavailable.
- The original protected workflow request was terminally abandoned without approval or
  execution. A fresh exact publication request is required; corrected engine releases,
  pins and live enforcement remain separate approvals.
- Source-delivery follow-through preserves main's draft/ready guards from `63a8194`.
  Ordinary CI runs fixture checks only: the original automatic live steps and their
  engine-specific package additions were removed before publication because the current
  pins are not acceptance-ready. `make sandbox-enforcement-ci` remains available for
  separately approved disposable CI execution; no dispatch is added or invoked here.
  Source may merge after real ready-event checks pass without claiming live acceptance.
