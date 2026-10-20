# Agent build CPU budget implementation plan

> **For agentic workers:** Use executing-plans to implement inline. The user explicitly requested execution without further questions.

**Goal:** Keep parallel builds/tests from saturating all CPU threads while leaving interactive agents unrestricted.

**Architecture:** A Linux Python entry point restricts build processes and descendants to the same first half of the available CPU threads, with nice level at least +10. The shared launcher sets one CPU list and installs PATH shims for build tools; the repository's Makefile uses the same entry point for host builds. This is a scheduling safeguard, not a security boundary: absolute executable paths and builds inside separate Docker daemons are outside its coverage.

**Tech stack:** Python standard library, POSIX shell, existing Rust launcher integration tests.

**Spec:** The approved chat request: automatic shared CPU budget for builds/tests, interactive agents unrestricted, no further design questions.

## Global constraints

- No dependency or sandbox-permission changes.
- Use half the available logical CPUs, rounded down, minimum one; all builds share that set rather than receiving separate CPU allowances.
- Preserve existing lower priority, arguments, stdin/stdout, working directory and exit status.
- Nested build tools inherit the same set without repeatedly halving it.
- Fail visibly on invalid CPU lists or failed scheduling operations.

## Review focus

- Concurrent independent builds must choose the same CPU set.
- PATH shims must reach the real executable rather than recurse.
- No scheduling changes to the launcher/interactive agent itself.
- Nested Make/Cargo invocations must retain the initial budget.
- Missing tools and invalid configurations must fail rather than run unrestricted.

## Task 1: Budget runner and host entry points

**Files:** `config/bin/.local/bin/agent-build`, `tools/tests/test_agent_build.py`, `Makefile`.

**Interface:** `agent-build COMMAND [ARGUMENTS...]`; `agent-build --cpu-list` prints the default list without changing scheduling; `AGENT_BUILD_CPUS` carries the initial CPU set to descendants. Invocation through a build-tool symlink resolves that tool outside the shim directory.

- [x] Write executable-entry-point tests for affinity, priority, inheritance, real-tool resolution, streams, status and invalid input.
- [x] Run the tests and verify failure because the runner is absent.
- [x] Implement the runner and route Makefile build/test recipes through it.
- [x] Verify focused tests pass: `Ran 12 tests ... OK`.

## Task 2: Launcher integration

**Files:** `config/bin/.local/bin/greywall-agents/launcher-common.sh`, `apps/cockpit/tests/greywall_launcher.rs`, `README.md`.

**Interface:** Both existing launchers inherit `AGENT_BUILD_CPUS`; cached `cargo`, `make`, `npm`, `pnpm`, `go`, `composer`, and `pytest` shims invoke the runner. Scheduling applies only when those tools run.

- [x] Add integration tests for the agent environment and automatic build-tool restriction.
- [x] Watch the new test fail before launcher changes: `claude: build was not restricted`.
- [x] Install runner/shims in the existing read-only cache and export the CPU list.
- [x] Document scope, trade-offs and bypasses.
- [x] Run focused launcher tests and `make test` under the new budget; report all failures.
- [ ] Review diff, commit, export patches and apply with the existing host approval flow.
- [ ] Run the required live cockpit build/tests; reload after any `apps/` changes.

## Verification ledger

- Both tasks use the same runner interface and inherited CPU list; no interface conflicts.
- The execution-plan workspace helper is absent (`No such file or directory`, exit 127); reported through `claude-todo blocked`.
- Focused launcher suite: `test result: ok. 38 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
- `sh -n config/bin/.local/bin/greywall-agents/launcher-common.sh` and `git diff --check`: exit 0.
- Independent read-only whole-change review: no concrete correctness or regression findings.
- First full-suite attempt: `make: *** [Makefile:135: test] Terminated` at the harness's 120-second timeout, with no failed test before termination.
- Full rerun without the harness timeout: `make test` exited 0; 1,223 Rust tests passed, 0 failed, and Python reported `Ran 12 tests ... OK`. Existing warnings: two unused `Result` values in `desktop/src/tests.rs`, one unnecessary `mut` in `cockpit/src/tests.rs`.
