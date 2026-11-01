# Receipt freshness is computed at evaluation, from the check, the head and artifact digests

Issue #40. Decided by the worker, implementing reports 06 and 09 as written.

- **Freshness is never stored.** `Evidence::evaluate` compares each receipt with the current
  check definition, source and artifact files when a gate is asked. A stored "stale" flag
  would need every edit to reach verification first; a receipt computed fresh on read
  cannot be shown current after its file changed.
- **Applicability matches the head and the uncommitted-changes digest, not the base.** The
  same head is the same code whatever it was compared against; the base stays on the
  receipt for the report. The caller computes the dirty digest: verification runs no
  program (`verification -> processes`).
- **Artifacts are fingerprinted before the run** (`fingerprint`, SHA-256 per file inside the
  checkout, symbolic links resolved and kept inside it). An edit during the run therefore
  makes the receipt stale too: the safe direction.
- **A receipt's identity is its source and its check definition, matched in the query.**
  `evidence` loads the newest receipt of exactly the required definition on exactly the
  source asked about; once its artifacts changed it is stale, and an older pass does not
  stand in for a newer failure. No loading window exists, so parallel clones' runs on
  other heads cannot crowd it out. The source of the newest other receipt only explains
  `check-changed` (same source) or `source-changed`.
- **A run id makes delivery idempotent.** The same run again returns its receipt; another
  result under the same run id is refused, so evidence is append-only.
- **Prose claims are kept, never counted, and bound to a source.** A check with only claims
  about this source is `unverified`; a claim about another head shows nowhere else; a claim
  after a stale receipt leaves it stale.
- **`sha2 = "=0.10.9"`**, the pin `cockpit-approvals` already carries
  (`2026-10-31-34-approvals-dependencies.md`), so no new package enters `Cargo.lock`.
- **Rejected:** storing a check-definition digest (comparing the stored fields is exact and
  needs no versioned encoding); invalidating receipts on a file-change event (needs a
  watcher and still races the read).
