# Install pinned engine pairs in the profile's owned persistent directory

Issue #470. Installer architecture chosen by the worker within Theo's release-install
decision; dependency additions and the CLI edge explicitly approved by Theo on 2026-11-02.

- **`cockpit-sandbox` owns installation.** It consumes the existing `engine-pins.json`.
  The CLI calls it offline with persistence disabled, so a standalone CLI build does not
  include the database. A combined CLI/daemon build unifies Cargo features and enables
  persistence for the daemon; the CLI uses only the installer API. Existing daemon/session
  consumers retain transaction ownership.
- **Reuse the profile's private state layout.** Engine pairs live under
  `<state>/engines/<greywall SHA-256>-<greyproxy SHA-256>`. This keeps CLI and daemon resolution
  identical, isolates profiles and needs no additional directory environment contract.
  Versions remain available to older running sessions; there is no mutable current link.
- **Verify both downloads before extraction.** Curl is bounded to HTTPS, 90 seconds and
  128 MiB per asset, with a parent watchdog. Extraction uses tar 0.4.46 and the Rust backend
  of flate2 1.1.10; sha2 0.10.9 and tempfile 3.27.0 reuse the workspace versions. The resolved
  dependencies offer licences already accepted by the repository's policy.
- **Extract the published layout, with a narrow link exception.** Raw tar entries stream
  into private staging; expanded bytes, member sizes and counts are bounded. Only the
  expected files/directories and greywall's verified `greywatch -> greywall` alias are
  accepted. Archive modes/ownership are never applied; licence material is retained.
  Both binary commits are checked before the complete directory is renamed into place.
- **Fail without overwriting an active installation.** Error paths remove staging; a
  concurrent install or rerun accepts only an identical tree. An altered installation is
  reported for owner inspection, never silently accepted or overwritten. No engine pruning
  or legacy removal is performed; #66 owns cutover removal.
- **Keep engines readable and explicitly deny writes.** Greywall re-executes its Linux
  Landlock wrapper inside the sandbox, so hiding its ancestor directory breaks launch.
  Read-only access plus an explicit write denial protects engines even beneath a writable
  project root. Install/restart advice retains the selected Cockpit profile.
- **The installer does not start services.** #495 owns Greyproxy configuration, lifecycle
  and endpoint health. The smoke test uses real released binaries for version/help only,
  a fresh daemon home and fake provider/service commands. #433 still owns enforcement.
  The existing Greyproxy pin lacks fork PR #8; no concurrency claim is made for it.
- **Rejected:** PATH/legacy fallback, local compilation for releases, arbitrary manifest
  overrides, broad archive unpacking, and automatically restarting running sessions.
  Explicit local paths are limited to profile `dev` of debug builds, with a warning.

Evidence: [installer fixtures](../../crates/cockpit-sandbox/src/install/tests.rs),
[real daemon readiness tests](../../crates/cockpit-daemon/tests/readiness.rs),
[clean-home release smoke](../../tools/verify-engine-install.py).
