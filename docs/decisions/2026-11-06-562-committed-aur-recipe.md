# The torlek-bin recipe of one published release is committed beside its template

Issue #562. Decided by the worker on 2026-11-06, from Theo's instruction that day to deliver a
reviewed `PKGBUILD` and matching `.SRCINFO` pinned to the published `v0.1.0-alpha.4`. It amends
one point of the 2026-11-03 decision (the section at the end of this record), which kept only
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

## Revisited 2026-11-03 (#562, Arch packaging returns to Alpha 1)

The earlier decision of this record, by Theo on 2026-11-03: Alpha 1 is at its last step,
waiting for live acceptance, so its Arch packaging can be prepared now, beside the archive
route.

- **#562 is in the Alpha 1 milestone again.** This reverses the part of the
  [2026-11-02 scope decision](2026-11-02-568-alpha-platform-scope.md) that moved it to Alpha 2.
  #563 and #565 stay in Alpha 2.
- **It is not a prerequisite of the Alpha 1 tag or of the archive route.** The AUR recipe must
  name a published release's checksums ([aur.md](../packaging/aur.md)), so publishing it before
  the tag would be circular. The worker records this, because a gate on the tag could never pass.
  Live acceptance (#52/#137) and the archive route stay unchanged.
- **The recipe was a template rendered per release, not a committed PKGBUILD.** The top of
  this record replaced that: it commits one rendering per published release. Rejected: a committed recipe with
  placeholder or `SKIP` checksums, or with a `latest` URL, which #562 forbids.
- **Publication still needs its own approvals.** Like every channel here, this decision grants
  no tag, AUR push, account or install.
