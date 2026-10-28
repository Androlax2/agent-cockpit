# Bounded Billing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make repeat Claude captures proportional to appended transcript data and normal archive refreshes proportional to a bounded pending batch, without discarding billing evidence.

**Architecture:** Keep a locked, launch-specific transcript cursor outside the metadata spool. Treat the existing billing directory as the pending queue; claim files into a processing directory, commit validated evidence, then retain them in a processed directory. Replay retained evidence once when initializing a new archive, not on normal refreshes.

**Tech Stack:** Existing Python standard library, Rust, rusqlite; no dependencies.

**Spec:** Option A approved in this conversation for todo `16fa01b3`; constraints below incorporate the todo's preservation requirements.

## Global Constraints

- Preserve exact reply keys, idempotence, sticky-unknown conflicts, late evidence and safe file handling.
- Do not infer billing from current settings or turn unpriced records into free usage.
- Keep metadata records limited to key and billing; cursors contain no conversation text.
- Retain imported evidence on disk for archive recovery; never remove it before its database commit.
- No dependency manifests, launcher behavior, Makefiles, or permissions change.
- A normal import examines at most 256 pending/processing directory entries. Invalid files are retained separately and reported rather than starving the queue.
- Rebuilding a lost archive requires a one-time full replay of retained metadata. This recovery path is intentionally not bounded; normal refreshes are.

## Review Focus

- Truncation, replacement, incomplete final lines and concurrent captures must not skip replies.
- A launch or funding-mode change must not reuse another launch's cursor.
- A crash before or after a SQLite commit must leave replayable evidence.
- Invalid records and symlinked directories/files must not prevent valid evidence from eventually reaching the archive.
- Repeated captures after import and late conflicting evidence must preserve sticky unknown.

---

### Task 1: Incremental transcript capture

**Files:**

- Modify: `config/claude/.claude/hooks/billing-capture.py`
- Create: `tools/tests/test_billing_capture.py`

**Interfaces:** Preserve `record-transcript-billing <mode> <since> <path>` and `record-billing`. Store cursor state in `~/.cache/claude/billing-cursors/`, keyed by absolute transcript path, launch timestamp and billing mode. Serialize captures with a regular-file, no-follow advisory lock. Persist device/inode, byte offset and a digest of boundary bytes; reset on replacement, shrinkage or boundary mismatch. Save the cursor only after all preceding evidence writes succeed. Process a complete JSON final line without a newline, but leave its offset eligible for rereading if more bytes arrive.

- [ ] Write tests proving unchanged captures read only bounded cursor-validation bytes, appended replies retain exact keys, separate launches/modes rescan independently, truncated/replaced files reset, incomplete lines are retried, failed saves do not advance, concurrent captures are safe, and cursor symlinks/FIFOs are rejected.
- [ ] Run `python3 -m unittest discover -s tools/tests -p 'test_billing_capture.py'`; confirm new incremental tests fail against the original implementation.
- [ ] Implement the cursor around the existing parsing and evidence publication code, preserving timestamp filtering and error propagation.
- [ ] Run the Python tests and `cargo test --manifest-path apps/Cargo.toml -p cockpit --test costs --test session_state_hook --test opencode_plugin`; expect all tests to pass.

### Task 2: Bounded pending import and retained recovery

**Files:**

- Modify: `apps/cockpit/src/billing.rs`
- Modify: `apps/cockpit/tests/costs.rs`

**Interfaces:** Preserve `billing::import(&mut Connection) -> io::Result<usize>` and all callers. Keep existing root-level JSON producers unchanged. Use `processing/`, `processed/`, and `rejected/` beneath the billing directory; reject symlinked directories. Serialize importers with an immediate SQLite transaction. Claim pending files by rename to unique processing names before reading. Retry processing files first after interruptions. Record a recovery-completed marker in SQLite only after replaying retained evidence. Move successfully committed processing files to processed; retain malformed input in rejected and return a bounded error report. Do not cache imported evidence by mtime/size, since recaptured records must still be safe and idempotent.

- [ ] Add entry-point tests for a backlog exceeding 256 records, a drained queue with a large retained history, archive rebuild, abandoned processing files, malformed-file quarantine, symlink/FIFO rejection, duplicate recapture, and late conflicts. Keep existing totals/unpriced tests unchanged.
- [ ] Run the targeted costs tests; confirm failures at the missing batching/retention boundary.
- [ ] Implement claim/commit/retain and one-time recovery without changing reply pricing or billing conflict SQL.
- [ ] Run the targeted Rust suites and Python suite; expect all tests to pass.

### Task 3: Verification and handoff

- [ ] Repeat the synthetic measurement and record bytes read / entries examined as well as elapsed time; do not make timing assertions in tests.
- [ ] Review the final diff for crash windows, unsafe file handling and unwanted host-executed changes.
- [ ] Run the repository test suite when authorized; report exact failures rather than suppressing them.
- [ ] Commit on the existing feature branch with `Todo: 16fa01b3`, export patches, request host application through `ask-to-run`, and run the host tests after approval, as required by `CLAUDE.md`.
- [ ] Run `cockpit-todo list`, then mark `16fa01b3` done only when the requested work and required checks are complete.

## Baseline

Measured before implementation using synthetic non-sensitive data:

- Unchanged 50,750,000-byte transcript: 50,750,000 bytes reread per capture; median 23.13 ms over five runs.
- Unchanged 5,000-record spool: 5,000 stats plus 5,000 SQLite lookups per refresh; 24.63 ms in a Python equivalent of the current Rust loop (not a Rust benchmark).

The steady-state acceptance criteria are bounded transcript validation reads and no per-record visits to retained history during normal refreshes, not a machine-specific timing target.

## Verification results

- Python cursor tests: initially 5 failures out of 10; all 11 now pass, including an additional directory-replacement race test observed failing before its fix.
- Rust queue tests: initially 9 failures out of 18; the expanded costs suite and existing hook/plugin suites pass.
- Same unchanged 50,750,000-byte transcript: 1,024 transcript bytes read per repeat capture; median 0.14 ms over five runs.
- 5,000 pending records drained in 20 bounded refreshes, retaining 5,000 records. An unchanged full debug `costs` CLI run took median 2.30 ms over five runs; this includes process startup and unrelated report work, so it is not directly comparable to the Python baseline loop.
- Functional regression tests verify that normal refreshes ignore a retained directory containing 5,000 malformed records. Syscall tracing was unavailable: `strace` returned `PTRACE_TRACEME: Operation not permitted` and `ptrace(PTRACE_SEIZE): Operation not permitted`; reported through `cockpit-todo blocked` without a workaround.
- Review is an author self-review, following the explicitly approved no-subagents execution method.
- Final clone `make test` passed: 46 Python tests and 1,794 Rust tests. Costs suite: `test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.
- This parallel clone has no `origin/main` ref. Export only this task's commit relative to its recorded starting revision `1b4227480774c3a29edde480d50b7da4b43f0554`; host application still checks each patch against the running checkout before approval.
