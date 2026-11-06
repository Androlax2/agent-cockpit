# Debian packages repackage the release archives; the APT source is hosted on Androlax2/torlek-apt

Issue #563. Theo moved #563 into the Alpha 1 milestone on 2026-11-03, superseding its
2026-11-02 move to Alpha 2. The choices below are the worker's, with the reasons given. The
questions Theo owned (hosting, key custody) were answered on 2026-11-06, in the last section.

- **The package is built from one release's archive, never from source.** `build.py` checks the
  archive against that release's `SHA256SUMS` and its own manifest, and the installed bundle
  passes the release's own `verify-install`. The release pipeline stays the only owner of
  versions and bytes. Rejected: a `debian/` source package built by `dpkg-buildpackage`, which
  would compile a second set of programs whose bytes no release attests.
- **The bundle stays whole under `/usr/lib/torlek`, as in the [AUR recipe](2026-11-06-562-committed-aur-recipe.md).**
  `torlek` finds its programs and manifest through its resolved path.
- **The `.deb` is written with the Python standard library** (`ar`, `tar.xz`, fixed owners and
  timestamps), so its bytes are reproducible and it builds and tests on hosts without dpkg,
  such as Arch and this sandbox. Rejected: requiring `dpkg-deb`. dpkg has not read the output
  yet; the `dpkg-deb` test runs where dpkg exists, such as the Ubuntu CI runners.
- **`Depends: tmux (>= 3.6)` as the README states it, even though Ubuntu 24.04 cannot meet it.**
  A weaker dependency would install a Torlek that `torlek doctor` refuses, which hides the gap.
  The supported targets are Ubuntu 26.04 and Debian 13 with `trixie-backports` tmux.
  Ubuntu 24.04, including WSL2, stays archive-only (Theo, 2026-11-03): publishing a backported
  tmux would make this project maintain a terminal multiplexer's security updates. Revisit if
  24.04 users ask for APT.
- **`libc6 (>= 2.39)`, the glibc of the `ubuntu-24.04` build runners,** until `dpkg-shlibdeps`
  runs on a real build. A lower bound would be a guess.
- **No maintainer scripts.** Services are per user, written by `torlek daemon install`; a root
  script can neither reach a user's systemd manager nor know which profile it belongs to.
- **Versions are `X.Y.Z~channel.N-R`.** `~` keeps every prerelease below `X.Y.Z`; the revision
  `R` counts packaging-only rebuilds of the same archive.
- **The maintainer is a required argument with no default.** Debian policy requires an address,
  and inventing one would publish it. Publication reads it from the `apt` environment's
  `TORLEK_APT_MAINTAINER` variable, which Theo sets.
- **One suite per channel (`alpha`, `beta`)**, keeping the ten newest versions since
  the 2026-11-06 section below. Rejected:
  one suite for all channels, which would move alpha users to betas without asking.
- **No `Valid-Until` in `Release`.** It needs the signing key online to re-sign on a schedule.
- **`verify` follows apt's signature rule, not gpgv's exit code,** so an index signed by an old
  and a new key verifies with either. That is the rotation procedure in
  [deb.md](../packaging/deb.md#signing-key).
- **Hosting: GitHub Pages of a separate public repository** (Theo, 2026-11-03). This
  repository's Pages site is replaced by every documentation deploy, and GitHub Release assets
  cannot form an APT source that updates. Its name, `Androlax2/torlek-apt`, was decided on
  2026-11-06 (the section below).
- **Where the signing subkey lives** was decided on 2026-11-06 (the section
  below): a secret of the protected `apt` environment.
- **Publication still needs its own approvals.** This decision grants no tag, repository,
  key, upload or install.

## Revisited 2026-11-06 (#563, hosting and key custody)

Hosting and key custody decided by Theo on 2026-11-06; the mechanism is the worker's, with the
reasons given. The release workflow publishes the signed APT site to the Pages branch of
`Androlax2/torlek-apt`. This answers the open questions above.

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
- **The published site is verified against the committed key before anything is added:**
  every suite in it, and no pool file outside a verified index, since `index` signs the whole
  pool. The served site is verified again after the push. The site is the pool's only copy.
- **`publish-apt` runs only once the repository variable `TORLEK_APT_PUBLISH` is `true`.** A job
  that names a missing environment creates it without protection rules.
- **Packages are built from the published release's assets, checked again against
  `SHA256SUMS` and build provenance,** after `publish`, and after the Debian packages workflow
  has installed the same archives, packaged with the CI maintainer, on clean containers.
  Neither job gates the release.
- **`build.py` checks an archive against its own manifest, not against today's bundle rules.**
  An earlier release predates programs this checkout's `release.py` requires; the install job
  runs the release's own `verify-install` instead.
