# Debian and Ubuntu: the `torlek` package and its APT source

**Status:** packages built from the published v0.1.0-alpha.4 and alpha.3 archives install,
upgrade and remove through apt on clean Ubuntu 26.04 and Debian 13 containers, amd64 and arm64,
from a repository signed with a throwaway key ([CI evidence](#ci-evidence)). The publication
job exists but is off. [Owner setup](#owner-setup) steps 1 to 5 are done: the key, its public
half in `packaging/deb/torlek.asc`, the `apt` environment (required reviewer, `v*` tags), the
deploy key, `TORLEK_APT_SIGNING_KEY` and `TORLEK_APT_MAINTAINER`. Only step 6 remains: the
coordinator sets `TORLEK_APT_PUBLISH` after this merges. Do not document `apt install torlek` as an install
route until the first publication has passed its [site check](#publication).

This repackages the published release archives, like the [AUR recipe](aur.md). It is not an
official Debian or Ubuntu package.

## What exists

| File | Role |
| --- | --- |
| [packaging/deb/build.py](../../packaging/deb/build.py) | Builds `torlek_<version>_<arch>.deb` from one release archive and that release's `SHA256SUMS` |
| [packaging/deb/torlek/control.in](../../packaging/deb/torlek/control.in) | The reviewed control file, with version, architecture, maintainer and size as `@TOKEN@`s |
| [packaging/deb/torlek/copyright](../../packaging/deb/torlek/copyright) | The machine-readable copyright file installed to `/usr/share/doc/torlek/` |
| [packaging/deb/repository.py](../../packaging/deb/repository.py) | `index` writes the suite's `Packages` and `Release`; `prune` keeps the newest versions; `sign` writes `InRelease` and `Release.gpg`; `verify` checks a tree as apt would |
| [packaging/deb/fetch-release.sh](../../packaging/deb/fetch-release.sh) | Downloads a published release's checksums and Linux archives, and checks both checksums and build provenance |
| [packaging/deb/publish.sh](../../packaging/deb/publish.sh) | The release workflow's publication: verify the published site, add the release, sign, push |
| [packaging/deb/check-site.sh](../../packaging/deb/check-site.sh) | Downloads the suite GitHub Pages serves and verifies it |
| [packaging/deb/ci-stage.sh](../../packaging/deb/ci-stage.sh), [ci-install.sh](../../packaging/deb/ci-install.sh) | The [Debian packages workflow](../../.github/workflows/deb.yml)'s build, lintian and fixture repositories, and its clean-container install |
| [tools/tests/test_deb_package.py](../../tools/tests/test_deb_package.py) | Version order, refusals of tampered or mismatched inputs, package layout, pruning, signing, publication to a local Git remote and the site check, with throwaway keys |

The builder writes the `.deb` itself with the Python standard library: an `ar` container
holding `debian-binary`, `control.tar.xz` and `data.tar.xz`, with fixed owners and timestamps,
so the same archive gives the same bytes. Every entry is dated to the release's publication
day: lintian refuses files dated 1970. dpkg, apt and lintian read it in the
[Debian packages workflow](#ci-evidence), where the tests that need dpkg run instead of
skipping.

## Package

- **Name:** `torlek`. No Debian or Ubuntu package of that name existed on 2026-11-03
  (packages.debian.org and packages.ubuntu.com name search).
- **Architectures:** `amd64` from `linux-x86_64`, `arm64` from `linux-aarch64`: the two
  archives of the Alpha 1 plan (`ALPHA1_PLATFORMS` in [tools/release.py](../../tools/release.py)).
- **Layout:** the whole bundle goes to `/usr/lib/torlek` unchanged, with `/usr/bin/torlek` as a
  symlink into it, for the same reason as the AUR recipe: `torlek` finds `torlek-tui`, the
  helpers and the manifest through its own resolved path. The builder checks that the archive's
  files are exactly the ones its manifest lists, with their checksums. Which programs a bundle
  must hold is the rule of the release that made it: the install job runs that release's own
  `release.py verify-install` on `/usr/lib/torlek`, so alpha.3, which predates `torlek-sandbox`,
  still packages. `copyright` and `changelog.Debian.gz` go to
  `/usr/share/doc/torlek/`. The package installs nothing outside `/usr` and has no conffiles.
- **No maintainer scripts:** no `preinst`, `postinst`, `prerm` or `postrm`. The package never
  starts, stops or adopts a daemon. Per-user services are written by `torlek daemon install`.
- **Dependencies:** `libc6 (>= 2.39), libgcc-s1 (>= 4.2), git, curl, tmux (>= 3.6)`;
  `Suggests: systemd`. The programs need `GLIBC_2.39` and `GCC_4.2.0` symbols (built on
  `ubuntu-24.04` runners), and the workflow
  fails when a term `dpkg-shlibdeps` computes for the newest release is missing from `Depends`.
  `git`, `curl` and `tmux >= 3.6` are the [user guide](../USER-GUIDE.md#platforms-and-prerequisites)
  prerequisites. `systemd` is needed only for the user services. Provider CLIs are left out.
- **Maintainer:** `build.py --maintainer 'Name <address>'` is required and has no default.
  Debian policy requires an address. Publication reads it from the `apt` environment's
  `TORLEK_APT_MAINTAINER` variable; the CI fixtures use `ci-fixture@example.invalid`.

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
- **The pool keeps the channel's ten newest versions** (`prune --keep`, `TORLEK_APT_KEEP`), so
  `apt install torlek=<version>` can downgrade to any of them. A package is about 12 MB, and
  alphas came out daily: keeping every version would reach GitHub Pages' 1 GB limit in about
  forty releases.
- **One channel per site for now.** `index` refuses a package of another channel in the shared
  pool, so a `beta` suite beside `alpha` needs a per-suite pool first (#57).
- **No `Valid-Until`.** It protects against a mirror replaying an old index, but it needs the key
  online to re-sign on a schedule. HTTPS to a single host covers most of that risk for now.
- **`verify`** follows apt's signature rule: at least one good signature from the given key,
  and no bad, expired or revoked one. Then it checks every index against the signed hashes, and
  every package against its index. It is a publication check written here, not apt.

A client adds the source like this, once the first publication has passed:

```text
# /etc/apt/sources.list.d/torlek.sources
Types: deb
URIs: https://androlax2.github.io/torlek-apt/
Suites: alpha
Components: main
Signed-By: /etc/apt/keyrings/torlek.asc
```

### Hosting

GitHub Pages of a separate public repository, **`Androlax2/torlek-apt`**, served at
`https://androlax2.github.io/torlek-apt/` (Theo, 2026-11-06). Only the release workflow signs
and publishes to it. Rejected: this repository's Pages under `/apt/`, because every
documentation deploy replaces the whole site and would have to reassemble the signed tree; and
GitHub Release assets, which form a new source per tag that never updates.

The site is the single commit of its `gh-pages` branch. Each publication replaces that commit
with `--force-with-lease`, so old packages leave the repository's history instead of filling
it, and two publications cannot overwrite each other. Limits (GitHub's Pages documentation,
2026-11-03): a 1 GB site and a soft 100 GB monthly bandwidth.

### Signing key

Theo holds an offline OpenPGP primary key used only for certification. Its signing subkey is
the `TORLEK_APT_SIGNING_KEY` secret of this repository's protected `apt` environment, and each
run that uses it waits for Theo's approval (Theo, 2026-11-06). A leaked secret costs only the
subkey. No agent creates, imports or stores the real key; the tests and CI use throwaway keys
in temporary `GNUPGHOME`s.

`publish.sh` signs with every signing subkey the secret holds, and refuses one that is not in
the committed public key `packaging/deb/torlek.asc` (committed by [owner setup](#owner-setup) step 2), which is also what
`verify` and the site check trust. Users check `gpg --show-keys torlek.asc` against the
primary key's fingerprint, published in this file (the README install section and each
release's notes follow once the first publication is verified), before copying it to `/etc/apt/keyrings/torlek.asc`.

The primary key, certification only, user ID `Torlek APT <contact@theobenoit.fr>`:

```text
CC46 0FBB B9A6 9E1A 5239  A8D4 CABB C6ED 28E3 4720
```

Its signing subkey, which signs the repository, is
`398A9A8B389603FD7D2A5FF7109BE7EFA0AE2BB7`. The subkey expires on 2028-11-05; extend it before then (see [Rotation](#rotation-and-compromise)).
Only the primary fingerprint is the one to check.

### Owner setup

Once, before the first publication. Each step is Theo's; none is done by an agent.

1. **Create the key**, on a machine you trust, in a fresh directory you keep offline:

   ```sh
   export GNUPGHOME="$PWD/torlek-apt-gnupg"; mkdir -m 0700 "$GNUPGHOME"
   gpg --quick-gen-key 'Torlek APT <ADDRESS>' ed25519 cert 5y
   PRIMARY=$(gpg --with-colons --list-keys | awk -F: '$1 == "fpr" { print $10; exit }')
   gpg --quick-add-key "$PRIMARY" ed25519 sign 2y
   SUBKEY=$(gpg --with-colons --list-keys "$PRIMARY" | awk -F: '$1 == "fpr" { print $10 }' | tail -n 1)
   ```

   Keep `$GNUPGHOME`, including `openpgp-revocs.d/$PRIMARY.rev`, offline: it is the primary key
   and its revocation certificate.
2. **Export the public key** and commit it in a pull request as `packaging/deb/torlek.asc`,
   with `$PRIMARY` in this file: `gpg --armor --export "$PRIMARY" > torlek.asc`.
3. **Export the subkey alone without a passphrase**, in a throwaway copy, straight into the
   secret:

   ```sh
   export GNUPGHOME=$(mktemp -d)
   gpg --homedir "$PWD/torlek-apt-gnupg" --armor --export-secret-subkeys "$SUBKEY!" | gpg --import
   gpg --passwd "$SUBKEY"        # set an empty passphrase
   gpg --armor --export-secret-subkeys "$SUBKEY!" | gh secret set TORLEK_APT_SIGNING_KEY --env apt
   gpgconf --kill gpg-agent; rm -rf "$GNUPGHOME"
   ```

4. **Create `Androlax2/torlek-apt`**, public, with one commit on a `gh-pages` branch (a
   `README.md` saying what the site is will do), and set its Pages source to that branch.
   Publication keeps the files it finds and replaces the commit.
5. **Create the `apt` environment** in this repository's settings, with yourself as required
   reviewer and deployments limited to `v*` tags, before adding anything to it. Then add:
   - `TORLEK_APT_DEPLOY_KEY`: the private half of an SSH key (`ssh-keygen -t ed25519 -N ''`)
     whose public half is a deploy key with write access on `torlek-apt` only;
   - the `TORLEK_APT_SIGNING_KEY` secret from step 3;
   - the `TORLEK_APT_MAINTAINER` variable, `Name <address>` as the packages should show it.
6. **Turn publication on:** set this repository's variable `TORLEK_APT_PUBLISH` to `true`.
   Until then `publish-apt` does not run, so no unprotected `apt` environment is created.

### Rotation and compromise

- **Extend the subkey** before it expires: `gpg --quick-set-expire "$PRIMARY" 2y "$SUBKEY"` in
  the offline directory, then commit the new `torlek.asc` and replace the secret as in step 3.
  Users refresh their copy, because apt never updates a file under `/etc/apt/keyrings`.
- **Replace the key:** create the new one as in steps 1 to 3, commit a `torlek.asc` holding
  both public keys, and put both subkeys in the secret (concatenate the two exports). Every
  index is then signed by both, and a client that trusts either key keeps verifying, which the
  tests check. After at least one release and an announcement, drop the old key from both.
- **Compromise:** revoke the subkey (or import the primary's revocation certificate), replace
  the secret and `torlek.asc`, publish, and announce it in a GitHub security advisory and the
  README. Until users replace their key, apt refuses the new index.

## Publication

After `publish` releases a tag, [release.yml](../../.github/workflows/release.yml) runs:

1. **`deb`**, the [Debian packages workflow](../../.github/workflows/deb.yml) on that release:
   download, checksum and provenance checks, lintian, the throwaway-key fixtures, and the
   clean-container installs and upgrade.
2. **`publish-apt`**, in the `apt` environment, after Theo approves it:
   [fetch-release.sh](../../packaging/deb/fetch-release.sh) checks the published archives again;
   [publish.sh](../../packaging/deb/publish.sh) clones the site, verifies every suite in it
   against the committed key, refuses a pool file no verified index lists, builds the two
   packages dated to the release's publication day, prunes, indexes, signs, verifies and
   pushes; then
   [check-site.sh](../../packaging/deb/check-site.sh) downloads the suite from
   `androlax2.github.io` until one whole copy verifies, for up to ten minutes.

`publish.sh` refuses, and pushes nothing, when the public key is not committed, the secret
holds no usable signing key or one outside `torlek.asc`, the published site does not verify,
or the release is already in the pool. Neither job gates the release itself.

**After the first publication,** follow the README instructions from the start on a new
fixture, and link that evidence from #97 before documenting the route.

**A packaging fix alone** (the same archives, `--debian-revision 2`) is not automated: the
release workflow only publishes new tags. Until it is, rebuild by hand only with Theo's
approval.

## CI evidence

The [Debian packages workflow](../../.github/workflows/deb.yml) runs on pull requests that
change the packaging, by hand for any published prerelease, and from the release workflow.
It packages the newest published prerelease and the one before it in its channel.

Run [42258338412](https://github.com/Androlax2/agent-cockpit/actions/runs/42258338412) on
`5317bea8`, 2026-11-06, packaged `v0.1.0-alpha.4` (revision `87274175`) and `v0.1.0-alpha.3`
from their release archives after `sha256sum --check` and `gh attestation verify`:

| Package | SHA-256 |
| --- | --- |
| `torlek_0.1.0~alpha.4-1_amd64.deb` | `83c818f9c7d1a88226a2ba306e9c87373e94983f575fbb65a72c5940b8e4d103` |
| `torlek_0.1.0~alpha.4-1_arm64.deb` | `afe7e9d8be03c6ec01356ac3867cc98f45a18ea12fb3587689b1d9aa6b799598` |
| `torlek_0.1.0~alpha.3-1_amd64.deb` | `3c5062e94257eff8b59fd481b85230c5b3cb6cd5863f34055de9f81411b83e71` |
| `torlek_0.1.0~alpha.3-1_arm64.deb` | `0c0aa72c751e5093056db8767592da442a81d09e102341de34de94b4934fe571` |

The fixture packages carry the CI maintainer, so a published package's checksum differs.

- `stage`: the packaging tests ran 72 with none skipped; lintian passed with the findings
  below; `dpkg-shlibdeps` computed `libc6 (>= 2.39), libgcc-s1 (>= 4.2)`, both in `Depends`.
- `install` on Ubuntu 26.04.1 LTS and Debian 13 (trixie), each on `amd64` and `arm64`: apt
  refused the index signed by another key (`sqv`: missing key) and the changed package (hash
  sum mismatch), installed `0.1.0~alpha.3-1`, upgraded to `0.1.0~alpha.4-1`, passed
  `dpkg --verify`, `torlek version --all`, `torlek doctor` and the tag's
  `release.py verify-install` (`version=0.1.0-alpha.4`, `files=10`), then removed and purged
  it with the tester's files unchanged.

### lintian findings

lintian fails the job on any error. The warnings and notes left, with the reason each stays:

- `initial-upload-closes-no-bugs`: the changelog closes no Debian bug, because this is not an
  upload to the Debian archive.
- `no-manual-page`: Torlek has no manual page; `torlek --help` and the user guide document it.
- `extra-license-file`, `package-contains-documentation-outside-usr-share-doc`: `LICENSE` and
  `README.md` are part of the release bundle and its manifest, which stays whole under
  `/usr/lib/torlek`.
- `binary-has-unneeded-section`: the programs are the release's bytes; stripping them would
  break the manifest's checksums.

## Not yet verified

- `publish-apt` builds the packages again with the real maintainer; they differ from the
  installed fixtures only in the `Maintainer` field and the changelog's signature line, and
  are not themselves installed before the push.
- No publication: owner setup steps 1 to 5 are done (the `apt` environment with its required
  reviewer and `v*` tags, the deploy key, `TORLEK_APT_SIGNING_KEY`, `TORLEK_APT_MAINTAINER`)
  and only step 6, `TORLEK_APT_PUBLISH`, remains. `publish-apt`, the deploy key, the
  force-push and `check-site.sh` against GitHub Pages (including `~` in file names) have not
  run. Their logic is tested against a local Git remote and `file://` URLs.
- apt verified the fixtures with whichever verifier each distribution's apt uses (`sqv` or
  `gpgv`); a dual-signed index has been checked only by `verify` and the tests.
- No #97 onboarding beyond `torlek doctor`, `torlek version --all` and `verify-install`'s
  daemon start: the containers have no systemd user manager and no provider CLI.
- The copyright file names the project's Apache-2.0 license only. The Rust crates compiled into
  the programs carry their own licenses, which neither the archive nor the package lists yet.
