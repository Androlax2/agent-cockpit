# Output ownership and pending results precede launch

Issue #406, first output-lifecycle source part. Decided by the worker under the
coordinator's bounded assignment: recovery must distinguish a reservation from an
attempted external effect. This does not activate retention or complete #406/#314.

- **Reservation owns attribution.** `cockpit-processes` records an output identity
  with every execution intent. Sessions, approvals and verification bind canonical
  project, optional session, capture mode and origin database epoch in that same
  transaction. Older executions remain explicitly unknown. Process rows and IDs
  are never recycled. PTY and hosted-terminal control files are not stdout logs.
- **A launch must claim its intent before effects.** The launch-start marker and
  current execution state, capture mode and file-reader guard are checked under
  the writer before native spawn or
  hosted command preparation. An untouched reservation can therefore be ended as
  never started; a stale producer cannot launch it after recovery. A claimed but
  unidentified launch remains uncertain, even if no PID or output file is found.
- **Readers retain actual lifetimes.** Managed file readers carry non-expiring
  guards and an `Arc<Database>`. Blocking file work owns its guard until it really
  returns; cancelling its async waiter is not completion. A stream retains its
  guard until its last viewer and drainer have finished. Launch diagnostics use
  the same protection for managed file output. Unknown legacy callers remain
  conservatively protected, not candidates for future deletion.
- **Pending result is durable intent, not a receipt.** Verification reserves its
  epoch-qualified run key, check/source/artifacts and pending output hold before
  launch. Normal collection seals the receipt and releases the hold in one writer
  transaction. Existing run keys stay unchanged; ordinary identical receipt
  retries and conflicting-result refusal remain intact. No capacity cap or claim
  tombstone is introduced. Metadata remains growing, as do existing audit IDs.
- **Recovery is an explicit owner decision.** `torlek check pending --project P`
  shows origin and current database epochs separately. `torlek check
  resolve-pending RUN --project P --revision N --epoch E` accepts only explicitly
  observed identities and an inactive collector, plus observed termination or
  durable proof that launch never began. It records only `unknown`, preserves
  all output files and cannot overwrite a sealed result. Replays are harmless;
  a late producer remains fenced. A claimed launch with unknown containment is
  refused, not timed out into success. A stale revision is refused before containment
  observation. Polling rechecks an apparently orphaned result under the writer,
  because its first read snapshot can predate the collector commit/guard release.
  No auto-refresh confirms a restored epoch.
- **Protection facts are not deletion authority.** Process activity, pending
  results and unknown attribution protect output. Sessions supplies current
  live/latest protection within the caller's snapshot. Any future retirement
  must compose these facts with citation and reader protections in the same
  transaction; this part implements no retirement, policy settings, TUI changes
  or filesystem deletion.

Rejected: deriving ownership from a path or an eventual receipt, treating a
missing PID as termination, dropping a database opener while blocking work still
uses its output, and releasing a pending hold before the receipt commits.

Fixture coverage lives in `cockpit-processes/tests/output_catalog.rs`,
`cockpit-verification/tests/pending.rs`, daemon `tests/verification/pending.rs`
and `tests/sessions/output_catalog.rs`, plus the actual CLI/protocol checks.
Tests use isolated homes and processes. They are source evidence, not Theo's live
storage acceptance or permission to activate cleanup.
