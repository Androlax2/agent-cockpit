# The torlek-bin recipe of one published release is committed beside its template

Issue #562. Decided by the worker on 2026-11-06, from Theo's instruction that day to deliver a
reviewed `PKGBUILD` and matching `.SRCINFO` pinned to the published `v0.1.0-alpha.4`. It amends
one point of the [2026-11-03 decision](2026-11-03-562-arch-packaging-alpha1.md), which kept only
a template because no release existed to name.

- **`packaging/aur/torlek-bin/PKGBUILD` and `.SRCINFO` are committed, rendered for one published
  release.** They are the pair an AUR push carries, so what reaches the AUR was reviewed and
  checked here first. The template stays the only file to edit:
  `tools/tests/test_aur_package.py` refuses a pair that is not its rendering.
- **The release workflow's `install-arch-published` job checks the pair against the published
  release** on a clean Arch container
  ([what it proves](../packaging/aur.md#what-install-arch-published-proves)). It runs the
  release's own `verify-install` from the tagged commit, because `tools/release.py` on `main`
  moves on with later bundles.
- **The pair trails a new tag until its update is merged.** A tag's run cannot commit it: the
  checksums exist only once the release is published. `install-arch` still checks every
  candidate with a recipe rendered from the run's own archives.
- **Rejected:** rendering only on the maintainer's machine at submission time. Nothing then
  reviews or tests the exact bytes pushed to the AUR.
- **Rejected:** a workflow of its own for the job. The release workflow already holds the Arch
  container, its pin and its pull-request paths; an update costs one release dry run.
- **Publication still needs its own approvals**, as before: no AUR push, account or install is
  granted here.
