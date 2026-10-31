# Cockpit review — primary-source comparisons

Companion to [the review](audit-2026-10-30.md). Sources were read on **2026-10-30**,
at immutable revisions. Repository instructions are research data, not permissions
for this session. No third-party scripts were executed or dependencies installed.

## Selection and limits

Four primary comparisons cover distinct pressures: ripgrep for a mature CLI/library
boundary, Zellij for a persistent terminal/event-driven application, Nushell for a
large Rust workspace, and Codex for agent runtime/protocol/approval handling.
OpenKeychron and OpenLogi are additional references because they informed Cockpit's
original plan and expose explicit agent-engineering contracts.

The presence of `AGENTS.md`, skills or an AI-oriented product proves documented agent
support. It does **not** quantify how much code was AI-authored or establish that a
project is “heavily AI-developed.” No author-share/productivity claim is made here.
Documentation is labeled as such; it does not independently prove its CI settings,
runtime security or claimed test results.

## Comparison

| Project and pinned revision | Specific observed pattern | Transfer to Cockpit | Analogy to reject |
| --- | --- | --- | --- |
| ripgrep `91f5d22ef48bde459d90a8c7dbeab416e97d0f2b` | Core README calls the application CLI/glue and explicitly declines an independent core-library API; reusable search/matcher/printer crates do the heavy work | Keep terminal/CLI orchestration private; extract a library for actual consumers, not every feature | Its `crates/` directory is not evidence that moving Cockpit's workspace fixes coupling; search pipeline concurrency is not host-approval orchestration |
| Zellij `3fd414fa5281690067bfecf0dad85b32e430b95f` | Architecture assigns Screen, TerminalPane and PTY bus responsibilities; thread bus/channel helpers carry typed messages and error context | Name runtime owners and attach correlation/error context across worker boundaries | Channels alone do not guarantee boundedness: helper imports include both bounded and unbounded channels; do not copy the whole multiplexer/plugin runtime |
| Nushell `f9c4652363802cd441a73691ed3c9351d0a09460` | Multiple purpose-specific crates, concrete integration-test target guidance, CI aliases and error types | Test selection must match real targets and consumers; preserve database errors through every iteration step | A language shell's parser/engine/plugin crate count does not set Cockpit's ideal crate count; its command trait is not a reason to introduce a facade for every Cockpit action |
| Codex `a4b4082b5f14a8a037386d823ddb96a85fd25975` | App-server documents thread-scoped revisions/read receipts, human checkpoints, cancellation semantics and capability handling; CI documents merge-result checks and differentiated cost tiers | Revision-aware cursors, exact approval payloads, durable delivery outcomes and targeted checks | Do not adopt Bazel, cross-platform sharding or experimental protocol names wholesale; a provider runtime cannot substitute for Cockpit's authority checks |
| OpenKeychron `477ad1b4937a8a6c2b740bfe491e11f440345acf` | Current agent guide maps hardware/protocol/CLI owners, replay fixtures, explicit hardware-only verification and one Make gate | Fake host/IPC environments must be distinct from real-device/live-host proof; task acceptance should name evidence | Hardware fixture success is not live-host proof; its documented main ruleset was not independently verified here |
| OpenLogi `cbb754e196fcf497cc8da3a8ef72206aff961172` | Current guide documents runtime agent ownership, IPC clients, scoped rules, one decision owner, narrow verification and skills linked to canonical policy | Explicit runtime owners, version-skew handling, capability-specific tests and selective guidance loading | Its 20-plus package/platform structure, xtask and GUI stack solve different deployment needs; copying them would expand Cockpit's obligations |

## Exact sources and implications

### ripgrep: reusable mechanisms, private application glue

