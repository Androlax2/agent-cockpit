# Sign-in is a current adapter fact with independent provenance

Issue #443. Decided by the worker, following reports 08 and 09 and the harness commands.

- **Probe on each readiness request, in the bulk lane.** Sign-in can change after the
  daemon starts. The daemon retains adapter handles and probes off async workers; readiness
  keeps its config-only dependency edge. No database or protocol change is needed.
- **Ask the harness, never infer credential-file layouts.** Claude's `auth status` JSON
  and exit status, Codex's `login status` response, and OpenCode's `auth list` credential
  counts distinguish signed in, signed out and unknown. Operational failures remain
  unknown. These commands report configured credentials, not their validity or model access.
- **Bound and discard raw output.** Every probe has a ten-second deadline and 64 KiB per
  stream. Nonblocking socket pairs avoid blocked reader threads when wrappers leave their
  output open. Only normalized status and fixed next actions reach readiness; account
  names, paths and masked keys do not. Each probe leads its own process group, stopped
  before reaping its leader so a reused PID cannot be signalled. This reuses the workspace's
  pinned `rustix` 1.1.6 process API (already used by Codex and the process supervisor).
  Each adapter owns its runner, preserving the existing contract-only dependency rule and
  leaving discovery and delivery behavior unchanged.
- **Provenance belongs to the probe.** Isolated-home conformance passed on Claude 2.1.316
  and Codex 0.166.0, using only placeholder API keys. Codex's other capabilities retain
  their 0.161.0 pin. OpenCode's source-backed count parser remains discovered until the
  ignored conformance test runs on 1.19.5; the installed v2.1.4 is not that harness.
- **Rejected:** caching the status at startup (stale after login), treating any nonzero exit
  as signed out (also means configuration failure), reading credential files, or reporting
  raw command errors (may disclose credentials). OpenCode's credential-free models mean
  signed out is not a session-launch refusal; existing readiness blocking semantics stay.

Evidence: adapter `tests/sign_in.rs` and `tests/sign_in_conformance.rs`,
[readiness assessment](../../crates/cockpit-readiness/tests/report.rs), and
[doctor and onboarding](../../crates/cockpit-daemon/tests/readiness.rs).

Sources: [Claude CLI](https://code.claude.com/docs/en/cli-reference),
[Codex CLI](https://learn.chatgpt.com/docs/developer-commands),
[Codex 0.166.0 login](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/cli/src/login.rs),
[OpenCode 1.19.5 providers](https://github.com/anomalyco/opencode/blob/v1.19.5/packages/opencode/src/cli/cmd/providers.ts).
