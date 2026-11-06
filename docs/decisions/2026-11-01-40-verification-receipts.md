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
  checkout; the #326 section below replaced "symbolic links resolved and kept inside it"
  with refusing every link). An edit during the run therefore makes the receipt stale too:
  the safe direction.
- **A receipt's identity is its source and its check definition, matched in the query.**
  `evidence` loads the newest receipt of exactly the required definition on exactly the
  source asked about; once its artifacts changed it is stale, and an older pass does not
  stand in for a newer failure. No loading window exists, so parallel clones' runs on
  other heads cannot crowd it out. "Newest" is by `finished_at`, not insertion, and a
  future `finished_at` is refused: a run delivered late never passes a newer result, and
  a long failure finishing after a short pass is the latest word.
- **Without an exact receipt, a run of another definition on this source outranks claims,
  and claims about this source outrank runs elsewhere.** So a prose claim on a new head
  stays visibly `unverified` instead of hiding behind `source-changed` from an old run.
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

## Revisited 2026-11-01 (#326)

Decided by the worker: the issue names `openat` + `O_NOFOLLOW` as a fix, and it is the only
one that works the same on Linux and macOS without a check-then-open window. Artifacts are
opened segment by segment with `O_NOFOLLOW`, and any symbolic link is refused.

- **Every segment of an artifact path is opened relative to the directory opened before it,
  with `O_NOFOLLOW`** (`crates/cockpit-verification/src/artifacts.rs`). The checkout root is
  opened once; nothing is resolved by name after it was checked, so a process swapping a
  segment for a link cannot lead the open out of the checkout. This replaces the
  "symbolic links resolved and kept inside it" rule above.
- **A link is refused even when it stays inside the checkout.** Fingerprinting refuses it;
  evaluation reports the receipt `stale(artifact-unreadable)`. Following in-checkout links
  safely would need `openat2(RESOLVE_BENEATH)`, which macOS lacks; a check's artifacts are
  the project's own files, so a link there is rare and fails loud.
- **Segments are opened without `O_DIRECTORY` and their type read with `fstat`**: Linux
  answers `ENOTDIR` for a link under `O_DIRECTORY | O_NOFOLLOW`, which would report a swapped
  directory as a missing file instead of a link. Non-blocking, so a FIFO is reported, not
  waited on.
- **`rustix = "=1.1.6"`** with `fs`, the pin the other crates carry; no new package enters
  `Cargo.lock`.
- **Rejected:** opening first and comparing `/proc/self/fd` (Linux) or `F_GETPATH` (macOS)
  with the root: two platform paths and a comparison by name that renames still race.