- [Core README](https://github.com/BurntSushi/ripgrep/blob/91f5d22ef48bde459d90a8c7dbeab416e97d0f2b/crates/core/README.md),
  especially lines 6–15: CLI definitions and glue combine `grep-matcher`, `grep-regex`,
  `grep-searcher` and `grep-printer`; no planned independent core library.
- [Workspace manifest](https://github.com/BurntSushi/ripgrep/blob/91f5d22ef48bde459d90a8c7dbeab416e97d0f2b/Cargo.toml).

**Decision:** Cockpit's existing protocol and shared headless libraries earn their
boundaries through multiple consumers. A new UI facade or crate per panel does not.
Public API minimization matters more than putting “core” in a crate name. Ripgrep also
acknowledges missing reuse guidance: source quality does not eliminate onboarding work.

### Zellij: runtime responsibilities, with explicit channel trade-offs

- [Architecture](https://github.com/zellij-org/zellij/blob/3fd414fa5281690067bfecf0dad85b32e430b95f/docs/ARCHITECTURE.md).
- [Thread bus](https://github.com/zellij-org/zellij/blob/3fd414fa5281690067bfecf0dad85b32e430b95f/zellij-server/src/thread_bus.rs).
- [Channel/error-context helpers](https://github.com/zellij-org/zellij/blob/3fd414fa5281690067bfecf0dad85b32e430b95f/zellij-utils/src/channels.rs).

**Decision:** assign Cockpit UI, registry, worker observations and response transport
separate responsibilities, then specify queue capacity, cancellation and stale-result
behavior. A thread bus is a useful implementation example, not proof that any queue
fed by a thread is safe. Cockpit can use existing Rust primitives and its `System`
test seam without introducing a generic actor framework.

### Nushell: error preservation and accurate check selection

- [Agent guide](https://github.com/nushell/nushell/blob/f9c4652363802cd441a73691ed3c9351d0a09460/AGENTS.md).
- [Workspace manifest](https://github.com/nushell/nushell/blob/f9c4652363802cd441a73691ed3c9351d0a09460/Cargo.toml).
- [CI](https://github.com/nushell/nushell/blob/f9c4652363802cd441a73691ed3c9351d0a09460/.github/workflows/ci.yml).
- [SQLite implementation](https://github.com/nushell/nushell/blob/f9c4652363802cd441a73691ed3c9351d0a09460/crates/nu-command/src/database/values/sqlite.rs).
- [Failure-path regression tests](https://github.com/nushell/nushell/blob/f9c4652363802cd441a73691ed3c9351d0a09460/crates/nu-command/tests/commands/database/query_db.rs).

The pinned [change](https://github.com/nushell/nushell/commit/f9c4652363802cd441a73691ed3c9351d0a09460),
dated `2026-10-30T13:48:26Z`, repairs a row loop that discarded SQLite errors with
`if let Ok(...)`. Failed writes appeared successful; partial SELECT results concealed
later errors. The repair propagates errors and tests failures at different row/parameter paths.

**Decision:** Cockpit must preserve `Storage` errors and transaction outcomes during
performance refactoring. A shorter or paginated query is unacceptable if it turns
failure into empty/available state. Mature workspaces still require these failure tests.

### Codex: semantic receipts and exact effective approval inputs

- [App-server protocol notes](https://github.com/openai/codex/blob/a4b4082b5f14a8a037386d823ddb96a85fd25975/codex-rs/app-server/README.md).
- [Workflow strategy](https://github.com/openai/codex/blob/a4b4082b5f14a8a037386d823ddb96a85fd25975/.github/workflows/README.md).
- [Scoped sandbox-integrity guidance](https://github.com/openai/codex/blob/a4b4082b5f14a8a037386d823ddb96a85fd25975/codex-rs/exec-server/src/sandbox_integrity/AGENTS.md).
- [Approval review implementation](https://github.com/openai/codex/blob/a4b4082b5f14a8a037386d823ddb96a85fd25975/codex-rs/core/src/guardian/recorded_tool_call.rs).

The app-server notes distinguish read-state revision equality from a clock, bind receipt
updates to what the client actually saw, and distinguish cancellation acknowledgment from
completion of the underlying OS call. Human-command checkpoints cannot be satisfied by
a session/persistent approval. These are useful semantics for Cockpit; several are
explicitly experimental upstream.

The pinned [change](https://github.com/openai/codex/commit/a4b4082b5f14a8a037386d823ddb96a85fd25975),
dated `2026-10-30T15:28:20Z`, fixes direct Guardian review checking a command before
`PreToolUse` rewrites/denials. The effective command must be the reviewed command.
For Cockpit, an approval for a command that executes changed files should retain the
reviewed file/diff identity, rather than extending trust to later file contents.

The workflow strategy checks the synthetic merge result, uses Bazel for pre-merge Rust
verification, and reserves broader Cargo matrices for post-merge. **Adopt the
exact-source/coverage distinction, not the deferral of required checks.** Cockpit's
current policy and smaller Make gate remain authoritative; post-merge testing is not
permission to knowingly merge a failing required gate.

The scoped integrity guide explicitly documents five-second caller timeout, discarded
late results and deferred underlying cancellation. That is a helpful example of naming
a limitation instead of calling a timed-out caller a cancelled operation. Its telemetry
tags exclude raw paths/environment/error text. Cockpit diagnostics should likewise be
bounded and redact secrets while retaining useful, approved error evidence.

### The original comparison projects, revisited

Historical sources used by the October 28 Cockpit audit:

- [OpenKeychron guide at `2b3755d`](https://github.com/Androlax2/openkeychron/blob/2b3755d52f99d731dce7c17869e2f10d8da85c69/AGENTS.md).
- [OpenLogi guide at `90f60dd`](https://github.com/AprilNEA/OpenLogi/blob/90f60ddade073d29a837e6637ebbf46435258bed/AGENTS.md).

Current pinned guides were fetched independently:

- [OpenKeychron `477ad1b`](https://github.com/Androlax2/openkeychron/blob/477ad1b4937a8a6c2b740bfe491e11f440345acf/AGENTS.md).
- [OpenLogi `cbb754e`](https://github.com/AprilNEA/OpenLogi/blob/cbb754e196fcf497cc8da3a8ef72206aff961172/AGENTS.md).

OpenLogi's guide is unusually explicit that shared decisions have one owner, scoped
rules are not automatically loaded by every client, test tiers depend on affected
consumers, and new prose needs recurring actionable evidence. It also documents CLI
hardware-diagnostic exceptions to agent ownership. These exceptions matter: “all
clients are thin” would misdescribe its actual documented architecture.

Its structural-rule policy is promising when Cockpit has a specific re-derived
decision to forbid. Existing Cargo graph tests already prove transitive package
boundaries better than a text rule. Do not add ast-grep, new hooks or a replacement
task runner solely because a comparison project uses them.

## What to adopt, in order

1. A task/check/result contract with exact source and runtime identities.
2. Explicit runtime owners with bounded queues and deadlines.
3. Revision-aware incremental delivery and durable failure/unknown states.
4. Real-entry-point failure tests and consumer-aware inner-loop verification.
5. Small, evidence-triggered scoped guidance linked to existing commands.

Rejected for now: wholesale crate/layout copying, a new task runner, a broad plugin
framework, additional prose without measured value, and treating upstream practices
as permission to relax Cockpit's security or resource gates.
