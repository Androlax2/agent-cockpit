# Provider sign-in implementation plan

> Implement inline with executing-plans and test-driven-development; obtain one fresh
> independent review of the complete branch before auto-merge.

**Goal:** Readiness and onboarding show each adapter's current sign-in fact.

**Architecture:** The provider contract owns the fact and its provenance. Adapters run
their documented read-only status commands. The daemon probes on the bulk lane on each
readiness request and maps into readiness-owned facts; readiness retains its config-only
dependency edge. No persistent state or new protocol fields are needed.

**Tech stack:** Rust, the existing standard-library process runners, serde_json, Tokio.

**Spec:** [#443](https://github.com/Androlax2/agent-cockpit/issues/443), reports
[08](../../design/greenfield/08-agent-onboarding-context.md) and
[09](../../design/greenfield/09-project-setup-readiness.md).

## Constraints

- Keep submit/cancel and delivery behavior, installed paths and authorization unchanged.
- No dependencies, credential-file inference, model calls, or live user state in tests.
- Status output never enters reports or logs: it may contain account identifiers or keys.
- Unknown includes a reason and next step; an operational failure is not signed out.
- Conformance stays unverified until measured against the exact harness version.

## Tasks

### 1. Contract and boundaries

- [ ] Extend ownership documentation and crate-graph fixtures for readiness's existing edge.
- [ ] Add `SignIn` with `SignInStatus` and `Provenance`, and `Provider::sign_in` defaulting
  to unknown with a next step. Contract tests prove the safe default.
- [ ] Run `CARGO_BUILD_JOBS=2 cargo nextest run -p cockpit-provider-api -p cockpit-workspace-rules -j 2`.

### 2. Adapter probes

- [ ] Write failing `tests/sign_in.rs` fixtures for Claude JSON plus exit status, Codex
  exit status plus its negative response, and OpenCode's documented credential counts.
- [ ] Implement `sign_in_within(Duration) -> SignIn`, using bounded command output and a
  deadline. Preserve the existing version/bridge runner behavior.
- [ ] Add ignored installed-harness conformance tests in isolated homes, using placeholder
  credentials only. Label unmeasured provenance discovered.
- [ ] Run focused tests for cockpit-claude, cockpit-codex and cockpit-opencode, two at a time.

### 3. Readiness and command-line proof

- [ ] Add a readiness-owned sign-in requirement to `ProviderFound`; map the contract in
  the daemon without importing provider-api into readiness.
- [ ] Keep provider handles for live probes in readiness; run them off async workers.
- [ ] Drive `cockpit doctor` through the real daemon with fake provider answers for all
  states, a subsequent sign-in change and sanitized output. Verify `cockpit init` too.
- [ ] Run the focused readiness, daemon readiness and CLI onboarding tests; update the
  coverage row and a decision record with the measured evidence.
- [ ] Format/lint affected crates, review independently, fix findings, rebase onto main,
  rerun affected checks, push the branch, open the PR and arm auto-merge.

## Review focus

- Unsupported, failing or malformed commands remain unknown, never signed out.
- A sign-in change is visible without a daemon restart.
- Probe output cannot leak credentials or account identifiers in an error.
- A stalled process, large output or inherited output pipe cannot stall readiness.
- OpenCode's listed credentials do not prove model access or token validity; absent
  credentials do not rule out credential-free models.
