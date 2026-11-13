# Arch Linux: the `torlek-bin` AUR recipe

**Status:** the recipe for `v0.1.0-alpha.4` is committed and checked against the published
archives on a clean Arch container (#562). No AUR package is published. Do not document
`yay -S torlek-bin` as an install route until the [publication checklist](#publication-checklist)
has passed.

This is an [AUR](https://wiki.archlinux.org/title/Arch_User_Repository) recipe that repackages
the published release archives. It is not an official Arch repository package, and it is not
the legacy desktop staging recipe in [packaging/arch](../../packaging/arch/PKGBUILD)
(`agent-cockpit-bin`, see [arch-package-staging.md](arch-package-staging.md)). The two share
no installed path, so both can stay installed.

## What exists

| File | Role |
| --- | --- |
| [packaging/aur/torlek-bin/PKGBUILD.in](../../packaging/aur/torlek-bin/PKGBUILD.in) | The reviewed recipe, with the tag and checksums as `@TOKEN@`s |
| [packaging/aur/torlek-bin/PKGBUILD](../../packaging/aur/torlek-bin/PKGBUILD) and [.SRCINFO](../../packaging/aur/torlek-bin/.SRCINFO) | The template rendered for one published release, now `0.1.0-alpha.4` with `pkgrel` 2: the pair an AUR push carries, with `torlek-bin.install`. Never edited by hand |
| [packaging/aur/render.py](../../packaging/aur/render.py) | `render` fills the template from one release's `SHA256SUMS` (and `--pkgrel`), writing `PKGBUILD` and its `makepkg --printsrcinfo` together or neither; `check-srcinfo` refuses a stale `.SRCINFO` |
| [packaging/aur/ci-install.sh](../../packaging/aur/ci-install.sh) | The release workflow's two Arch jobs, each as root in a fresh `archlinux:base-devel` container: render, build as a user, `namcap`, `pacman -U`, `verify-install`, `torlek doctor`, `pacman -R` with the user's files unchanged. `install-arch` takes the run's own candidate archive; `install-arch-published` takes the committed pair |
| [tools/tests/test_aur_package.py](../../tools/tests/test_aur_package.py) | Tag and checksum refusals, `vercmp` order, `.SRCINFO`, the committed pair against the template, and a `makepkg` build of a `tools/release.py` bundle of stand-in programs |

The committed `PKGBUILD`, `.SRCINFO` and `torlek-bin.install` are what the AUR repository holds; the template is the
only file to edit. The pair names one published release's checksums: there is no placeholder or
`SKIP` checksum, and no `latest` URL. It trails a new tag until that tag's
[update](#update-checklist) is merged
([decision](../decisions/2026-11-06-562-committed-aur-recipe.md)).

### What `install-arch-published` proves

For the committed pair, on every pull request that changes the recipe and on the nightly and
dispatched runs. A tag's run leaves it out, because the pair names an earlier release:

- It is byte for byte the rendering of the `SHA256SUMS` that its release published.
- `makepkg` downloads the x86_64 and aarch64 archives from the release and each passes its
  checksum.
- The x86_64 package builds with every dependency resolved, installs with `pacman -U`, and
  passes the release's own `verify-install` taken from its tagged commit.
- `torlek doctor` runs as a user and reports the release build.
- `pacman -R` removes every packaged path and leaves the user's home, with a file placed in
  the profile's configuration and state folders, unchanged.

### Expected `namcap` findings

Both Arch jobs print `namcap`'s findings for the recipe and the built package, and fail on an
error-level one. `0.1.0alpha.4-1` has warnings only:

- `Reference to x86_64 should be changed to $CARCH`: each `source_<arch>` names its own
  archive. `$CARCH` there would make the `.SRCINFO` depend on the machine that printed it.
- `Unused shared library '/usr/lib64/ld-linux-x86-64.so.2'`, once per program: the release
  build lists the loader as a needed library, and the package ships the programs unchanged.
- `Dependency included, but may not be needed` for `git`, `curl` and `tmux`: Torlek runs
  them as programs, which `namcap` cannot see; it only reads linked libraries.

## Package

- **Name:** `torlek-bin`, the AUR convention for a prebuilt package. It `provides` and
  `conflicts` with `torlek`, the name a later source-built recipe would take. Neither name was
  taken on 2026-11-06: the AUR RPC `info` and `search` returned nothing, and neither did the
  official repositories' package search. Theo confirmed the name and the template's
  `Maintainer` line that day.
- **Architectures:** `x86_64` and `aarch64`, the two Linux archives of the Alpha 1 release plan
  (`ALPHA1_PLATFORMS` in [tools/release.py](../../tools/release.py)), each with its own
  `source_<arch>` and `sha256sums_<arch>`.
- **Layout:** the whole bundle goes to `/usr/lib/torlek` unchanged, with `/usr/bin/torlek` as a
  symlink into it. `torlek` finds `torlek-tui`, the daemon its helpers, and both the release
  manifest (`share/torlek/manifest.json`) through their own resolved path, so the bundle cannot
  be split across `/usr/bin` and `/usr/share`. The unchanged tree still passes the manifest
  check. The license is linked to `/usr/share/licenses/torlek-bin/LICENSE`.
- **Masks helper (#390):** `package()` also installs the bundle's `share/torlek/systemd` units,
  `torlek-engine-masks.socket` and `torlek-engine-masks@.service`, to `/usr/lib/systemd/system`;
  the helper they run is the bundle's own `/usr/lib/torlek/bin/torlek-sandbox`. Theo decided on
  2026-11-13 that the package installs and enables them itself, so the owner copies no command.
  [torlek-bin.install](../../packaging/aur/torlek-bin/torlek-bin.install), which `render.py`
  publishes beside the recipe, enables the socket on install and upgrade (restarting it when
  systemd runs) and stops and disables it before removal; after removal it deletes the socket's
  owner drop-in the engines' setup scripts write, unless `torlek setup engine-masks`'s socket
  still uses it. The files are the ones that setup step installs, the helper's path alone told
  apart: `the_packages_install_the_files_the_setup_step_installs_under_their_own_paths` in
  `crates/cockpit-containers/tests/masks_install.rs` compares them. A release whose bundle
  predates the units, such as the committed `0.1.0-alpha.4`, installs none, and the install
  script does nothing; that recipe-only change is `pkgrel` 2.
- **Dependencies:** `glibc` and `libgcc`, the only libraries the programs link (`libc`, `libm`
  and `libgcc_s`), plus the [user guide](../USER-GUIDE.md#platforms-and-prerequisites)'s
  prerequisites: `git`, `curl` and `tmux>=3.6`. Optional: `systemd` for `torlek daemon install`,
  `bubblewrap` for `agents.sandbox = "required"`, and `github-cli` to file a bug report as an
  issue. Provider CLIs are left out, because their AUR names are not owned here.
- **License:** Apache-2.0, the archive's `LICENSE`.

### Version order

pacman does not allow a hyphen in `pkgver`, so the renderer drops the tag's only hyphen:
`0.1.0-alpha.1` becomes `0.1.0alpha.1`. `vercmp` then orders `0.1.0alpha.1 < 0.1.0alpha.2 <
0.1.0alpha.10 < 0.1.0beta.1 < 0.1.0`, which the tests check. `0.1.0_alpha.1` or `0.1.0.alpha.1`
would sort after `0.1.0`, and a later stable package would then look like a downgrade. The
renderer accepts only `X.Y.Z-alpha.N` and `X.Y.Z-beta.N` tags, the release tags that exist
([RELEASING](../RELEASING.md#tags)).

### User state

The package owns files under `/usr` only. A profile's configuration, state and user services
(`torlek.service`, `torlek-notify.service`, and `torlek-greyproxy-*.service` from a release before #828) live in the
user's home and are written by `torlek daemon install`, not by pacman, so `pacman -R` keeps them.
Their units still point at `/usr/lib/torlek/bin`: stop them with `systemctl --user disable --now`
before removing the package. An upgrade replaces the programs in place, and a running daemon
keeps the old one until it restarts.

`/usr/lib/torlek/bin` is root's, so `torlek --profile next daemon install` (the profile proposed
beside a legacy Cockpit) installs the unit but skips the `torlek-next` command beside the program,
and says so. Select that profile with `torlek --profile next` or `TORLEK_PROFILE=next`. Never run
the install as root to get the link: pacman would not own it.

## Publication checklist

The first AUR submission. Each step that downloads, installs or publishes needs its own
approval; this list grants none. `$TAG` is the committed recipe's `_tag`, without its `v`.

1. Download the release's checksums and both Linux archives:
   `gh release download "v$TAG" -R Androlax2/agent-cockpit -p SHA256SUMS -p "torlek-v$TAG-linux-*.tar.gz" -D "$WORK"`
2. In `$WORK`, check the archives against the checksums:
   `sha256sum -c --ignore-missing SHA256SUMS`, which must list both archives as `OK`.
3. Check the build provenance of each archive:
   `gh attestation verify "torlek-v$TAG-linux-x86_64.tar.gz" --repo Androlax2/agent-cockpit`,
   then the same for `linux-aarch64`.
4. Check the names again: `torlek-bin` and `torlek` must still be free on the AUR
   (`https://aur.archlinux.org/rpc/v5/info?arg[]=torlek-bin&arg[]=torlek`) and in the official
   repositories.
5. Check that the template's `Maintainer` line still names the person whose AUR account will
   push.
6. Check the two release-workflow jobs: `install-arch` on the tag's own run, and
   `install-arch-published` on the pull request that committed this recipe. Read its `namcap`
   output and explain or fix every finding.
7. On a clean Arch machine or VM with a systemd user manager, which the container lacks:
   build the committed recipe in a clean chroot (`extra-x86_64-build` from `devtools`), install
   the package with `pacman -U` and walk the #97 onboarding scenario (`torlek daemon install`,
   a session). Also run `torlek --profile next daemon install`. Record the commands and their
   output on #562.
8. Publish, only with Theo's approval, from the AUR account whose SSH key is registered there:
   `git -c init.defaultBranch=master clone ssh://aur@aur.archlinux.org/torlek-bin.git`, which
   gives an empty repository until the first push creates the package. Copy in
   `packaging/aur/torlek-bin/PKGBUILD` and `.SRCINFO`, nothing else, run
   `python3 packaging/aur/render.py check-srcinfo` on the clone, then commit and push `master`.
9. Check the published route on a new fixture: `git clone https://aur.archlinux.org/torlek-bin.git`,
   then `makepkg -si` and `torlek doctor`. Link the evidence from #97.

## Update checklist

The AUR keeps every push and refuses a rewritten history, so a wrong push is corrected by a
later one. Nothing reaches the AUR that was not merged here first.

**A new release tag.** `pkgrel` starts again at 1.

1. Steps 1 to 3 of the [publication checklist](#publication-checklist), for the new tag.
2. Replace the committed pair:
   `rm packaging/aur/torlek-bin/PKGBUILD packaging/aur/torlek-bin/.SRCINFO`, then
   `python3 packaging/aur/render.py render --tag "$TAG" --sums "$WORK/SHA256SUMS" --out packaging/aur/torlek-bin`
3. Open a pull request with the pair and this page's status line. Its `install-arch-published`
   job must pass before it merges.
4. Steps 6 to 9 of the publication checklist. In step 7, also upgrade from the `torlek-bin`
   published before, with a daemon running. In step 8, the clone already holds the package:
   replace its two files.

**A fix to the recipe alone.** Change the template, then follow the four steps above for the
same tag, adding `--pkgrel` with one above the published number to the `render` command, so
that AUR helpers see an update. This holds for any template change once a recipe is on the AUR.

**Never push** a `--development` rendering (`check-srcinfo` refuses it), a recipe whose release
is still a draft, or a pair that differs from the committed one.

## Not yet verified

- No AUR package is published, so the route in step 9 has never run.
- No clean-chroot build has run: the CI jobs build in a fresh `archlinux:base-devel` container.
- systemd user services, providers, the native sandbox, the #97 onboarding scenario and an
  upgrade between two published packages have not run from this package: step 7 does, by hand.
- `aarch64` is never built or installed: `install-arch-published` only downloads its archive
  and checks it against the recipe. There is no official Arch image for aarch64.
- `install-arch` is not in `publish`'s needs, so a tag publishes even when it fails: step 6
  stops the AUR submission instead
  ([decision](../decisions/2026-11-06-562-committed-aur-recipe.md)).
- On the Ubuntu runners, the `.SRCINFO`, `vercmp` and fixture tests report themselves as
  skipped. Both Arch jobs run them.
