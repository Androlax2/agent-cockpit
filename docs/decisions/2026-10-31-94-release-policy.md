# Cockpit releases follow OpenLogi's model, with alphas until M5 and stable 0.x from M5

Issue #94. Decided by Theo: the OpenLogi model and stable `v0.x.y` only from the M5 cutover,
as promoted betas. The choices below are the worker's, where the reference is silent; the
policy is [Releasing](../RELEASING.md).

- **The model.** One workspace version bumped by a release-plz release PR; release-plz tags
  and the release workflow owns the GitHub Release; git-cliff builds `CHANGELOG.md` from
  conventional squash subjects; a dispatched workflow cuts pre-releases on `main` without a
  bump. Cockpit adds `-alpha.N` from M3 to M5. #96 picks the tools.
- **M5 gate is #66.** Theo's answer named the cutover gate #71; #71 is the FigCraft plugin,
  so the policy uses #66 (and #52 for M3).
- **A base before the first stable.** OpenLogi counts `N` from the previous stable and names
  the version through a release PR. Cockpit has no stable until M5, so every pre-release
  before the first stable tag counts from the bootstrap commit `d2d8604` (earlier history is
  the untagged `apps/` code) and uses the current `0.1.0` without a release PR.
- **Promotion means no drift.** A stable promotes a beta only if nothing but the release PR
  landed after it. Byte-identical promotion would need the build to ignore the version stamp,
  so the stable rebuilds the verified beta's source plus the bump.
- **The gate runs in the release workflow**, on the tagged commit before the draft is
  published, so a stable tag (a squash commit that does not exist before the merge) is gated
  once.
- **Issue in the body.** A title ending in its issue number doubles the `(#N)` git-cliff
  reads, so titles are conventional subjects and the issue goes in the body.
- **Rejected:** hand-written changelog entries and hand-made tags, because both drift from
  what shipped; cargo-dist alone, because it lacks the pre-release grammar (left to #96).
