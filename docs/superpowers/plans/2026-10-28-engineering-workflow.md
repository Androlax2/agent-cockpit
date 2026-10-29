# Cockpit Engineering Workflow Implementation Plan

> **For agentic workers:** Use the executing-plans skill for inline execution, or
> subagent-driven-development if Theo selects delegated execution. Check off each
> independently reviewed phase; this proposal does not authorize implementation.

**Goal:** Give Cockpit reproducible engineering checks, useful AI/contributor guidance,
and independently testable Rust boundaries without changing product behavior.

**Architecture:** Retain the Make-based workflow and `apps/` Cargo workspace. Introduce
checks before structural refactors; add cohesive libraries under `apps/crates/` while
keeping installed executable owners and paths stable.

**Tech Stack:** Rust, Python, Bash/POSIX shell, fish, JavaScript, Lua, Markdown; Linux,
GTK4 and existing command/PTY integration tests.

**Spec:** [Engineering audit and proposed scope](../../audits/audit-2026-10-28.md).
Status: Theo approved inline execution, including baseline builds. Installs, manifests,
dependency changes and workflow pushes retain separate approval gates. Baseline findings
determine cleanup batch sizes. Later crate extractions require reviewed move maps.

## Global constraints

Implementation checkpoint (2026-10-29): integrated source is committed through
`2c5ef2b`, based on `bb98448`. Independent reviews found no remaining code issues;
host receipt `0f9a4cc2a2f3` passed 220 Python and 3,262 Rust tests (zero skipped).
Clippy, Rustdoc and dependency-policy checks passed separately. Final Markdown
whitespace ownership, aggregate CI, GitHub publication and reviewed live application
remain open; these results do not establish deployment or GitHub runner execution.

- No product behavior, wire-format, database-schema or approval-policy changes.
- Manifest edits and tool installations require explicit approval before execution.
- Pushes, workflow activation and host application retain existing approval requirements.
- Keep `apps/target/release`, binary names, workspace lockfile and test coverage.
- Inspect current source before execution: this parallel checkout may predate applied
  resource-budget and shared-test-lock changes. Do not overwrite those implementations.
- Changes to Makefile, hooks and deployment scripts must be called out as host-executed.
- Fix violations rather than suppressing them globally or disabling tests.

## Review focus

- Extensionless executables and mixed shell dialects must receive the correct checks.
- Missing Node/browser/GTK/PTY prerequisites must fail clearly, not skip silently.
- Crate moves must preserve executable discovery and repository-relative test fixtures.
- Dependency direction must stay acyclic; wire parsing must not acquire storage or UI.
- Local quality checks must never install, deploy or invoke host approval automatically.

## Options

- **A — staged alignment (recommended):** checks and guidance first, then daemon and
  protocol extractions. Smaller reviews and attributable failures; several phases.
- **B — tooling and guidance only:** faster first delivery, but retains current crate
  coupling. Architectural work would remain a separately approved follow-up.
- **C — comprehensive crate/layout rewrite:** broad structural consistency in one pass,
  but large conflicts and binary/test path risk. Not recommended.

## Phase 1 — Measure and define the gate

**Files:** existing `Makefile`, `apps/Cargo.toml`, member manifests, test entry points;
new `docs/DEVELOPMENT.md` during implementation.

- [x] Reconcile checkout revision and active related work before editing; preserve
  untracked session configuration. Follow authorized Git synchronization rules.
- [x] Inventory installed tool versions and test prerequisites without installing tools.
- [x] Run approved baseline checks once, preserving command, exit status and output:
  formatter, default Clippy, existing Python suite, existing `make test`, rustdoc.
  The full test command builds OSD, so build approval is required first.
- [x] Record reproduced failures separately from missing tools and previously reported
  flakes. Do not infer a green baseline from source inspection.
- [x] Approve exact tool versions, manifest changes and new tool choices before phase 2.

Existing commands to preserve:

```sh
python3 -m unittest discover -s tools/tests -p 'test_*.py'
make test
```

Proposed Rust checks, wrapped by the existing resource-control runner in Make recipes:

```sh
cargo fmt --manifest-path apps/Cargo.toml --all -- --check
cargo clippy --manifest-path apps/Cargo.toml --workspace --all-targets --locked -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --manifest-path apps/Cargo.toml --workspace --no-deps --document-private-items --locked
```

These commands are planned verification, not results from this review.

## Phase 2 — Shared linting and CI

**Files:** `Makefile`, `.github/workflows/ci.yml`, `.editorconfig`,
`rust-toolchain.toml`, `rustfmt.toml`, `ruff.toml`, `.markdownlint-cli2.jsonc`,
`.config/typos.toml`, `.cargo/deny.toml`, `apps/Cargo.toml`, both member manifests;
tool discovery/check script under `tools/` only where Make alone is insufficient.

**Interface:** proposed public targets `fmt-check`, `clippy`, `test`, `doc`,
`lint-python`, `lint-shell`, `lint-js`, `lint-markdown`, `check-links`, `typos`, `deny`,
and aggregate `ci`. Formatting writes are explicit separate commands.

- [x] Cover tracked Rust with rustfmt and shared inherited Clippy lints. Start with
  ordinary Clippy; add unsafe documentation, suppression reasons and selected stricter
  lints only with reviewed fixes. Do not enable all pedantic lints then blanket-allow them.
