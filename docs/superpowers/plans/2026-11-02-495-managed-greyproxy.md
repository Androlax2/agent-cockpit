# Managed Greyproxy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A fresh Cockpit profile can configure and start its pinned proxy, diagnose failures,
and refuse sandbox launches when that owned proxy cannot serve them.

**Architecture:** Keep lifecycle and health in `cockpit-sandbox`; the offline CLI calls it
and the daemon gathers its current health on a blocking worker. Use existing systemd/user
and launchd conventions with separate units and private per-profile state.

**Tech Stack:** Rust standard library, existing serde_json/tempfile, curl, systemd/launchd.

**Spec:** [Issue decision](../../decisions/2026-11-02-495-managed-greyproxy.md), implementing
[GitHub #495](https://github.com/Androlax2/agent-cockpit/issues/495).

## Global Constraints

- Fixed loopback ports, explicitly approved; never adopt or stop an occupied foreign service.
- No dependencies, manifest changes, legacy host actions, new grants or credential expansion.
- Only verified published engine pins; preserve #470's installation and archive guarantees.
- Tests use fixtures/fake commands/temp homes; real engines only in isolated CI for lifecycle.
- One issue/branch/PR, one fresh independent full-delta review, normal Git history only.

## Review Focus

- A healthy-looking foreign HTTP server must not become an owned proxy or receive a token.
- A modified/symlinked service file or private state must be refused before service actions.
- Hanging commands, descendants holding pipes and oversized output must have bounded costs.
- A healthy proxy must survive an idempotent install; a stopped/unhealthy one blocks launches.
- Named profiles and unusual home paths must retain correct paths, escaping and repair advice.

### Task 1: Managed configuration, lifecycle and health

**Files:** Create `crates/cockpit-sandbox/src/proxy.rs` and scoped helper/test modules;
modify `crates/cockpit-sandbox/src/lib.rs` and `README.md`.

**Interfaces:** `ManagedProxy::new(&Environment, &Platform) -> Result<Self, ProxyError>`;
`install`, `restart`, `stop`, `health` return typed failures with stable codes and repair
advice. `data_dir` and `unit_path` identify paths to protect at launch.

- [ ] Write failing tests for owned configuration and units on Linux/macOS/WSL2, unconfigured
  and unhealthy proxies, occupied TCP/UDP endpoints, tamper/symlink refusal, idempotency,
  command/output deadlines, private token handling, and authenticated health.
- [ ] Run the focused sandbox test selection and record the expected failures.
- [ ] Implement the explicit lifecycle using bounded commands, atomic private files and
  an ownership receipt. Keep the service's engine/data paths absolute and profile scoped.
- [ ] Run `CARGO_BUILD_JOBS=2 cargo nextest run --locked -p cockpit-sandbox --test-threads 2`.
- [ ] Commit the capability with its tests.

### Task 2: Product entry points and launch/readiness wiring

**Files:** Modify `cockpit-cli/src/{main,sandbox_install}.rs`, CLI installer tests;
`cockpit-sandbox/src/{launch,greywall}.rs`; daemon `serve.rs`, `readiness.rs` and
`tests/{readiness,sandbox,real_daemon/mod}.rs`; sessions `lifecycle.rs`; readiness facts/tests.

**Interfaces:** CLI `sandbox install [--engines-only]`, `sandbox status`, `sandbox restart`,
`sandbox stop`. Sandbox carries managed proxy context; health is checked before a required
launch and reported separately by doctor. Greywall receives only the owned data path.

- [ ] Add CLI tests that prove offline status and validation through the real binary;
  daemon tests prove health refresh, launch refusal before execution and profile repair advice.
- [ ] Run each new test and record its expected failure before implementation.
- [ ] Wire callers without changing wire fields or database transaction ownership. Add
  explicit proxy state/unit denial even for a broad writable checkout. Preserve dry runs.
- [ ] Run focused CLI, sandbox, readiness and daemon tests with two build/test workers.
- [ ] Commit the wiring and fixture updates.

### Task 3: Evidence, supported-platform validation and review

**Files:** Update `tools/verify-engine-install.py` for explicit binaries-only smoke;
add a bounded isolated lifecycle/probe smoke as appropriate; update the coverage map and
decision evidence. Coordinate workflow edits with the release lane before any workflow push.

- [ ] Keep local installer smoke free of service effects; prove real lifecycle/network only
  in isolated CI. Record unavailable platform/capability checks without marking them passed.
- [ ] Run affected fmt, Clippy, rustdoc, crate graph, Python and docs checks once on final code.
- [ ] Invoke the installed Codex `$code-review` skill at medium on the final delta and
  record its revision/findings. Its inline recipe does not replace the lane's separate
  fresh independent full-delta review; preserve each review's actual provenance.
- [ ] Fix review findings with focused regression checks; open a PR with `Closes` only for
  measured acceptance, otherwise `Part of`. Track remaining #61/#433 evidence explicitly.
- [ ] Normal push, verify CI, then repository-authorized squash auto-merge after review/checks.
