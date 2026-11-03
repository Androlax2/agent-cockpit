# Report registered clone target sizes without granting cleanup authority

Issue #406, first source part only. Scope assigned by the coordinator under Theo's
delegated Alpha 2 implementation authority. Measurement and bounds chosen by the worker;
the coordinator explicitly authorized reusing the existing exact `rustix =1.1.6` pin with
`default-features = false` and `features = ["fs", "std"]` in `cockpit-projects`.

- **One read-only entry point.** `torlek storage` sends `StorageReport` to the owner socket.
  The daemon refuses agents before scheduling filesystem work. It uses the existing Bulk
  admission and `projects::blocking` boundary; no Control worker walks a filesystem.
  No schema, setting, cache, retention or deletion policy is introduced. #406 remains open.
- **Registered identities only.** The report selects parallel/review clone registrations,
  including occupied clones. Their checkout id, incarnation and recorded `.git` marker
  identify them. Removing/deleting checkouts are unavailable. The report uses the literal
  `target/`, not Cargo configuration, `CARGO_TARGET_DIR`, arbitrary paths or unregistered
  sibling directories. It does not run Git, Cargo or a shell while measuring.
- **Apparent bytes.** Sum regular-file lengths from metadata, without reading file contents.
  Hardlinks count once per entry. Sparse files, compression, directories and filesystem
  allocation are not measured: the number is not reclaimable space or a prune estimate.
  Missing targets are distinct from complete empty ones. Incomplete results retain observed
  bytes and a reason; unavailable results have no byte value. CLI exit 1 also covers a
  truncated report. Control characters in table cells are escaped.
- **Bounded work.** At most 128 clone rows, 100,000 directory entries across the report
  (including filesystem `.`/`..` entries), 64 target directory levels and a cooperative
  five-second scan budget. Absolute checkout paths also have a 64-component bound. Entry
  and time limits are checked between entries/clones; the bounded identity recheck still
  runs after traversal. A stalled kernel filesystem call cannot be forcibly interrupted.
  Existing Bulk containment retains admission until the work actually ends, including
  after a client disconnect or lane budget warning.
- **Do not follow links.** Open each ancestor relative to its already-open descriptor;
  directory opens require `DIRECTORY | NOFOLLOW`. Linux ancestors and the identity
  directory use metadata-only `O_PATH` anchors, so enumeration permission outside `target/`
  is unnecessary. Other Unix platforms use read-only directory anchors and report denial
  explicitly. Traversal uses no-follow metadata and directory opens; symlinks and special
  files produce incomplete results, and inaccessible targets produce unavailable results.
  No unsafe FFI or dependency on the verification layer is introduced.
- **Race limits are visible.** Descriptors pin the directories being observed. Directory
  identity is compared after opening; root marker/location and target identity are checked
  again after traversal. Detected replacement invalidates completeness. Files can still
  grow, disappear or move between metadata calls: this is not a filesystem snapshot, and
  even partial bytes are not a guaranteed lower bound on the current tree. Registration
  and occupancy reflect the database read before scanning, not current session authority.
- **Rejected:** path-check-then-follow traversal, because a replacement symlink can escape
  the registered tree; shell `du`, because its identity and traversal policy would differ;
  assuming unreadable means zero; automatic cleanup based on this observation.

The [implementation](../../crates/cockpit-projects/src/storage.rs),
[project fixtures](../../crates/cockpit-projects/tests/storage.rs),
[race/limit fixtures](../../crates/cockpit-projects/src/storage_tests.rs),
[daemon entry points](../../crates/cockpit-daemon/tests/storage.rs) and
[CLI fixtures](../../crates/cockpit-cli/tests/storage.rs) cover this source part. No live
storage scan, pruning acceptance or wider #406 completion follows from these fixtures.
