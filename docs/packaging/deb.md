# Debian and Ubuntu: the `torlek` package and its APT source

**Status:** package builder, repository indexer, signing and checks are tested (#563) on
fixture archives. No `.deb` has been installed with dpkg or apt, no repository is hosted, and
no signing key exists. Do not document `apt install torlek` as an install route until the
[publication checklist](#publication-checklist) has passed.

This repackages the published release archives, like the [AUR recipe](aur.md). It is not an
official Debian or Ubuntu package.

## What exists

| File | Role |
| --- | --- |
| [packaging/deb/build.py](../../packaging/deb/build.py) | Builds `torlek_<version>_<arch>.deb` from one release archive and that release's `SHA256SUMS` |
| [packaging/deb/torlek/control.in](../../packaging/deb/torlek/control.in) | The reviewed control file, with version, architecture, maintainer and size as `@TOKEN@`s |
| [packaging/deb/torlek/copyright](../../packaging/deb/torlek/copyright) | The machine-readable copyright file installed to `/usr/share/doc/torlek/` |
| [packaging/deb/repository.py](../../packaging/deb/repository.py) | `index` writes the suite's `Packages` and `Release`; `sign` writes `InRelease` and `Release.gpg`; `verify` checks a tree as apt would |
| [tools/tests/test_deb_package.py](../../tools/tests/test_deb_package.py) | Version order, refusals of tampered or mismatched inputs, package layout, and a repository signed with a throwaway key |

The builder writes the `.deb` itself with the Python standard library: an `ar` container
holding `debian-binary`, `control.tar.xz` and `data.tar.xz`, with fixed owners and timestamps,
so the same archive gives the same bytes. GNU `ar` and `tar` read it. **dpkg has not read it:**
this sandbox has no dpkg, apt or lintian. The test that runs `dpkg-deb --field` and the
`dpkg --compare-versions` cross-check skip here and run on hosts with dpkg, such as the Ubuntu
CI runners.

## Package

- **Name:** `torlek`. No Debian or Ubuntu package of that name existed on 2026-11-03
  (packages.debian.org and packages.ubuntu.com name search).
- **Architectures:** `amd64` from `linux-x86_64`, `arm64` from `linux-aarch64`: the two
  archives of the Alpha 1 plan (`ALPHA1_PLATFORMS` in [tools/release.py](../../tools/release.py)).
- **Layout:** the whole bundle goes to `/usr/lib/torlek` unchanged, with `/usr/bin/torlek` as a
  symlink into it, for the same reason as the AUR recipe: `torlek` finds `torlek-tui`, the
  helpers and the manifest through its own resolved path. The installed tree passes
  `release.refuse_changed_contents`. `copyright` and `changelog.Debian.gz` go to
  `/usr/share/doc/torlek/`. The package installs nothing outside `/usr` and has no conffiles.
- **No maintainer scripts:** no `preinst`, `postinst`, `prerm` or `postrm`. The package never
  starts, stops or adopts a daemon. Per-user services are written by `torlek daemon install`
  and `torlek sandbox install`.
- **Dependencies:** `libc6 (>= 2.39), libgcc-s1, git, curl, tmux (>= 3.6)`; `Suggests: systemd`.
  The programs are built on `ubuntu-24.04` runners (glibc 2.39), so 2.39 is the version they
  were linked against. `dpkg-shlibdeps` might find a lower bound, but it has not run.
  `git`, `curl` and `tmux >= 3.6` are the [user guide](../USER-GUIDE.md#platforms-and-prerequisites)
  prerequisites. `systemd` is needed only for the user services. Provider CLIs are left out.
- **Maintainer:** `build.py --maintainer 'Name <address>'` is required and has no default.
  Debian policy requires an address, and none has been chosen yet.

### Supported distributions

The tmux dependency is declared as the README states it, so apt refuses the package where
tmux 3.6 is not available, instead of installing a Torlek that `torlek doctor` would refuse.

| Distribution | tmux (2026-11-03) | glibc | Can apt install `torlek`? |
| --- | --- | --- | --- |
| Ubuntu 26.04 | 3.6a | 2.43 | Yes: the target fixture |
| Ubuntu 24.04, including WSL2 | 3.4 | 2.39 | No: `tmux (>= 3.6)` cannot be met |
| Debian 13 | 3.5a; 3.7c in `trixie-backports` | 2.41 | Only after `apt install -t trixie-backports tmux` |
| Debian 12 | 3.3a; 3.5a in backports | 2.36 | No |

Sources: packages.ubuntu.com and sources.debian.org on 2026-11-03. A tmux built from source
under `/usr/local` does not satisfy a dpkg dependency. Ubuntu 24.04, including WSL2, stays
archive-only: this project does not publish a backported tmux
([decision](../decisions/2026-11-03-563-deb-apt-packaging.md)).

### Version order

The tag's hyphen becomes `~`, and the Debian revision is added: `v0.1.0-alpha.1` becomes
`0.1.0~alpha.1-1`. dpkg sorts `~` before anything, even the end of the version, so
`0.1.0~alpha.1-1 < 0.1.0~alpha.2-1 < 0.1.0~alpha.10-1 < 0.1.0~beta.1-1 < 0.1.0-1`, which the
tests check. A rebuild of the same archive for a packaging fix raises only the revision
(`--debian-revision 2`). The builder accepts only `X.Y.Z-alpha.N` and `X.Y.Z-beta.N` tags
([RELEASING](../RELEASING.md#tags)).

### What the builder refuses

It fails, and writes nothing, when:

- `SHA256SUMS` is malformed, lists a file twice, or lacks either Linux archive of the tag;
- the archive is not named for this tag, or its bytes do not match its line;
- the archive has a member outside its top directory, a link, or another special file;
- its manifest names another version, channel or platform, or its files differ from the manifest;
- the maintainer has no address, or the `.deb` already exists.

### User state

The package owns files under `/usr` only. Configuration, state and user services live in the
user's home, so `apt remove` and `apt purge` keep them. The units point at `/usr/lib/torlek/bin`:
stop them with `systemctl --user disable --now` before removing the package. An upgrade
replaces the programs in place, and a running daemon keeps the old one until it restarts.
`/usr/lib/torlek/bin` is root's, so a profile's `torlek-<profile>` link is skipped, as with
[the AUR package](aur.md#user-state).

## APT repository

```text
dists/<suite>/InRelease            clear-signed Release (what apt reads)
dists/<suite>/Release              SHA256, size and path of each index
dists/<suite>/Release.gpg          detached signature, for older clients
dists/<suite>/main/binary-amd64/Packages{,.gz}
dists/<suite>/main/binary-arm64/Packages{,.gz}
pool/main/t/torlek/torlek_<version>_<arch>.deb
```

- **Suites are the release channels:** `alpha` now, `beta` when #57 opens it. `index` refuses a
  package of another channel, and a suite without both architectures. A stable suite does not
  exist, because no stable release does.
- **The pool keeps every published version** of its channel, so `apt install torlek=<version>`
  can downgrade. At about two packages per release, the 1 GB GitHub Pages limit is far off.
- **No `Valid-Until`.** It protects against a mirror replaying an old index, but it needs the key
  online to re-sign on a schedule. HTTPS to a single host covers most of that risk for now.
- **`verify`** follows apt's signature rule: at least one good signature from the given key,
  and no bad, expired or revoked one. Then it checks every index against the signed hashes, and
  every package against its index. It is a publication check written here, not apt.

A client would add the source like this, once the hosting and key exist (the URL depends on
[the hosting choice](#hosting-options)):

```text
# /etc/apt/sources.list.d/torlek.sources
Types: deb
URIs: https://androlax2.github.io/torlek-apt/
Suites: alpha
Components: main
Signed-By: /etc/apt/keyrings/torlek.asc
```

### Hosting options

`androlax2.github.io/agent-cockpit` already serves the documentation site. `docs.yml` deploys a
new Pages artifact on every run, which replaces the whole site.

- **Option A, chosen: GitHub Pages of a separate public repository** (for example
  `Androlax2/torlek-apt`, which does not exist yet), deployed from an Actions artifact by a
  manually started workflow in a protected environment. No purchased domain, no binaries in Git
  history, and the signing secret stays out of this repository's documentation workflow. Cost:
  Theo creates one repository.
- **Option B: this repository's Pages under `/apt/`.** No new repository, but every
  documentation deploy would have to reassemble the exact signed tree, or apt clients would see
  it disappear. That couples two workflows and puts the APT tree in the documentation job.
- **Option C: GitHub Release assets as a flat repository.** Each tag's assets would form a
  separate source that never updates, and `releases/latest` skips prereleases, so every alpha
  would need a new source line. It cannot be an APT source that updates.

Limits for A and B (GitHub's Pages documentation, 2026-11-03): a 1 GB site, a soft 100 GB
monthly bandwidth limit, and a deployment timeout of 10 minutes.

### Signing key

Nothing in this repository creates or holds a real key. The tests use throwaway keys in a
temporary `GNUPGHOME`.

- **Creation, by Theo, outside any agent session:** an OpenPGP primary key used only for
  certification, kept offline, with a signing subkey for the repository. The subkey expires
  (for example after two years) and is extended before then. `sign` takes the key by its full
  40-character fingerprint, never a short ID or a name.
- **Where the signing subkey lives:** the open question is between Theo's machine (each
  publication signed by hand) and an environment secret of the publishing workflow, which needs
  Theo's approval for each run. A leaked secret then costs only the subkey.
- **Distribution:** the public key is committed under `packaging/deb/` once it exists, and is
  served beside the repository. Its fingerprint goes in this file, the README install section
  and each release's notes. Users check `gpg --show-keys torlek.asc` against that fingerprint
  before copying the key to `/etc/apt/keyrings/torlek.asc`.
- **Rotation:** publish the new public key, then sign every index with both keys
  (`sign --key OLD --key NEW`) for at least one release. A client that trusts either key keeps
  verifying, which the tests check. Announce the change, then drop the old key. Users replace
  their copy of `torlek.asc`, because apt never updates a file under `/etc/apt/keyrings`.
  An expiry extension also needs users to refresh their copy.
- **Compromise:** revoke the subkey (or the primary key), re-sign with a new one, publish the
  revocation, and announce it in a GitHub security advisory and the README. Every user must
  replace their key; until they do, apt refuses the new index.

## Publication checklist

Each step that downloads, installs or publishes needs its own approval; this list grants none.

1. Download the release's checksums and both Linux archives:
   `gh release download "v$TAG" -R Androlax2/agent-cockpit -p SHA256SUMS -p "torlek-v$TAG-linux-*.tar.gz" -D "$WORK"`
2. In `$WORK`, check the archives against the checksums:
   `sha256sum -c --ignore-missing SHA256SUMS`, which must list both archives as `OK`.
3. Check each archive's build provenance:
   `gh attestation verify "torlek-v$TAG-linux-x86_64.tar.gz" --repo Androlax2/agent-cockpit`,
   then the same for `linux-aarch64`.
4. Build both packages, one per archive:
   `python3 packaging/deb/build.py --tag "$TAG" --sums "$WORK/SHA256SUMS" --archive "$WORK/torlek-v$TAG-linux-x86_64.tar.gz" --maintainer "$MAINTAINER" --date "$RELEASE_DATE" --out "$REPO/pool/main/t/torlek"`,
   where `$RELEASE_DATE` is the release's publication day, then the same for `linux-aarch64`.
5. Check them with `lintian` and `dpkg-deb --info` on Debian or Ubuntu. Explain or fix every
   finding.
6. Get the published pool, if one exists, and check it with `verify` against the current key
   before adding to it. Then run `index`, `sign` and `verify` on `$REPO` for the channel's suite.
7. On clean Ubuntu 26.04 and Debian 13 (with `trixie-backports` tmux) fixtures, serve `$REPO`
   locally, add it as a source, then run `apt install torlek`, `torlek doctor` and the #97
   onboarding. Remove the package and confirm the profile's state directories are still there.
   When an earlier package was published, also upgrade from it. Record the commands and output
   on #563.
8. Publish, only with Theo's approval, through the chosen hosting route.
9. On a new fixture, follow the published instructions from the start, then link the evidence
   from #97.

**Updates:** a new tag goes through the whole checklist with `--debian-revision 1`. A packaging
fix alone rebuilds the same archives with the next revision, and both versions stay in the pool.

## Not yet verified

- dpkg has not read the `.deb`, and neither apt nor lintian has run. This sandbox has neither
  them nor Docker. GNU `ar` and `tar` read the container. Expect lintian findings at least for
  the bundle's own `LICENSE` and `README.md` under `/usr/lib/torlek`.
- No clean APT install on Debian or Ubuntu, no `torlek doctor`, no #97 onboarding, no upgrade,
  and no removal keeping user data.
- No published release exists, so the real programs, their dynamic dependencies
  (`dpkg-shlibdeps`) and the `libc6` bound are unchecked. The tests package stand-in programs.
- apt has not read the repository. `verify` follows apt's rules, but recent apt can verify with
  Sequoia's `sqv` instead of `gpgv`, and neither has run against this tree. Dual-signed indexes
  are checked only by `verify`.
- `~` in file names on GitHub Pages, and apt over GitHub Pages, are untested.
- The copyright file names the project's Apache-2.0 license only. The Rust crates compiled into
  the programs carry their own licenses, which neither the archive nor the package lists yet.
