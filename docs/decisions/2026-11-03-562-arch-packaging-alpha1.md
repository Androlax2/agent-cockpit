# Arch packaging returns to Alpha 1, beside the archive route

Issue #562. Decided by Theo on 2026-11-03: Alpha 1 is at its last step, waiting for live
acceptance, so its Arch packaging can be prepared now.

- **#562 is in the Alpha 1 milestone again.** This reverses the part of the
  [2026-11-02 scope decision](2026-11-02-568-alpha-platform-scope.md) that moved it to Alpha 2.
  #563 and #565 stay in Alpha 2.
- **It is not a prerequisite of the Alpha 1 tag or of the archive route.** The AUR recipe must
  name a published release's checksums ([aur.md](../packaging/aur.md)), so publishing it before
  the tag would be circular. The worker records this, because a gate on the tag could never pass.
  Live acceptance (#52/#137) and the archive route stay unchanged.
- **The recipe is a template rendered per release, not a committed PKGBUILD.** Rejected:
  a committed recipe with placeholder or `SKIP` checksums, or with a `latest` URL, which #562
  forbids.
- **Publication still needs its own approvals.** Like every channel here, this decision grants
  no tag, AUR push, account or install.
