# Codex delivered-turn approval routing implementation plan

> **For agentic workers:** Use the executing-plans skill; implement inline, then obtain one fresh independent review of the full branch delta.

**Goal:** Observe where delivered-turn approvals go, and prove the owner's TUI can refuse them without Cockpit granting authority.

**Architecture:** `cockpit-codex` continues to own Codex transport and conformance.
Test-only recording proxies observe the real TUI and delivery connections. A local
scripted model and isolated homes exercise the installed harness without account usage.
No crate dependencies, manifests, sandbox lifecycle or production approval behavior change
until measurements justify a separate decision.

**Tech Stack:** Existing Rust standard library, serde_json, tempfile, installed Codex and tmux.

**Spec:** [Issue #436](https://github.com/Androlax2/agent-cockpit/issues/436), its acceptance criteria, and Theo's provider-validation lane brief.

## Constraints and review focus

- Installed Codex is 0.166.0; delivery's existing conformance pin remains 0.161.0.
  A version-specific observation cannot promote unrelated capabilities.
- No real credentials in fixtures. Signed-in runs require a concrete `ask-to-run` approval.
- Approvals must be observed on the owner's real TUI connection, including after the
  delivery connection closes. The harness only declines; it never accepts commands.
- Record start versus steer, unrelated threads, and receipt of the matching refusal.
- Missing harness, wrong version, no approval, timeout, or missing owner response fail
  explicitly; synthetic transport tests cannot claim live conformance.
- Bounded subprocesses, socket reads and teardown; fixtures use their own tmux server.

## Task 1: Isolated approval-routing harness

**Files:** `crates/cockpit-codex/tests/approval_routing.rs` and test-only helpers under
`crates/cockpit-codex/tests/common/`; existing `Provider::deliver` is the entry point.

- [x] Add recording-proxy tests proving request IDs survive framing and separate client
  roles, with bounded reads and no synthetic approval responses from Cockpit.
- [x] Run those tests before implementing the helpers; verify the expected failure.
- [x] Implement the minimum proxy and local fake-model support.
- [x] Add ignored exact-version conformance for a delivered start and steer, the real
  TUI's refusal, and a closed delivery connection; run with isolated homes only.
- [x] Record observed results and any remaining signed-in validation gap.

## Task 2: Reviewable evidence and live-run request

**Files:** adapter README, its fixture provenance if captured, report 16's provider row,
and `docs/decisions/2026-11-02-436-approval-routing-evidence.md`.

- [x] Document measurements separately from unmeasured acceptance; use `Part of #436`
  unless the signed-in criterion is also proven.
- [ ] Prepare one bounded signed-in run with usage and host effects stated; request host
  approval only after the command and isolated harness are reviewable.
- [x] Run final affected tests, formatting and lint checks with modest parallelism.
- [ ] Obtain one native full-delta review, or explicitly report its absence and use the
  authorized fresh-agent review. Commit, normal push, open the PR, then arm squash
  auto-merge after review and focused checks. Never rewrite pushed history.

## Execution record

- Baseline: delivery and turns integration targets, 20 passed.
- Versions: standalone Codex 0.166.0; OpenCode 2.1.4; Rust/Cargo 1.99.1;
  nextest 0.9.150. Wrapper version discovery failed on host-owned directory creation;
  standalone discovery used fresh temporary homes.

- Isolated 0.166.0 proof: delivered start and steer both reach the real TUI after
  delivery disconnects; TUI cancellation resolves the request and interrupts the turn.
- Fixture corrections: exclude the TUI's title helper, accommodate a new model response
  after steer, and respect the one-second protected-screen input quarantine. Initial
  ignored runs failed on those fixture defects; they were not live provider failures.
- Added a credential-free preflight regression: an enabled sentinel MCP server is refused
  before any thread/model call and the sentinel executable does not run.
- Signed-in execution is still pending explicit host approval. Native `/code-review` is
  unavailable in this session; use the brief's authorized fresh independent reviewer.
- The first independent full-delta review found inherited notification commands and
  execution rules, insufficient signed-in provenance, a steer disconnect ambiguity, and
  unbounded helper commands. The fixes add native credential-free regressions and bounded
  subprocess capture. A second fresh review covers these substantial safety changes.
- Preflight development caught the exact version's default ChatGPT endpoint in its
  effective config; the official endpoint is allowed, while custom routing is refused.
- The second full-delta review found that project settings require `cwd` on `config/read`
  and that Escape can be rebound to approval. Follow-through adds both guards, rejects
  nonempty disabled project layers, and checks the same setup in isolated mode. Native
  regressions cover CLI versus persisted fixture trust and a remapped Escape.
- Verification: 125 regular crate tests passed (19 skipped); nine exact-0.166.0 isolated
  native tests passed (seven skipped, including the signed-in test). Formatting, Clippy
  with warnings denied, and `make docs-ci` passed. The decision-record target passed its
  three tests. The full workspace and supported-platform gates remain CI's responsibility.