- [x] Add `--locked` to existing Cargo build/test recipes as well as new checks,
  preserving the build wrapper and the OSD build prerequisite in `make test`.
- [x] Configure Ruff for Python, including extensionless Python entry points; use
  ShellCheck/shfmt only for supported shell dialects and fish's own checks for fish.
- [x] Select and approve pinned JavaScript/TypeScript tooling without downloading through `npx`;
  configure browser versus Node environments explicitly. Inventory Lua checking needs
  against its actual host runtime before choosing another dependency.
- [x] Add Markdown style and local links/anchors checks, preserving valid intentional
  HTML and code examples. Add typos and cargo-deny policies fitted to Cockpit's graph.
- [ ] Fix each language's findings in separate reviewable batches. Keep mechanical
  formatting separate from behavior-sensitive error/unsafe-code changes.
- [x] Add meaningful discovery tests: extensionless Python/Bash included, fish not sent
  to ShellCheck, filenames with spaces handled, missing tools return nonzero.
- [x] Add Linux CI jobs calling these exact Make targets, read-only repository
  permissions, immutable action revisions and superseded-run cancellation. Document
  explicit system/tool prerequisites; no dependency on Theo's live desktop or secrets.
- [ ] Run each job's command locally on the final tree, then `make ci` once when
  authorized. Document environment-specific integration/browser checks separately;
  their absence must never be described as full product verification.

## Phase 3 — AI docs, Markdown and workflow skills

**Files:** `AGENTS.md`, `CLAUDE.md`, `CONTRIBUTING.md`, `docs/DEVELOPMENT.md`,
`docs/ARCHITECTURE.md`, `docs/DECISIONS.md`, `.agents/rules/{rust,scripts,protocol,ci}.md`,
`.agents/skills/verify-cockpit/SKILL.md`, `.agents/skills/apply-cockpit/SKILL.md`.

- [x] Write the current architecture and boundary map, including host versus sandbox,
  trusted request handling, SQLite ownership and binary/test paths.
- [x] Make `AGENTS.md` the cross-client engineering index. Preserve existing
  `CLAUDE.md` Git authorization and apply policy verbatim; link canonical guidance.
- [x] Put path-scoped guidance in `.agents/rules/`; explicitly index it for clients
  that do not auto-load scoped files. Do not assume identical client skill discovery.
- [x] Create only two focused skills: selecting verification and executing the approved
  apply workflow. Load skill-creator before authoring; validate with scenarios for a
  shell-only edit, daemon change, missing prerequisite and absent host approval.
- [x] Link the actual Make commands, environment prerequisites, focused/full test
  selection, and review workflow from developer docs. Label planned architecture as
  proposed until implemented. Coordinate README additions with todo `a3fea48f`.
- [ ] Verify relative links, Markdown checks and command definitions; check that no
  new guidance grants permissions or repeats conflicting policies.

## Phase 4 — Headless daemon crate

**Files:** `apps/Cargo.toml`, `apps/osd/Cargo.toml`, `apps/osd/src/lib.rs`,
`apps/osd/src/bin/osd.rs`; proposed `apps/crates/cockpit-daemon/{Cargo.toml,src/}`.

- [x] Produce and review an exact import/test move map for `registry`, `chief`, `teams`
  and their helper dependencies. Inspect `fifo` ownership; do not create a crate for it.
- [x] With manifest approval, move the engine and its existing tests together. Keep
  GTK binaries in `osd`, and retain cross-layer tests with explicit dev dependencies.
- [x] Add a dependency regression check proving the daemon library has no GTK runtime
  dependency. Preserve existing public behavior and fixture coverage.
- [ ] Run focused registry/chief/team tests, executable integration tests, then the
  approved full gate. Verify wrappers still resolve `apps/target/release/osd`.

## Phase 5 — Contracts, then reassess UI

**Files:** `apps/cockpit/src/{teams,chief}.rs`, their consumers in both packages;
proposed `apps/crates/cockpit-protocol/{Cargo.toml,src/}`.

- [x] Map mutually referencing team/chief wire types into one cohesive protocol crate.
  Keep persistence, authorization decisions and shared SQLite transactions with their
  current owners. Avoid separate team/chief crates that would depend on each other.
- [x] Preserve and run hostile-payload, serialization and CLI/daemon roundtrip tests.
  Add a dependency guard against protocol dependencies on GTK, terminal UI or SQLite.
- [x] Review `status`, `costs`, `app::refresh`, todo drafts and approval terminal code
  for ownership changes needed before UI extraction; write a separate concrete move
  plan only if the reduction in coupling justifies it.
- [x] Keep executables and integration test ownership stable. Do not move the whole
  workspace to root `crates/` merely to match the reference repositories.

## Completion and handoff

Each phase needs its own diff, actual verification results and review. Protected
operations remain separately approved. Todo `5779dac9` stays open through implementation;
producing this plan is not completion of the requested alignment.

Recommended execution: inline in this GPT-6 Astra session, with independent review of
the protocol and daemon moves. Phase 1 determines cleanup effort; reliable implementation
time estimates require the lint/test baseline rather than file-count guesses.
