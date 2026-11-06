# The release workflow publishes the signed APT site to the Pages branch of Androlax2/torlek-apt

Issue #563. Hosting and key custody decided by Theo on 2026-11-06; the mechanism is the
worker's, with the reasons given. Supersedes the open questions of
[2026-11-03](2026-11-03-563-deb-apt-packaging.md).

- **Hosting: GitHub Pages of a new public repository, `Androlax2/torlek-apt`** (Theo). Theo
  creates it when publishing is ready; only the release workflow signs and publishes to it.
- **The signing subkey is a secret of a protected `apt` environment here; Theo holds the
  offline primary key** (Theo). Each publication waits for Theo's approval. No agent creates,
  imports or stores the real key.
- **The site is one commit on `gh-pages`, replaced with `--force-with-lease`.** History would
  keep every pruned package forever, about 24 MB per release. Rejected: a Pages deployment from
  an Actions artifact, which only deploys the repository that runs the workflow, so it would
  need a second workflow and token in `torlek-apt`.
- **A deploy key with write access on `torlek-apt` alone**, in the `apt` environment, against
  GitHub's pinned ed25519 host key. Rejected: a personal access token, which reaches more than
  one repository.
- **The pool keeps the ten newest versions of the channel.** A package is about 12 MB and
  alphas came out daily, so keeping every version would fill the 1 GB Pages limit in about
  forty releases.
- **The published site is verified against the committed key before anything is added,** and
  the served site again after the push. The site is the pool's only copy.
- **`publish-apt` runs only once the repository variable `TORLEK_APT_PUBLISH` is `true`.** A job
  that names a missing environment creates it without protection rules.
- **Packages are built from the published release's assets, checked again against
  `SHA256SUMS` and build provenance,** after `publish`, and after the Debian packages workflow
  has installed them on clean containers. Neither job gates the release.
- **`build.py` checks an archive against its own manifest, not against today's bundle rules.**
  An earlier release predates programs this checkout's `release.py` requires; the install job
  runs the release's own `verify-install` instead.
