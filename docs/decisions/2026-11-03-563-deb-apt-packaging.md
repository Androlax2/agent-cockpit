# Debian packages repackage the release archives; the APT source waits for hosting and key decisions

Issue #563. Theo moved #563 into the Alpha 1 milestone on 2026-11-03, superseding its
2026-11-02 move to Alpha 2. The choices below are the worker's, with the reasons given; the
open questions are Theo's.

- **The package is built from one release's archive, never from source.** `build.py` checks the
  archive against that release's `SHA256SUMS` and its own manifest, and the installed bundle
  still passes `release.refuse_changed_contents`. The release pipeline stays the only owner of
  versions and bytes. Rejected: a `debian/` source package built by `dpkg-buildpackage`, which
  would compile a second set of programs whose bytes no release attests.
- **The bundle stays whole under `/usr/lib/torlek`, as in the [AUR recipe](2026-11-03-562-arch-packaging-alpha1.md).**
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
  and inventing one would publish it. **Open question:** which name and address.
- **One suite per channel (`alpha`, `beta`), keeping every version of the channel.** Rejected:
  one suite for all channels, which would move alpha users to betas without asking.
- **No `Valid-Until` in `Release`.** It needs the signing key online to re-sign on a schedule.
- **`verify` follows apt's signature rule, not gpgv's exit code,** so an index signed by an old
  and a new key verifies with either. That is the rotation procedure in
  [deb.md](../packaging/deb.md#signing-key).
- **Hosting: GitHub Pages of a separate public repository** (Theo, 2026-11-03). This
  repository's Pages site is replaced by every documentation deploy, and GitHub Release assets
  cannot form an APT source that updates. **Open question:** that repository's name and owner;
  `Androlax2/torlek-apt` is the working name. Nothing is created here.
- **Open question: where the signing subkey lives,** on Theo's machine or as a secret of the
  publishing workflow's protected environment. No key exists; none is created here.
- **Publication still needs its own approvals.** This decision grants no tag, repository,
  key, upload or install.
