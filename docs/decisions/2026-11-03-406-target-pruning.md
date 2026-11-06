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
  Unmeasured non-candidates are unavailable, not absent. The CLI returns failure for
  unavailable/incomplete measurements and for an Apply that has not completed;
  inspecting a retained status remains a successful query.
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
  another writer. Current database epoch, generation, phase and cursor bind each receipt. Missing
  entries after an unreceipted unlink count as absent with unconfirmed attribution,
  never confirmed freed bytes. Status preserves the recorded target/quarantine
  locations, which may themselves be absent or uncertain.
- **Owner abandonment completes recovery without filesystem effects.** The coordinator
  approved this bounded recovery amendment under the same delegated source authority.
  `torlek storage prune abandon ID --revision N --epoch E` requires the explicit
  current database epoch shown by status, separately from the historical plan epoch
  after restore. It refuses active or unreceipted steps; cancellation or elapsed time
  never proves that a syscall ended. The writer fences the generation, releases only
  that operation's exact guards and records `abandoned`, retaining paths, byte counts
  and the previous reason. Remaining target, quarantine and source files stay untouched;
  any manual disposition is a separate owner action, with no implied restoration.
  Matching completed/abandoned replays return unchanged history; stale identity refuses.
  Abandoned operations never resume. A retained capability rechecks the current step
  before filesystem work, and late receipts cannot overwrite the terminal summary.
  Later cleanup requires a fresh measured plan and the original policy/identity checks;
  still-enabled automatic policy can create such a new plan, never revive the old one.
- **Rejected:** whole-clone sweep, a live-session occupancy snapshot without durable
  launch fences, path-recursive deletion, `st_dev` alone, automatic restoration,
  and silently refreshing a stale owner plan into current intent.

Implementation: [projects target pruning](../../crates/cockpit-projects/src/target_prune.rs),
[daemon orchestration](../../crates/cockpit-daemon/src/storage.rs),
[owner CLI](../../crates/cockpit-cli/src/storage.rs).
Evidence: [real daemon entrypoints](../../crates/cockpit-daemon/tests/sessions/target_prune.rs),
[CLI fixtures](../../crates/cockpit-cli/tests/storage_prune.rs) and
[project durability fixtures](../../crates/cockpit-projects/tests/target_prune.rs).
The [owner recovery fixtures](../../crates/cockpit-daemon/tests/sessions/target_prune/abandon.rs)
cover unchanged files, unrelated guards, stale/agent requests, restored database identity,
terminal replay and retained callbacks across restart.

Database/output retention, shared caches, memory policy and low-disk handling remain
separate #406 work. These isolated fixtures do not establish live cleanup acceptance.

## Revisited 2026-11-03 (#406, the storage report)

The first source part of #406, scoped by the coordinator under Theo's delegated Alpha 2
implementation authority; measurement and bounds chosen by the worker. The coordinator
authorized reusing the existing exact `rustix =1.1.6` pin with `default-features = false` and
`features = ["fs", "std"]` in `cockpit-projects`. `torlek storage` reports registered clone
target sizes without granting cleanup authority; the pruning decision above builds on its
descriptor-based traversal. Bounds and output are in `crates/cockpit-projects/README.md`.

- **One read-only entry point.** `torlek storage` sends `StorageReport` to the owner socket.
  The daemon refuses agents before scheduling filesystem work and uses the Bulk admission and
  `projects::blocking` boundary; no Control worker walks a filesystem. The report introduces
  no schema, setting, cache, retention or deletion policy.
- **Registered identities only.** The report selects parallel/review clone registrations,
  including occupied clones, by checkout id, incarnation and recorded `.git` marker. It uses
  the literal `target/`, not Cargo configuration, `CARGO_TARGET_DIR` or arbitrary paths, and
  runs no Git, Cargo or shell while measuring.
- **Apparent bytes.** Sum of regular-file lengths from metadata, hardlinks counted once per
  entry: not allocated or reclaimable space, and not a prune estimate. Missing targets are
  distinct from complete empty ones; incomplete results keep observed bytes and a reason;
  unavailable results have no byte value. The CLI exits 1 for unavailable, incomplete or
  truncated reports, and escapes control characters in table cells. Removing and deleting
  checkouts are not reported.
- **Bounded work, per clone.** The first limits (100,000 entries and five seconds shared by
  the whole report) left every clone but one unavailable on real 17 to 38 GB targets (#764).
  Each clone now gets the smaller of three seconds and an equal share of the time left, so a
  huge `target/` ends as `incomplete` with "after N entries" in the reason and never starves
  the clones after it. A count cut off by time depends on disk speed and on how many clones
  are registered; the entry limit is the reproducible one. The report may hold its Bulk slot
  and a blocking thread for up to 30 seconds, and a stalled kernel filesystem call cannot be
  interrupted. Bulk containment keeps the admission until the work ends, after a client
  disconnect too. Absolute checkout paths have a 64-component bound.
- **Do not follow links, and race limits are visible.** Each ancestor is opened relative to
  its already-open descriptor with `DIRECTORY | NOFOLLOW`; Linux uses metadata-only `O_PATH`
  anchors, so enumeration permission outside `target/` is unnecessary. Identity is compared
  after opening and again after traversal. Files can still grow or move between metadata
  calls: this is not a snapshot, and partial bytes are not a guaranteed lower bound. The
  traversal uses no unsafe FFI and no dependency on the verification layer.
- **Rejected:** path-check-then-follow traversal, because a replacement symlink can escape
  the registered tree; shell `du`, because its identity and traversal policy would differ;
  assuming unreadable means zero; automatic cleanup based on this observation.

Implementation: `crates/cockpit-projects/src/storage.rs`. Evidence:
`crates/cockpit-projects/tests/storage.rs`, `crates/cockpit-projects/src/storage_tests.rs`,
`crates/cockpit-daemon/tests/storage.rs` and `crates/cockpit-cli/tests/storage.rs`. No live
storage scan or pruning acceptance follows from these fixtures.
