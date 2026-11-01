# Artifacts are opened segment by segment with `O_NOFOLLOW`, and any symbolic link is refused

Issue #326. Decided by the worker: the issue names `openat` + `O_NOFOLLOW` as a fix, and it is
the only one that works the same on Linux and macOS without a check-then-open window.

- **Every segment of an artifact path is opened relative to the directory opened before it,
  with `O_NOFOLLOW`** (`crates/cockpit-verification/src/artifacts.rs`). The checkout root is
  opened once; nothing is resolved by name after it was checked, so a process swapping a
  segment for a link cannot lead the open out of the checkout. This replaces the
  "symbolic links resolved and kept inside it" rule of `2026-11-01-40-verification-receipts.md`.
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
