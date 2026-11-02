# Native instruction validation implementation plan

> **For agentic workers:** Use the executing-plans skill for inline implementation.

**Goal:** Observe the real daemon's unsolicited launch brief and supported native
skill advertisement in requests made by the installed harnesses to isolated local
model fixtures. Keep signed-in model validation open until separately approved and
measured.

**Architecture:** Reuse the daemon integration tests' temporary home, owner socket,
real agent channel and private tmux server. Replace only the fixture provider
executables with wrappers around explicitly selected native binaries. A bounded
Python standard-library HTTP fixture records model input and returns canned text;
it never requests tools or grants approval. Existing fake sandbox engines remain
the test boundary, so this does not claim real sandbox or signed-in conformance.

**Tech stack:** Existing Rust integration-test dependencies, Python standard
library, installed native harnesses and real tmux.

**Spec:** [Issue #419](https://github.com/Androlax2/agent-cockpit/issues/419), its
current comments, and
[the native skill decision](../../decisions/2026-11-01-419-native-skill-per-harness.md).

## Constraints

- Claude Code target/installed 2.1.316; Codex target 0.161.0 versus installed
  0.166.0; OpenCode target 1.19.5 versus installed 2.1.4. Use the separately approved
  extracted OpenCode 1.19.5 for evidence. Do not change production pins.
- No account access, credential copies, installations, new dependencies or
  production model changes. All model endpoints and keys are fixture values.
- Preserve merged #469/#481 delivery and skill wiring and #489/#490/#491 delivery,
  mapped-socket and sign-in work. Total context belongs to #479.
- Claude's native print mode may prove input delivery but does not prove TUI parity.
  Codex's native Cockpit skill remains unsupported.
- Do not change shared sandbox lifecycle or the existing Home fixture. Use the
  existing managed-engine stand-ins and disclose their boundary.
- Use focused checks at two jobs; supported-platform proof belongs to CI.

## Review focus

- A brief appearing only in a tool description or user prompt must not count as
  unsolicited delivery: inspect the provider's actual context-bearing fields.
- System policy, parent skill directories and credential helpers must not enter a
  fresh-home run: refuse unmanaged configuration before launching native code.
- A stalled fixture must stop its processes and restore temporary permissions.
- Helper/title requests must not substitute for the user's conversation request.
- Version drift, an absent skill or a live-validation gap must remain visible in
  receipts and coverage instead of becoming a green claim.

## Task 1: Native daemon entry-point probes

Files: create `crates/cockpit-daemon/tests/native_instructions.rs` and focused
helpers under `crates/cockpit-daemon/tests/instruction_fixture/`.

- [x] Inspect current launch wiring, exact-version harness behavior and existing
  daemon fixtures. Baseline: two `agent_onboarding` tests pass.
- [x] Add ignored native tests using `SessionNew` through the real owner socket.
  First observe their missing evidence, then complete the smallest fixture needed
  to obtain a request with the daemon-generated session ID and brief.
- [x] Capture Claude/OpenCode native skill advertisement, and Cockpit MCP tool
  discovery where supported, without directing the prompt to find instructions.
- [x] Enforce fresh homes, explicit program/version selection, bounded captures
  and cleanup. Exercise meaningful refusal/cleanup paths with fake processes.
- [x] Record exact successful commands and failures. Keep live acceptance open.

## Task 2: Evidence, review and integration

Files: a dated #419 decision, native fixture usage documentation and the relevant
row of `docs/design/greenfield/16-feature-coverage-map.md`.

- [x] Update evidence with exact versions, provenance and remaining limits.
- [x] Run the affected native/focused tests and final lint/format/docs checks once.
- [x] Invoke the actual Codex `$code-review medium` workflow over the full delta;
  record revision, findings and outcome. Retain a separate fresh independent
  full-delta review as required by the lane.
- [ ] Resolve findings, open one PR with `Part of #419`, and enable squash
  auto-merge only after review and checks. Continue #51 from updated main after
  merge. Any signed-in run requires its own concrete host approval first.

## Execution evidence

The prerequisite build was
`CARGO_BUILD_JOBS=2 cargo build --locked -p cockpit-cli -p cockpit-daemon -p cockpit-claude -p cockpit-codex -p cockpit-opencode -j 2`
(exit 0). It fetched only already-locked dependencies from merged main; no manifest
or lockfile changed. Subsequent checks used `--offline`.

`CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=2 cargo nextest run --locked --offline -p cockpit-daemon --test native_instructions --test agent_onboarding -j 2`
passed five tests (the ordinary fixture entry runs nine Python regressions), with
three native tests explicitly skipped.

The final native run used:

```sh
CARGO_BUILD_JOBS=2 \
COCKPIT_CLAUDE_PROGRAM=/home/theo/.local/share/claude/versions/2.1.316 \
COCKPIT_CODEX_PROGRAM=/home/theo/.local/npm/bin/codex \
COCKPIT_OPENCODE_PROGRAM=$PWD/.outgoing/opencode-1.19.5-7mi5obxc/opencode \
COCKPIT_INSTRUCTION_CAPTURE_DIR=$PWD/.outgoing/instruction-419-isolated-git \
cargo nextest run --locked --offline -p cockpit-daemon --test native_instructions \
  -j 2 --run-ignored only --no-capture
```

Exit 0: all three native probes passed in 7.643 seconds; three ordinary tests were
excluded. Native requests used the real daemon-generated session, workspace and
manifest. No signed-in run occurred. Receipts are committed beside the fixture.

Failures retained: the initial incomplete fixture failed as expected; a wrong
`manifest_id` call failed compilation and was corrected to `manifest().id`;
OpenCode's deny-all fixture permissions hid the MCP schemas until the fixture
allowed Cockpit discovery; Claude context extraction initially read the wrong role
and was corrected from the captured 2.1.316 request, including deferred MCP tools.
Codex cleanup initially left a stale wrapper marker; cooperative stop and
signal-protected cleanup removed the race before final validation. The regression
for a skill description without native registration failed first, then passed
after requiring the skill name and native tool. Ruff formatting initially failed
and was applied. These are fixture iteration failures, not hidden production
failures or signed-in evidence.

## Required review record

Invoked the actual Codex `$code-review medium` skill at
`/home/theo/.codex/skills/code-review/SKILL.md` over the complete delta from
`e05bf5d37a5a8df4a171a366604f790d7b138056`, including the staged working tree
`8e3eb4c41c63f1c34f0f5d5f07ca46e59d7e419e`. All eight prescribed inline angles ran:
line-by-line, removed behavior, cross-file, reuse, simplification, efficiency,
altitude and conventions. No subagents or verifier pass were used in that workflow.
The separate `/simplify` workflow was not invoked.

Findings and resolution:

1. Tool descriptions could count as MCP/skill registration. Parse actual tool
   names, including the namespace captured from Codex 0.166.0. The new negative
   regression failed before this fix. The first stricter native run exposed the
   actual namespace shape; it failed visibly, then passed with that shape handled.
2. A valid main capture could hide an auxiliary fixture failure. Check error
   evidence before consuming a capture and after native teardown; a Rust regression
   proves simultaneous success/error records fail.
3. Cleanup ignored filesystem errors. Propagate stop/marker I/O failures; a Rust
   regression proves neither a failed stop write nor an invalid marker directory
   counts as cleanup success.
4. A readiness reader could see an empty URL. Publish the URL by atomic rename.

All four are resolved. Focused tests pass (five total, including nine Python
regressions); the refreshed native run passes all three providers and replaces the
old receipts with the final fixture hash. A missing borrow during the Rust helper
extraction failed compilation and was corrected before these runs.

Final static checks: `cargo fmt --all -- --check`;
`CARGO_BUILD_JOBS=2 cargo clippy --locked --offline -p cockpit-daemon --test native_instructions -j 2 -- -D warnings`;
Ruff check/format on `tests/instruction_fixture`; Markdownlint, offline Lychee and
Typos on the changed code/docs. All pass. No local full suite or live/account test
was run. Supported-platform CI and a fresh independent full-delta review are the
remaining integration gates; neither is represented as this skill invocation.

## Independent review and follow-through

The fresh independent reviewer read the full
`e05bf5d37a5a8df4a171a366604f790d7b138056..a68eec37c14dc3cf73bc2b31a9f3c1315a1411ec`
delta and reported two P2 findings:

- `Home::project` inherits caller Git configuration during setup. The new fixture
  now initializes real Git with a cleared environment, isolated configuration,
  empty templates/hooks and signing disabled, then sends `ProjectAdd` to the real
  owner socket. A fake owner hook/signer/config regression passes. The pre-existing
  shared helper is unchanged and tracked in Hardening #526. Earlier receipts are
  superseded; owner Git configuration was not inspected to infer what it did.
- The native version timeout equaled discovery's ten-second limit, allowing the
  outer caller to kill the wrapper before cleanup. The reviewer reproduced the
  orphan with only a fake sleeping provider, then explicitly cleaned it up. The
  wrapper now gives native version discovery three seconds, reserving cleanup
  headroom. A fake wrapper/child regression checks the real ten-second outer limit.

Targeted follow-through by that same independent reviewer found both resolved and
no new concrete regression: ready for final verification and receipt refresh.
This was not a second full-delta review or a `$code-review` invocation.

Final ordinary check after these fixes:
`CARGO_BUILD_JOBS=2 NEXTEST_TEST_THREADS=2 cargo nextest run --locked --offline -p cockpit-daemon --test native_instructions -j 2`
passed three Rust tests, including eleven Python regressions, with three native
tests explicitly skipped (3.866 seconds). The earlier `agent_onboarding` baseline
was not repeated after discovering #526. The final native command above passed
all three providers using the isolated Git setup and refreshed every receipt.
Clippy, formatting and the changed-file documentation checks pass on the fixes.

The reviewer explicitly left signed-in recognition/model understanding, Claude TUI
parity, Codex 0.161.0, real sandbox/network enforcement, approval routing, persisted
completion and total context outside these measured claims. Those limitations
remain open; no account or native test was run by the reviewer.
