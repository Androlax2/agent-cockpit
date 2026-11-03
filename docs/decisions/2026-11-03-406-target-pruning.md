# Target pruning requires an immutable owner decision and durable launch exclusion

Partial issue #406. The coordinator accepted the scope and defaults under Theo's
delegated milestone authority; the worker chose the capability APIs and descriptor
implementation to preserve transaction ownership. This does not authorize live cleanup.

- **Only a registered non-main clone's literal `target/`.** The owner must declare
  it disposable and exclude unmanaged concurrent writers. Tracked or nonignored
  files, repository/application-state components, symlinks, hardlinks and special
  files refuse preparation. This contract does not protect against hostile writers
  with the owner's OS rights; mode 0700 and inode comparisons do not provide that proof.
- **Measure before deciding.** `torlek storage prune CHECKOUT --dry-run` observes
  without persisting a plan. Without `--dry-run`, it prepares an immutable plan and
  prints the explicit ID, revision and database epoch needed for confirmation.
  Incomplete or unavailable measurements cannot authorize removal. Bytes mean
  regular-file logical lengths, not allocated blocks or guaranteed reclaimed space.
- **Eligibility is separate from delivery or session labels.** Associated sessions
  must be ended with observed execution termination, or the owner must configure a
  full LOCAL base ref. Local ancestry binds and rechecks the clone tip and main
  checkout's base OIDs; it proves neither remote publication nor a merged pull request.
  Occupied, live, unknown or pending managed work defers cleanup.
- **Automatic behavior is real and opt-in.** Owner-only `[storage]` settings are
  `target_prune_automatic = false`, `target_prune_projects = []`,
  `target_prune_confirm_above_mib = 1024`, `target_prune_interval_minutes = 60` and
  unset `target_prune_base_ref` by default. Enabled, allowlisted targets at or below
  the threshold can be consumed automatically; larger targets need explicit plan
  confirmation. Configuration loads at daemon startup; project configuration cannot
  enable it. A changed effective policy invalidates the earlier decision.
- **The existing writer owns admission.** Projects persists one operation, its
  bounded manifest and global/per-checkout guards in the same reservation transaction.
  Sessions, verification after awaits, and approved run intents check the guards in
  their existing write transactions. The approval-to-projects dependency is not reversed.
  Global managed-idle evidence includes pending starts, observed process termination,
  scheduling placements, queue pause and foreground resource policy.
- **Mount proof precedes effects.** Linux uses the existing pinned rustix API:
  descriptor `statx` must return mount identity, and child `openat2` uses `BENEATH`,
  `NO_XDEV`, `NO_SYMLINKS` and `NO_MAGICLINKS`. Device equality alone is insufficient
  for bind mounts. Unsupported kernels/platforms or unknown metadata refuse before
  detachment. Linux fixture success does not establish support on every WSL2 kernel;
  no live mounts or host acceptance were exercised.
- **Detach and remove only proved entries.** The registered root/marker and exact
  target are revalidated. An exclusively created operation quarantine and marker are
  recorded before a no-replace rename. Every deletion reopens relative components,
  checks mount and entry identities, then removes only a manifest entry. Unexpected
  contents or replacements stop the operation; no wildcard removal or automatic
  restoration can overwrite a new `target/`. The owner no-mutator prerequisite still
  applies between identity checks and name-based rename/unlink syscalls.
- **Bounded work uses Bulk admission.** A manifest allows at most 100,000 entries,
  8 MiB encoded data and depth 64, with a cooperative five-second scan budget. At most
  eight unfinished plans and 64 MiB of manifests are admitted. Removal slices stop
  after 1,000 entries or 100 ms cooperatively. An individual filesystem syscall can
  still block; status uses Control admission and does no filesystem traversal.
- **Interrupted effects retain their identity.** The blocking closure keeps exclusive
  database ownership even if its async caller is canceled. Startup reconciliation runs
  after process reconciliation and before session restore. Pausing releases the global
  guard only after a joined step and keeps the checkout guard; guards never expire into
  another writer. Current generation, phase and cursor bind each receipt. Missing
  entries after an unreceipted unlink count as absent with unconfirmed attribution,
  never confirmed freed bytes. Status preserves the recorded target/quarantine
  locations, which may themselves be absent or uncertain.
- **Rejected:** whole-clone sweep, a live-session occupancy snapshot without durable
  launch fences, path-recursive deletion, `st_dev` alone, automatic restoration,
  and silently refreshing a stale owner plan into current intent.

Implementation: [projects target pruning](../../crates/cockpit-projects/src/target_prune.rs),
[daemon orchestration](../../crates/cockpit-daemon/src/storage.rs),
[owner CLI](../../crates/cockpit-cli/src/storage.rs).
Evidence: [real daemon entrypoints](../../crates/cockpit-daemon/tests/sessions/target_prune.rs),
[CLI fixtures](../../crates/cockpit-cli/tests/storage_prune.rs) and
[project durability fixtures](../../crates/cockpit-projects/tests/target_prune.rs).

Database/output retention, shared caches, memory policy and low-disk handling remain
separate #406 work. These isolated fixtures do not establish live cleanup acceptance.
